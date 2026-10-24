//! 上传：`/backend/empty`（HTTP 黑洞）与 `/backend/ws-upload`（WebSocket 主通道）。
//!
//! 两条通道都**只累加长度、绝不缓冲**——上游 PHP 也是直接丢弃，但 PHP 会先把请求体
//! 交给 SAPI；这里逐帧 poll，10 GB 上传的 RSS 增长应当接近 0。
//!
//! WS 结算协议（PLAN §3）：
//! 1. 客户端连上 `/backend/ws-upload`；
//! 2. 服务端**第一帧**下发 `{"token":"..."}`（客户端必须先读它，否则拿不到结算凭证）；
//! 3. 客户端持续推二进制帧，服务端只数长度；
//! 4. 客户端关闭连接后，服务端把 `{bytes, ms, gbps}` 写入一次性 token（10 s 过期）；
//! 5. 客户端 `GET /backend/ws-result?token=...` 取回服务端口径的权威字节数。
//!
//! 为什么不让服务端在 WS 里直接回结果：客户端停止发送到关闭握手之间还有缓冲在排空，
//! 服务端只有等连接真正关闭才数得准；而且 WS 帧里塞 JSON 会和二进制数据抢通道。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

/// 结算结果有效期。
const SETTLE_TTL: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settled {
    pub bytes: u64,
    pub ms: f64,
    pub gbps: f64,
}

#[derive(Default)]
pub struct Settlements {
    inner: Mutex<HashMap<String, (Settled, Instant)>>,
}

impl Settlements {
    /// 插入前统一做清理：先刷掉过期的，仍然超限就挤掉最旧的一条。
    /// 这样表的大小永远 ≤ `MAX_SETTLEMENTS`，不会因为客户端反复握手而涨。
    fn insert_locked(
        map: &mut HashMap<String, (Settled, Instant)>,
        token: String,
        result: Settled,
    ) {
        map.retain(|_, (_, at)| at.elapsed() < SETTLE_TTL);
        if map.len() >= crate::limits::MAX_SETTLEMENTS {
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(token, (result, Instant::now()));
    }
}

impl Settlements {
    /// WS 路径需要先生成 token 再插入（要提前告诉客户端），单测用这个便捷版本。
    #[cfg(test)]
    fn insert(&self, result: Settled) -> String {
        let token = random_token();
        self.insert_with_token(token.clone(), result);
        token
    }

    fn take(&self, token: &str) -> Option<Settled> {
        let mut map = self.inner.lock().expect("结算表锁中毒");
        match map.remove(token) {
            Some((v, at)) if at.elapsed() < SETTLE_TTL => Some(v),
            _ => None,
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

fn random_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::getrandom(&mut buf).expect("读取系统随机数失败");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// GET /backend/empty：200、零字节、可复用连接。
pub async fn empty_get() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        .header(header::CONTENT_LENGTH, "0")
        .body(Body::empty())
        .expect("响应构造失败")
        .into_response()
}

/// POST /backend/empty：逐帧丢弃请求体，只计数。
///
/// `Expect: 100-continue` 由 hyper 在第一次 poll body 时自动回 `100 Continue`，
/// 所以这里只要**确实去 poll** 就不会把客户端卡住。
pub async fn empty_post(body: Body, limiter: std::sync::Arc<crate::pace::RateLimiter>) -> Response {
    let started = Instant::now();
    let mut bytes = 0u64;
    let mut frames = 0u64;
    use http_body_util::BodyExt;
    let mut body = std::pin::pin!(body);
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(f) => {
                let n = f.data_ref().map(|d| d.len() as u64).unwrap_or(0);
                if n > 0 {
                    bytes += n;
                    frames += 1;
                }
                if limiter.is_limited() {
                    limiter.reserve(n as usize).await;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "上传请求体读取出错");
                return (StatusCode::BAD_REQUEST, "body error").into_response();
            }
        }
    }
    let secs = started.elapsed().as_secs_f64();
    let gbps = if secs > 0.0 {
        bytes as f64 * 8.0 / secs / 1e9
    } else {
        0.0
    };
    tracing::info!(bytes, frames, secs, gbps, "UPLOAD http");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_LENGTH, "0")
        .header("x-received-bytes", bytes.to_string())
        .body(Body::empty())
        .expect("响应构造失败")
        .into_response()
}

/// GET /backend/ws-upload：升级为 WebSocket。
///
/// 由 `main` 的包装函数注入结算表与限速值，这样本模块不需要认识整个 `App`。
pub fn ws_upload(
    ws: WebSocketUpgrade,
    settlements: std::sync::Arc<Settlements>,
    limiter: std::sync::Arc<crate::pace::RateLimiter>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Response {
    // 上限的作用是“内存有界”，不是防攻击：axum 会把一条消息聚合完再交给我们，
    // 不设限的话一个 10 GB 的帧就等于一次 10 GB 的内存分配。
    ws.max_message_size(crate::limits::MAX_WS_MESSAGE_BYTES)
        .max_frame_size(crate::limits::MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_ws(socket, settlements, limiter, permit))
}

async fn handle_ws(
    mut socket: WebSocket,
    settlements: std::sync::Arc<Settlements>,
    limiter: std::sync::Arc<crate::pace::RateLimiter>,
    // 持有许可直到连接结束，释放回信号量；这里不解引用，删掉就会提前放行
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    // 先下发 token：客户端必须在推数据前读到它
    let token = random_token();
    let hello = serde_json::json!({ "token": token }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    // 结算窗口用「首字节 → 末字节」，而不是整段连接时长：
    // 连接建立、token 下发、客户端拿到 token 前那段空转都是固定开销，摊进速率里
    // 会让 1 Gbps / 6 s 的窗口少 1.3%（实测 1057 vs 1071）。测速应该只量传输本身。
    // 残留偏差是“最后一个帧的传输时间没算进去”，1 Gbps 下约 8 ms / 17 s ≈ 0.05%。
    let opened = Instant::now();
    let mut first_byte: Option<Instant> = None;
    let mut last_byte = opened;
    let mut bytes = 0u64;
    // 限速要“先预留再读”，不能“先读再睡”：
    // 先读的话，每次都在等数据到达（客户端被上一轮的睡眠卡住）之后再睡满一个窗口，
    // 两者串行叠加，1 Gbps 实测只跑到 0.86 Gbps。
    // 先预留则在睡眠期间数据已经在缓冲里，醒来立刻能读，睡眠时长就是真正的节流。
    let mut expected = crate::limits::WS_FRAME_HINT;
    loop {
        // 先预留再读：睡眠期间客户端的数据会堆在内核缓冲里，醒来立刻能读，
        // 睡眠时长就是真正的节流。反过来“先读再睡”是两段串行等待，
        // 1 Gbps 实测只跑到 0.86 Gbps（-14%）。
        if limiter.is_limited() {
            limiter.reserve(expected).await;
        }
        let Some(msg) = socket.recv().await else {
            break;
        };
        match msg {
            Ok(Message::Binary(b)) => {
                let now = Instant::now();
                if first_byte.is_none() {
                    first_byte = Some(now);
                }
                last_byte = now;
                let n = b.len() as u64;
                bytes += n;
                if limiter.is_limited() {
                    expected = n as usize;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(error = %e, "WS 上传连接中断");
                break;
            }
        }
    }
    let span = match first_byte {
        Some(f) => last_byte.saturating_duration_since(f),
        None => Duration::ZERO,
    };
    let ms = if span > Duration::from_millis(1) {
        span.as_secs_f64() * 1000.0
    } else {
        // 只有一帧（或没有）：退化成整段连接时长，避免除出天文数字
        opened.elapsed().as_secs_f64() * 1000.0
    };
    let gbps = if ms > 0.0 {
        bytes as f64 * 8.0 / (ms / 1000.0) / 1e9
    } else {
        0.0
    };
    let settled = Settled { bytes, ms, gbps };
    settlements.insert_with_token(token, settled.clone());
    tracing::info!(bytes, ms, gbps, "UPLOAD ws");
}

impl Settlements {
    /// WS 路径已经先生成了 token（要提前告诉客户端），所以单独一个插入方法。
    fn insert_with_token(&self, token: String, result: Settled) {
        let mut map = self.inner.lock().expect("结算表锁中毒");
        Self::insert_locked(&mut map, token, result);
    }
}

#[derive(Deserialize)]
pub struct ResultQuery {
    pub token: Option<String>,
}

/// GET /backend/ws-result?token=...：一次性取回结算。
pub async fn ws_result(
    State(settlements): State<std::sync::Arc<Settlements>>,
    axum::extract::Query(q): axum::extract::Query<ResultQuery>,
) -> Response {
    match q.token.as_deref().and_then(|t| settlements.take(t)) {
        Some(v) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(v),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({ "error": "token 无效或已过期" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_single_use_and_expires() {
        let s = Settlements::default();
        let token = s.insert(Settled {
            bytes: 1234,
            ms: 10.0,
            gbps: 1.0,
        });
        assert_eq!(s.take(&token).map(|v| v.bytes), Some(1234));
        assert!(s.take(&token).is_none(), "一次性 token 不能取第二次");
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn unknown_token_is_none() {
        let s = Settlements::default();
        assert!(s.take("deadbeef").is_none());
    }

    #[test]
    fn settlements_never_grow_past_limit() {
        let s = Settlements::default();
        // 模拟客户端疯狂握手：插入远超上限的条目
        for _ in 0..(crate::limits::MAX_SETTLEMENTS * 2) {
            s.insert(Settled {
                bytes: 1,
                ms: 1.0,
                gbps: 1.0,
            });
        }
        assert!(
            s.len() <= crate::limits::MAX_SETTLEMENTS,
            "结算表涨到了 {}",
            s.len()
        );
    }

    #[test]
    fn token_is_hex_and_unique() {
        let s = Settlements::default();
        let a = s.insert(Settled {
            bytes: 1,
            ms: 1.0,
            gbps: 1.0,
        });
        let b = s.insert(Settled {
            bytes: 1,
            ms: 1.0,
            gbps: 1.0,
        });
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}

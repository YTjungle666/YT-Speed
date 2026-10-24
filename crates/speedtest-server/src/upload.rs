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

/// 上传限速器：按“累计字节对应的时刻表”睡眠，让读取速度不超过目标速率。
///
/// 为什么靠睡而不是丢数据：睡眠会让接收窗口收缩，TCP 自然把客户端压下来，
/// 这才是真实的限速；丢数据会变成“限速 + 重传”，客户端看到的数字会失真。
pub struct Pacer {
    bytes_per_sec: Option<f64>,
    started: std::time::Instant,
    bytes: u64,
}

impl Pacer {
    pub fn new(limit_mbps: f64) -> Self {
        Self {
            bytes_per_sec: if limit_mbps > 0.0 {
                Some(limit_mbps * 1_000_000.0 / 8.0)
            } else {
                None
            },
            started: std::time::Instant::now(),
            bytes: 0,
        }
    }

    /// 记下刚收到的字节，必要时让读取停一会儿。
    pub async fn account(&mut self, n: u64) {
        self.bytes += n;
        if let Some(bps) = self.bytes_per_sec {
            let due = self.started + Duration::from_secs_f64(self.bytes as f64 / bps);
            let now = std::time::Instant::now();
            if due > now {
                tokio::time::sleep(due - now).await;
            }
        }
    }
}

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
pub async fn empty_post(body: Body, limit_mbps: f64) -> Response {
    let started = Instant::now();
    let mut pacer = Pacer::new(limit_mbps);
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
                pacer.account(n).await;
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
    limit_mbps: f64,
) -> Response {
    ws.max_message_size(usize::MAX)
        .max_frame_size(usize::MAX)
        .on_upgrade(move |socket| handle_ws(socket, settlements, limit_mbps))
}

async fn handle_ws(
    mut socket: WebSocket,
    settlements: std::sync::Arc<Settlements>,
    limit_mbps: f64,
) {
    // 先下发 token：客户端必须在推数据前读到它
    let token = random_token();
    let hello = serde_json::json!({ "token": token }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    let started = Instant::now();
    let mut bytes = 0u64;
    let mut pacer = Pacer::new(limit_mbps);
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Binary(b)) => {
                let n = b.len() as u64;
                bytes += n;
                pacer.account(n).await;
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(error = %e, "WS 上传连接中断");
                break;
            }
        }
    }
    let ms = started.elapsed().as_secs_f64() * 1000.0;
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
        map.retain(|_, (_, at)| at.elapsed() < SETTLE_TTL);
        map.insert(token, (result, Instant::now()));
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

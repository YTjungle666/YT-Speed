//! 下载流：`/backend/garbage`。
//!
//! 上游 PHP 每次现生成随机数据，这里改成**启动时**用 `getrandom` 预生成 8 MiB，
//! 之后每条响应按 1 MiB 切片轮转（`Bytes::slice` 零拷贝）。这么做有两个原因：
//! 1. 10 GbE 下真正要打满带宽，随机数生成会成为瓶颈；
//! 2. 响应长度必须能给出精确 `Content-Length`，不能让 hyper 用 chunked。
//!
//! 契约（PLAN §3）：`ckSize` 非数字或 ≤0 → 4 MiB 总量，>1024 → 1024 MiB；
//! `Content-Length = ckSize × 1 MiB`；`octet-stream` + `attachment` + `no-store` + 禁压缩。

use std::convert::Infallible;

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::stream;

use crate::config::{BLOCK_BYTES, SLICE_BYTES};

/// 预生成的随机块，进程生命周期内共享。
#[derive(Clone)]
pub struct Payload {
    block: Bytes,
}

impl Payload {
    /// 用系统 CSPRNG 填满 8 MiB。熵足够高（>7.9 bit/byte），可以直接当下载数据。
    pub fn generate() -> Self {
        let mut buf = vec![0u8; BLOCK_BYTES];
        getrandom::getrandom(&mut buf).expect("读取系统随机数失败");
        Self {
            block: Bytes::from(buf),
        }
    }

    pub fn block(&self) -> &Bytes {
        &self.block
    }
}

/// `ckSize` 参数解析：非法值给默认 4，超过 1024 截到 1024。
pub fn parse_ck_size(raw: Option<&str>) -> u32 {
    let ck = raw
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0);
    match ck {
        Some(v) if v >= 1024.0 => 1024,
        Some(v) => (v as u32).max(1),
        None => 4,
    }
}

/// 构造下载响应。传入聚合限速器时按块摊平速率（用于“温和模式”）。
pub fn garbage_response(
    payload: &Payload,
    ck_size: u32,
    endless: bool,
    limiter: std::sync::Arc<crate::pace::RateLimiter>,
) -> Response {
    // `stream=1`：不给 Content-Length，一直发到客户端断开为止。
    // 测速客户端每条流只发一次请求就能跑满整个测试窗口，省掉“请求结束→再发一次”
    // 的同步空档（1 GiB 在 9 Gbit/s 下约 1 s 就发完，6 条流会同时空一下）。
    // 上限 32 GiB 是兜底：客户端异常消失时不会有一条响应无限期挂着。
    let total = if endless {
        crate::limits::MAX_STREAM_BYTES
    } else {
        ck_size as usize * SLICE_BYTES
    };
    let block = payload.block().clone();

    let body_stream = stream::unfold((0usize, 0usize), move |(offset, sent)| {
        let block = block.clone();
        let limiter = limiter.clone();
        async move {
            if sent >= total {
                return None;
            }
            let take = SLICE_BYTES.min(total - sent);
            let start = offset % block.len();
            // 单块内切片；跨块时（不会发生，8 MiB 是 1 MiB 的整数倍）也安全
            let chunk = block.slice(start..start + take);
            // 限速交给**聚合**限速器：所有并发响应共用一条时间轴，
            // 否则 8 路并行会把 1000 Mbit/s 限速乘成 8 倍（第一版就是这样，实测 4.8 Gbit/s）。
            if limiter.is_limited() {
                limiter.reserve(take).await;
            }
            Some((
                Ok::<Bytes, Infallible>(chunk),
                ((offset + take) % block.len(), sent + take),
            ))
        }
    });

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_DISPOSITION,
            "attachment; filename=random.bin",
        )
        .header(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate")
        // 随机数据不可压缩，明确禁掉，避免中间代理浪费时间
        .header("x-content-type-options", "nosniff")
        .header("content-encoding", "identity");
    if !endless {
        // endless 模式不能给 Content-Length，否则客户端会等满 32 GiB
        builder = builder.header(header::CONTENT_LENGTH, total.to_string());
    }
    builder
        .body(Body::from_stream(body_stream))
        .expect("响应构造失败")
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ck_size_defaults_and_clamps() {
        assert_eq!(parse_ck_size(None), 4);
        assert_eq!(parse_ck_size(Some("abc")), 4);
        assert_eq!(parse_ck_size(Some("0")), 4);
        assert_eq!(parse_ck_size(Some("-8")), 4);
        assert_eq!(parse_ck_size(Some("64")), 64);
        assert_eq!(parse_ck_size(Some("2048")), 1024);
        assert_eq!(parse_ck_size(Some("1024")), 1024);
        assert_eq!(parse_ck_size(Some("1.9")), 1);
    }

    #[test]
    fn payload_is_sized_and_random() {
        let p = Payload::generate();
        assert_eq!(p.block().len(), BLOCK_BYTES);
        // 同一进程两次生成不应相同（概率上必然）
        let q = Payload::generate();
        assert_ne!(p.block().as_ref(), q.block().as_ref());
    }
}

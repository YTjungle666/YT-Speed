//! 上传压测客户端：连 `/backend/ws-upload`，推指定 GB 的 1 MiB 帧，然后取回服务端结算。
//!
//! 用法：
//! ```sh
//! cargo run --release --example wsclient -- ws://127.0.0.1:8080/backend/ws-upload 10
//! ```
//!
//! 这个工具存在的意义是把“服务端数到的字节”和“客户端推了多少”分开看：
//! 两者差得多，说明有字节卡在发送缓冲里还没到服务端（PLAN §0.5 的上传口径问题）。

use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "ws://127.0.0.1:8080/backend/ws-upload".into());
    let total_gb: f64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(10.0);

    let (mut ws, _resp) = tokio_tungstenite::connect_async(&url).await?;

    // 服务端第一帧是 token，必须在推数据前拿到
    let token = match ws.next().await {
        Some(Ok(Message::Text(t))) => {
            let v: serde_json::Value = serde_json::from_str(&t)?;
            v["token"].as_str().unwrap_or_default().to_string()
        }
        other => return Err(format!("握手后没收到 token: {other:?}").into()),
    };
    println!("token={token}");

    let frame = vec![0xA5u8; 1024 * 1024];
    let total_bytes = (total_gb * 1024.0 * 1024.0 * 1024.0) as u64;
    let mut sent = 0u64;
    let started = Instant::now();
    while sent < total_bytes {
        ws.send(Message::Binary(frame.clone())).await?;
        sent += frame.len() as u64;
    }
    ws.close(None).await.ok();
    // 等对端把连接关干净，确保结算已落库
    while let Some(Ok(_)) = ws.next().await {}

    let secs = started.elapsed().as_secs_f64();
    println!(
        "客户端推送 {sent} bytes / {secs:.3}s = {:.2} Gbit/s",
        sent as f64 * 8.0 / secs / 1e9
    );

    let base = url
        .replace("ws://", "http://")
        .replace("/backend/ws-upload", "");
    let body = http_get(&format!("{base}/backend/ws-result?token={token}")).await?;
    println!("服务端结算 {body}");
    Ok(())
}

/// 极简 GET：为了不给压测工具引入 reqwest。
async fn http_get(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").ok_or("只支持 http://")?;
    let (host, path) = rest.split_once('/').ok_or("URL 需要带路径")?;
    let mut stream = tokio::net::TcpStream::connect(host).await?;
    let req = format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf).to_string();
    Ok(text.rsplit("\r\n\r\n").next().unwrap_or("").to_string())
}

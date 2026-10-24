# librespeed-rs

LibreSpeed 的 Rust 全栈重写（家庭内网自用）。设计与验收标准见
`~/Plan/speedtest-rs/PLAN.md`（本仓不放计划）。

## 结构

| crate | 作用 |
|---|---|
| `speedtest-core` | 纯逻辑、无 async、无 IO：命令/事件状态机、ramp/grace、带宽换算、ping/jitter、取消 |
| `speedtest-transport` | (P3) wasm 侧 fetch 流式下载 / WebSocket 上传；native 侧 hyper |
| `speedtest-server` | (P2) axum 服务端 |
| `speedtest-cli` | (P6) CLI |

core 的时间一律由调用方传入（`apply(ev, now_ms)`），所以测试不需要 sleep，
也不是“假时钟特例”——生产路径与测试路径是同一份代码。

换算照抄上游 `speedtest_worker.js`（v6.3.1）：
`speed = bytes / secs`、`mbps = speed * 8 * 1.06 / (use_mebibits ? 1048576 : 1000000)`、
下载 grace 1.5 s、上传 grace 3 s、`time_auto` 时每 tick `bonus += min(5*speed/100000, 400)`。

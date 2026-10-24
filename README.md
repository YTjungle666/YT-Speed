# librespeed-rs

LibreSpeed 的 Rust 全栈重写（家庭内网自用）。设计与验收标准见
`~/Plan/speedtest-rs/PLAN.md`（本仓不放计划）。

## 资源上限（硬约束）

服务会长年跑在局域网里，被浏览器、CLI、脚本反复打，所以**任何一个对外接口都不允许
有随输入无限增长的结构**。上限集中在 `crates/speedtest-server/src/limits.rs`，每条都
写了“为什么是这个数”：

| 结构 | 上限 | 超过时 |
|---|---|---|
| WS 单条消息 | 8 MiB（客户端 1 MiB 帧的 8 倍余量） | 连接以 1009 关闭，RSS 不涨 |
| WS 并发连接 | 64 | 新连接直接 503，不排队 |
| 结算表条目 | 1024（10 s 过期） | 挤掉最旧的一条 |
| 静态资产 | 64 MiB | 413；资产整体**流式发送**，不读进内存 |
| 下载响应 | 每次 ≤1024 MiB，流式 + 内核背压 | — |
| 上传 | 逐帧丢弃，只累加长度 | — |

实测（回环）：16 MiB 帧被拒时服务端 RSS 保持 12.4 MiB、`/healthz` 仍 200；
70 路并发 WS 握手得到 64×101 + 6×503。**整个服务不写任何磁盘文件**（没有结果库、
没有临时文件），日志只走 stdout 交给 journald。

`core` 与后续 `transport` 里的采样缓冲都是**定容环形缓冲**，不随测试时长增长。

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

//! 资源上限集中定义。
//!
//! 原则：**任何一个对外接口都不允许有随输入无限增长的结构**。
//! 测速服务是长年跑在局域网里的，被浏览器、CLI、脚本反复打，任何一处“按请求量增长”
//! 的容器都会在几天后被填满，而且这种泄漏在压测里看不出来（压测结束就退出）。
//!
//! 每条上限都写明“为什么是这个数”，改之前先读注释。

/// WS 单条消息上限。浏览器端每帧 1 MiB，留 8 倍余量。
///
/// 不能设成 `usize::MAX`：axum 的 WS 会把一条消息整体聚合成 `Vec<u8>` 再交给我们，
/// 一个声称 10 GB 的帧会直接吃掉 10 GB 内存。超限时连接以 1009 关闭。
pub const MAX_WS_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// WS 同时存在的连接数上限。
///
/// 每一路 WS 连接的内核收发缓冲在万兆下可能各占几百 KB；留 64 路既能跑多客户端
/// 并发测试，又能把最坏情况钉在几十 MB。
pub const MAX_WS_CONNECTIONS: usize = 64;

/// 结算表最多保留多少条。
///
/// 正常一次测速只会产生 1 条（10 s 内被取走）。留 1024 条是为了容忍客户端异常
/// （连上就断、重复握手），超过时挤掉最旧的，表本身永远不涨。
pub const MAX_SETTLEMENTS: usize = 1024;

/// 单个静态资产上限。
///
/// 资产是流式发送的（不整体读进内存），这个值只用来挡“误把镜像放进 web/”这类事故。
pub const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// `stream=1` 单条响应最多发多少（32 GiB）：客户端消失时不会有一条响应无限期挂着。
/// 32 GiB 在 9 Gbit/s 下约 31 s，比任何一轮测试窗口都长。
pub const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024 * 1024;

/// 单条 `stream=1` 响应的**时间**上限。
///
/// 只有字节上限不够：手机测速时锁屏/切后台，页面 JS 被冻结 → 定时器停摆 →
/// 引擎永远走不到 Done → 不会下发 StopAll，客户端那 10 条流一直读。
/// 实测一次遗留 30 条 ESTAB、服务端持续 400 Mbit/s 往外写。
/// 30 s 远大于最长的单阶段窗口（15 s + 1.5 s grace），正常测速碰不到。
pub const MAX_STREAM_SECS: u64 = 30;

/// WS 上传限速时的预留粒度（客户端帧是 1 MiB；这里只是“先预留再读”的初始猜测，
/// 收到第一帧后就会用真实帧长替换）。
pub const WS_FRAME_HINT: usize = 1024 * 1024;

/// 静态资产流式发送时的读缓冲。单连接内存 ≈ 这个值 × 分块数，128 KiB 足够打满万兆。
pub const STREAM_BUFFER_BYTES: usize = 128 * 1024;

// 采样点上限（`MAX_SAMPLES = 2048`）属于 P3 的 WASM 引擎，
// 放在 `speedtest-transport` 里，服务端用不到。

/// 客户端 session 登记表上限。手机端 `AbortController.abort()` 对 in-flight 的
/// 流式响应无效（实测 Android Edge：阶段结束后仍继续收 8 s），所以客户端阶段结束时
/// 改发 `/backend/stop?sid=`，由服务端把这条 session 的流全部结束。
/// 表本身要有界：超过就丢最旧的，条目 60 s 自动过期。
pub const MAX_STOP_SESSIONS: usize = 256;
pub const STOP_SESSION_TTL_SECS: u64 = 60;

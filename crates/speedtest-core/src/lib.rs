//! LibreSpeed 的纯逻辑测速状态机。
//!
//! 这个 crate **不碰网络、不碰 async、不读时钟**，因此：
//! - 同一份调度逻辑可以在浏览器（wasm32）与 CLI（native）里跑；
//! - 时间由调用方作为参数传入（`apply(ev, now_ms)`），测试不需要 sleep，
//!   也不需要专门的“假时钟”分支——生产路径与测试路径是同一份代码。
//!
//! 换算与相位规则照抄上游 `speedtest_worker.js`（v6.3.1）：
//! 下载 grace 1.5 s、上传 grace 3 s、`time_auto` 时每 tick
//! `bonus += min(5 * speed / 100000, 400)`、`mbps = speed * 8 * 1.06 / 1e6`。

pub mod conv;
pub mod engine;
pub mod ping;
pub mod settings;

pub use conv::{to_fixed2, Conv};
pub use engine::{Command, Dir, Engine, Event, Phase, Snapshot, TICK_MS};
pub use ping::PingAcc;
pub use settings::{Settings, TestPlan};

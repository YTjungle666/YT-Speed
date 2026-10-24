//! `speedtest-transport`：core 状态机与真实传输之间的那一层。
//!
//! 平台无关的部分（[`driver`] / [`ring`]）在 native 与 wasm32 上都能编、都能测；
//! 真正碰浏览器 API 的实现放在 `#[cfg(target_arch = "wasm32")]` 的模块里，
//! 这样 90% 的调度逻辑可以在命令行上全速回归，不用开浏览器。
//!
//! 资源边界：采样缓冲是**定容**环形缓冲（[`ring::MAX_SAMPLES`]），
//! 不随测试时长增长；本 crate 不写磁盘、不缓存响应体。

pub mod driver;
pub mod ring;

pub use driver::{Driver, Transport, UiFrame, UI_INTERVAL_MS};
pub use ring::{Sample, SampleRing, MAX_SAMPLES};

//! Leptos 前端（纯 CSR）。
//!
//! 边界：这个 crate **只做显示与交互**。测量状态机在 `speedtest-core`，浏览器传输在
//! `speedtest-wasm`，两者之间由 `web/engine.worker.js` 的 worker 协议连接：
//!
//! ```text
//!  UI(这里)  --postMessage{cmd:init|start|abort}-->  worker  --wasm 调用-->  transport
//!  UI(这里)  <--postMessage{type:frame|done|error}--  worker
//! ```
//!
//! 所以这里没有任何计时、采样、协议代码；换 UI 不会碰到测量口径。

mod components;
mod engine;

use components::{Chart, Gauge, History, ResultPanel, Settings, TopBar};
use leptos::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Route {
    Measure,
    Settings,
}

#[component]
pub fn App() -> impl IntoView {
    let (route, set_route) = signal(Route::Measure);
    // 引擎会话统一在这里创建：切页面不能重建 worker（会丢掉正在跑的测试）
    let session = engine::EngineSession::new();

    view! {
        <main class="app">
            <TopBar route=route set_route=set_route session=session />
            <Show
                when=move || route.get() == Route::Measure
                fallback=move || view! { <Settings session=session /> }
            >
                <section class="measure">
                    <Gauge
                        value=session.readout
                        phase=session.phase
                        peak=10_000.0
                    />
                    <ResultPanel session=session />
                    <History session=session />
                    <Chart samples=session.samples phase=session.phase />
                </section>
            </Show>
        </main>
    }
}

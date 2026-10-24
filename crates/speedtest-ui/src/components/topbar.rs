//! 顶栏：路由切换 + 开始/中止。
//!
//! 按钮状态直接跟 `running` 信号：跑的时候变成「中止」，跑完变回「开始」。

use leptos::prelude::*;

use crate::{engine::EngineSession, Route};

#[component]
pub fn TopBar(
    route: ReadSignal<Route>,
    set_route: WriteSignal<Route>,
    session: EngineSession,
) -> impl IntoView {
    let running = session.running;
    let label = move || {
        if running.get() {
            "中止"
        } else {
            "开始测速"
        }
    };

    view! {
        <header class="topbar">
            <div class="brand">
                <span class="dot"></span>
                "speedtest-rs"
                <span class="ver">"v0.1.0"</span>
            </div>
            <nav class="tabs">
                <button
                    class=move || if route.get() == Route::Measure { "tab on" } else { "tab" }
                    on:click=move |_| set_route.set(Route::Measure)
                >"测量"</button>
                <button
                    class=move || if route.get() == Route::Settings { "tab on" } else { "tab" }
                    on:click=move |_| set_route.set(Route::Settings)
                >"设置"</button>
            </nav>
            <button
                class=move || if running.get() { "primary danger" } else { "primary" }
                disabled=move || !session.is_ready() && !running.get()
                on:click=move |_| {
                    if running.get_untracked() {
                        session.abort();
                    } else {
                        session.start_with_plan();
                    }
                }
            >{label}</button>
        </header>
    }
}

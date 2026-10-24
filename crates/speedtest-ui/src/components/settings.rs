//! 设置页：并发路数、测试时长、是否测上传。
//!
//! 用户明确要求过：可调参数要落在**界面**上，只留环境变量等于没做。
//! 这些值存 localStorage，刷新后还在；服务端地址暂不暴露（同源部署）。

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::engine::{save_plan, EngineSession};

#[component]
pub fn Settings(session: EngineSession) -> impl IntoView {
    let plan = session.plan;
    let set_streams = move |ev| {
        let v = event_value(&ev);
        plan.update(|p| p.streams = (v.round() as u32).clamp(1, 32));
        save_plan(&plan.get_untracked());
    };
    let set_dl = move |ev| {
        let v = event_value(&ev);
        plan.update(|p| p.dl_seconds = v.clamp(2.0, 60.0));
        save_plan(&plan.get_untracked());
    };
    let set_ul = move |ev| {
        let v = event_value(&ev);
        plan.update(|p| p.ul_seconds = v.clamp(2.0, 60.0));
        save_plan(&plan.get_untracked());
    };

    view! {
        <section class="settings">
            <h2>"测量参数"</h2>
            <label>
                <span>"下载并发路数"</span>
                <input
                    type="number" min="1" max="32" step="1"
                    prop:value=move || plan.get().streams.to_string()
                    on:change=set_streams
                />
                <small>"默认 10。浏览器接收侧决定最优值，换机器要重新扫描（见 sweep.html）。"</small>
            </label>
            <label>
                <span>"下载时长（秒）"</span>
                <input
                    type="number" min="2" max="60" step="1"
                    prop:value=move || plan.get().dl_seconds.to_string()
                    on:change=set_dl
                />
                <small>"不含 1.5 s grace；短窗口更容易被起跑阶段影响。"</small>
            </label>
            <label>
                <span>"上传时长（秒）"</span>
                <input
                    type="number" min="2" max="60" step="1"
                    prop:value=move || plan.get().ul_seconds.to_string()
                    on:change=set_ul
                />
                <small>"上传以服务端结算为准；窗口太短会放大连接建立开销。"</small>
            </label>
            <p class="muted">
                "限速由服务端 `--max-download-mbps` / `--max-upload-mbps` 控制，页面只显示实测值。"
            </p>
        </section>
    }
}

fn event_value(ev: &leptos::ev::Event) -> f64 {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .and_then(|el| el.value().parse::<f64>().ok())
        .unwrap_or_default()
}

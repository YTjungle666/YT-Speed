//! 设置页：服务端地址、测试开关、并发路数与时长。

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::engine::{save_plan, EngineSession, MAX_BASE_CHARS};

#[component]
pub fn Settings(session: EngineSession) -> impl IntoView {
    let plan = session.plan;
    let set_base = move |ev| {
        let value = event_text(&ev);
        plan.update(|current| {
            current.base = value.chars().take(MAX_BASE_CHARS).collect();
        });
        save_plan(&plan.get_untracked());
    };
    let set_ping = move |ev| {
        let checked = event_checked(&ev);
        plan.update(|current| current.ping = checked);
        save_plan(&plan.get_untracked());
    };
    let set_upload = move |ev| {
        let checked = event_checked(&ev);
        plan.update(|current| current.upload = checked);
        save_plan(&plan.get_untracked());
    };
    let set_streams = move |ev| {
        let value = event_number(&ev);
        plan.update(|current| current.streams = (value.round() as u32).clamp(1, 32));
        save_plan(&plan.get_untracked());
    };
    let set_dl = move |ev| {
        let value = event_number(&ev);
        plan.update(|current| current.dl_seconds = value.clamp(2.0, 60.0));
        save_plan(&plan.get_untracked());
    };
    let set_ul = move |ev| {
        let value = event_number(&ev);
        plan.update(|current| current.ul_seconds = value.clamp(2.0, 60.0));
        save_plan(&plan.get_untracked());
    };

    view! {
        <section class="settings">
            <h2>"测量参数"</h2>
            <label>
                <span>"服务端地址"</span>
                <input
                    class="server-address"
                    type="url"
                    inputmode="url"
                    autocomplete="url"
                    spellcheck="false"
                    maxlength=MAX_BASE_CHARS.to_string()
                    placeholder="https://10.0.0.2:8080"
                    prop:value=move || plan.get().base
                    on:input=set_base
                />
                <small>"留空表示同源。末尾斜杠会自动去掉。"</small>
            </label>
            <div class="toggle-group">
                <label class="toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || plan.get().ping
                        on:change=set_ping
                    />
                    <span>"测试延迟"</span>
                    <small>"测量 ping 与抖动。"</small>
                </label>
                <label class="toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || plan.get().upload
                        on:change=set_upload
                    />
                    <span>"测试上传"</span>
                    <small>"关闭后只测下载与延迟。"</small>
                </label>
            </div>
            <label>
                <span>"下载并发路数"</span>
                <input
                    type="number" min="1" max="32" step="1"
                    prop:value=move || plan.get().streams.to_string()
                    on:change=set_streams
                />
                <small>"默认 10。最优值由浏览器接收侧决定，换机器/换网卡要自己试几组。"</small>
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

fn event_number(ev: &leptos::ev::Event) -> f64 {
    input_element(ev)
        .and_then(|element| element.value().parse::<f64>().ok())
        .unwrap_or_default()
}

fn event_text(ev: &leptos::ev::Event) -> String {
    input_element(ev)
        .map(|element| element.value())
        .unwrap_or_default()
}

fn event_checked(ev: &leptos::ev::Event) -> bool {
    input_element(ev).is_some_and(|element| element.checked())
}

fn input_element(ev: &leptos::ev::Event) -> Option<web_sys::HtmlInputElement> {
    ev.target()
        .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
}

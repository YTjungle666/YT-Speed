//! 本次结果、上游口径、复制与剪贴板降级。

use leptos::prelude::*;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::{spawn_local, JsFuture};

use crate::components::format::{current_result_text, format_ping, format_rate, upload_mode_label};
use crate::engine::EngineSession;

#[component]
pub fn ResultPanel(session: EngineSession) -> impl IntoView {
    let frame = session.frame;
    let (copy_status, set_copy_status) = signal(None::<String>);
    let (copy_fallback, set_copy_fallback) = signal(None::<String>);

    let down_raw = move || format_rate(frame.get().download_mbps_raw);
    let down_cmp = move || format_rate(frame.get().download_mbps);
    let up_raw = move || format_rate(frame.get().upload_mbps_raw);
    let up_cmp = move || format_rate(frame.get().upload_mbps);
    let ping = move || format_ping(frame.get().ping.as_deref());
    let jitter = move || format_ping(frame.get().jitter.as_deref());
    let mode = move || upload_mode_label(&frame.get().upload_mode);
    let degraded = move || frame.get().upload_mode == "xhr";
    let err = move || session.error.get();
    let can_copy = move || {
        let current = frame.get();
        current.download_mbps_raw.is_some()
            || current.upload_mbps_raw.is_some()
            || current.ping.is_some()
    };

    let copy = move |_| {
        let text = current_result_text(&frame.get_untracked());
        let clipboard_text = text.clone();
        set_copy_fallback.set(None);
        set_copy_status.set(Some("正在复制…".to_string()));
        spawn_local(async move {
            match write_clipboard(&clipboard_text).await {
                Ok(()) => set_copy_status.set(Some("已复制到剪贴板".to_string())),
                Err(_) => {
                    set_copy_status.set(None);
                    set_copy_fallback.set(Some(text));
                }
            }
        });
    };

    view! {
        <section class="result-block" aria-labelledby="result-title">
            <header class="section-header">
                <h2 id="result-title">"本次结果"</h2>
                <button
                    type="button"
                    class="secondary"
                    disabled=move || !can_copy()
                    aria-label="复制本次测速结果"
                    on:click=copy
                >"复制结果"</button>
            </header>
            <p class="copy-status" aria-live="polite">
                {move || copy_status.get().unwrap_or_default()}
            </p>
            <div class="results">
                <div class="card download">
                    <h3>"下载"</h3>
                    <p class="big">{down_raw}<span class="unit">"Mbit/s"</span></p>
                    <p class="sub">"上游口径 " {down_cmp}</p>
                </div>
                <div class="card upload">
                    <h3>"上传"</h3>
                    <p class="big">{up_raw}<span class="unit">"Mbit/s"</span></p>
                    <p class="sub">"上游口径 " {up_cmp}</p>
                    <p class="badge" class:off=move || !degraded()>{mode}</p>
                </div>
                <div class="card latency">
                    <h3>"延迟 / 抖动"</h3>
                    <p class="big">{ping}<span class="unit">"ms"</span></p>
                    <p class="sub">"抖动 " {jitter} " ms"</p>
                </div>
            </div>
            <Show when=move || err().is_some()>
                <div class="card error">
                    <h3>"失败原因"</h3>
                    <p>{move || err().unwrap_or_default()}</p>
                </div>
            </Show>
            <Show when=move || copy_fallback.get().is_some()>
                <label class="copy-fallback">
                    <span>"浏览器拒绝访问剪贴板，请手动选择下面的结果："</span>
                    <textarea
                        rows="7"
                        readonly
                        aria-label="供手动复制的测速结果"
                        prop:value=move || copy_fallback.get().unwrap_or_default()
                    ></textarea>
                </label>
            </Show>
        </section>
    }
}

async fn write_clipboard(text: &str) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("当前环境没有 window"))?;
    let promise = window.navigator().clipboard().write_text(text);
    JsFuture::from(promise).await.map(|_| ())
}

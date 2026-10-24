//! 结果面板：下载 / 上传 / 延迟 / 抖动，两种口径都列。
//!
//! 口径规则（和文档一致）：先给**实测值**，再给上游口径（×1.06）并标明。
//! 上传若走的是 XHR 降级，这里必须显式标出来——否则数字看着一样，含义不同。

use leptos::prelude::*;

use crate::engine::EngineSession;

#[component]
pub fn ResultPanel(session: EngineSession) -> impl IntoView {
    let f = session.frame;
    let fmt = |v: Option<f64>| v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "—".into());

    let down_raw = move || fmt(f.get().download_mbps_raw);
    let down_cmp = move || fmt(f.get().download_mbps);
    let up_raw = move || fmt(f.get().upload_mbps_raw);
    let up_cmp = move || fmt(f.get().upload_mbps);
    let ping = move || f.get().ping.unwrap_or_else(|| "—".into());
    let jitter = move || f.get().jitter.unwrap_or_else(|| "—".into());
    let mode = move || {
        let m = f.get().upload_mode;
        match m.as_str() {
            "ws" => "WS",
            "xhr" => "XHR 降级",
            _ => "—",
        }
    };
    let degraded = move || f.get().upload_mode == "xhr";
    let err = move || session.error.get();

    view! {
        <section class="results">
            <div class="card">
                <h3>"下载"</h3>
                <p class="big">{down_raw}<span class="unit">"Mbit/s"</span></p>
                <p class="sub">"上游口径 " {down_cmp}</p>
            </div>
            <div class="card">
                <h3>"上传"</h3>
                <p class="big">{up_raw}<span class="unit">"Mbit/s"</span></p>
                <p class="sub">"上游口径 " {up_cmp}</p>
                <p class="badge" class:off=move || !degraded()>{mode}</p>
            </div>
            <div class="card small">
                <h3>"延迟 / 抖动"</h3>
                <p class="big">{ping}<span class="unit">"ms"</span></p>
                <p class="sub">"抖动 " {jitter} " ms"</p>
            </div>
            <Show when=move || err().is_some()>
                <div class="card error">
                    <h3>"失败原因"</h3>
                    <p>{move || err().unwrap_or_default()}</p>
                </div>
            </Show>
        </section>
    }
}

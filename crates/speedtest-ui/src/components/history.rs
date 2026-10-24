//! 最近 20 次成功测速；数据上限由 `EngineSession` 中的 `History` 保证。

use leptos::prelude::*;

use crate::components::format::{
    format_ping_number, format_rate, format_timestamp, upload_channel_label,
};
use crate::engine::EngineSession;

#[component]
pub fn History(session: EngineSession) -> impl IntoView {
    let history = session.history;
    let rows = move || {
        let current = history.get();
        if current.is_empty() {
            return "完成一次测速后，结果会保存在本机。".to_string();
        }
        current
            .entries()
            .iter()
            .rev()
            .map(|entry| {
                let channel = upload_channel_label(entry.upload_channel);
                format!(
                    "{}  下载 {} Mbit/s  上传 {} Mbit/s  延迟 {} ms  {}",
                    format_timestamp(entry.timestamp),
                    format_rate(entry.download),
                    format_rate(entry.upload),
                    format_ping_number(entry.ping),
                    channel,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    view! {
        <section class="history" aria-labelledby="history-title">
            <header class="history-header">
                <div>
                    <h2 id="history-title">"最近结果"</h2>
                    <p class="muted">{move || format!("本机保留最近 {} 条", history.get().entries().len())}</p>
                </div>
                <button
                    class="secondary"
                    disabled=move || history.get().is_empty()
                    aria-label="清空测速历史"
                    on:click=move |_| session.clear_history()
                >"清空"</button>
            </header>
            <pre class="history-list">{rows}</pre>
        </section>
    }
}

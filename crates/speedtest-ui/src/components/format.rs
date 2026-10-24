//! 组件共用的数值、阶段和结果文本格式化。

use wasm_bindgen::JsValue;

use crate::engine::{Frame, UploadChannel};

pub fn format_rate(value: Option<f64>) -> String {
    value
        .filter(|number| number.is_finite())
        .map(|number| format!("{number:.2}"))
        .unwrap_or_else(|| "—".to_string())
}

pub fn format_ping(value: Option<&str>) -> String {
    value.unwrap_or("—").to_string()
}

pub fn format_ping_number(value: Option<f64>) -> String {
    value
        .filter(|number| number.is_finite())
        .map(|number| format!("{number:.2}"))
        .unwrap_or_else(|| "—".to_string())
}

pub fn phase_label(phase: &str) -> &'static str {
    match phase {
        "Idle" => "待机",
        "Ping" => "延迟",
        "Download" => "下载",
        "Upload" => "上传",
        "Done" => "完成",
        "Aborted" => "已中止",
        _ => "—",
    }
}

pub fn phase_color_class(phase: &str) -> &'static str {
    match phase {
        "Download" | "Done" => "tone-download",
        "Upload" => "tone-upload",
        _ => "tone-ping",
    }
}

pub fn upload_channel_label(channel: UploadChannel) -> &'static str {
    match channel {
        UploadChannel::Ws => "WS",
        UploadChannel::Xhr => "XHR 降级",
        UploadChannel::None => "未测",
    }
}

pub fn upload_mode_label(mode: &str) -> &'static str {
    match mode {
        "ws" => "WS",
        "xhr" => "XHR 降级",
        _ => "—",
    }
}

/// 本地时间 `YYYY-MM-DD HH:MM:SS`。
///
/// 不能用 `Date::to_iso_string()`：那是 UTC，界面上会显示成比本地早 8 小时的时间，
/// 看起来像记录串了。
pub fn format_timestamp(timestamp: u64) -> String {
    let date = js_sys::Date::new(&JsValue::from_f64(timestamp as f64));
    let pad = |value: u32| format!("{value:02}");
    format!(
        "{}-{}-{} {}:{}:{}",
        date.get_full_year(),
        pad(date.get_month() + 1),
        pad(date.get_date()),
        pad(date.get_hours()),
        pad(date.get_minutes()),
        pad(date.get_seconds()),
    )
}

pub fn current_result_text(frame: &Frame) -> String {
    format!(
        "speedtest-rs\n下载：{} Mbit/s\n上传：{} Mbit/s\n延迟：{} ms\n抖动：{} ms\n上传通道：{}\n时间：{}",
        format_rate(frame.download_mbps_raw),
        format_rate(frame.upload_mbps_raw),
        format_ping(frame.ping.as_deref()),
        format_ping(frame.jitter.as_deref()),
        upload_mode_label(&frame.upload_mode),
        format_timestamp(js_sys::Date::now() as u64),
    )
}

//! Web Worker 侧的 wasm 入口。
//!
//! JS 侧的约定（见 `web/engine.worker.js`）：
//!
//! ```js
//! import init, { start, tick, samples_json, abort } from './pkg/speedtest_wasm.js';
//! await init();
//! start('http://host:8080', JSON.stringify({ ping: true, download: true, upload: true }));
//! setInterval(() => postMessage(tick(performance.now())), 100);
//! ```
//!
//! 为什么是 JS 拉 tick、而不是 Rust 自己起定时器：Worker 里只有一个线程，
//! Rust 侧起 `setInterval` 再回调进来会让借用关系变复杂；由 JS 驱动 100 ms 心跳、
//! Rust 只做纯计算，边界最清楚，也方便在测试里手工喂时间。

use std::cell::RefCell;

use speedtest_core::{Settings, TestPlan};
use speedtest_transport::{Driver, UiFrame};
use wasm_bindgen::prelude::*;

mod transport;

use transport::WasmTransport;

thread_local! {
    static DRIVER: RefCell<Option<Driver<WasmTransport>>> = const { RefCell::new(None) };
}

// `#[serde(default)]` 必须加在结构体上：只在 derive 里写 Default 不会让
// 反序列化用默认值，缺字段会直接报 `missing field`。
// 踩过一次：给 PlanInput 加了新字段 `xhr` 后，所有不带该字段的页面全部失败。
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct PlanInput {
    #[serde(default)]
    ping: bool,
    #[serde(default)]
    download: bool,
    #[serde(default)]
    upload: bool,
    /// 下载并发流数（默认 8，实测最优点）
    streams: Option<u32>,
    /// 下载时长上限（秒）
    dl_seconds: Option<f64>,
    /// 上传时长上限（秒）
    ul_seconds: Option<f64>,
    /// 强制 XHR 上传（降级路径）；默认 false 表示用 WebSocket
    xhr: bool,
}

/// 开始一次测速。`base` 是服务端 origin（如 `http://192.0.2.10:8080`）。
///
/// `now_ms` 必须和 `tick()` 用的时钟是同一个（浏览器里都是 `performance.now()`）。
/// 这里踩过坑：`start` 内部用 `Date::now()`（纪元毫秒）、`tick` 用 `performance.now()`
/// （页面加载后毫秒），两者相差 1.7e12，core 算出的 `t` 永远是负数，于是状态机一直
/// 停在 grace 里、界面永远没有读数。
#[wasm_bindgen]
pub fn start(base: String, plan_json: String, now_ms: f64) -> Result<(), JsValue> {
    let input: PlanInput = if plan_json.trim().is_empty() {
        PlanInput {
            ping: true,
            download: true,
            upload: true,
            ..Default::default()
        }
    } else {
        serde_json::from_str(&plan_json).map_err(|e| JsValue::from_str(&e.to_string()))?
    };

    let mut settings = Settings::default();
    if let Some(n) = input.streams {
        settings.dl_streams = n.clamp(1, 32); // 上限 32：再多也只是多开连接，不会更快
    }
    if let Some(s) = input.dl_seconds {
        settings.dl_max_secs = s.clamp(1.0, 120.0);
    }
    if let Some(s) = input.ul_seconds {
        settings.ul_max_secs = s.clamp(1.0, 120.0);
    }
    let plan = TestPlan {
        ping: input.ping,
        download: input.download,
        upload: input.upload,
    };
    if !(plan.ping || plan.download || plan.upload) {
        return Err(JsValue::from_str("至少要有 ping/download/upload 之一"));
    }

    let mut transport = WasmTransport::new(base);
    if input.xhr {
        transport = transport.force_xhr();
    }
    let mut driver = Driver::new(settings, plan, transport);
    driver.start(now_ms);
    DRIVER.with(|d| *d.borrow_mut() = Some(driver));
    Ok(())
}

/// 100 ms 心跳。返回 UI 帧的 JSON（拿不到新帧时返回 `"null"`）。
#[wasm_bindgen]
pub fn tick(now_ms: f64) -> String {
    DRIVER.with(|d| {
        let mut slot = d.borrow_mut();
        let Some(driver) = slot.as_mut() else {
            return "null".to_string();
        };
        match driver.tick(now_ms) {
            Some(frame) => frame_json(&frame),
            None => "null".to_string(),
        }
    })
}

/// 曲线数据：只回最近的点，长度受 `MAX_SAMPLES` 限制。
#[wasm_bindgen]
pub fn samples_json() -> String {
    DRIVER.with(|d| {
        let slot = d.borrow();
        let Some(driver) = slot.as_ref() else {
            return "[]".to_string();
        };
        let pts: Vec<[f64; 2]> = driver.samples().iter().map(|s| [s.t_ms, s.mbps]).collect();
        serde_json::to_string(&pts).unwrap_or_else(|_| "[]".into())
    })
}

/// 取走最近一次致命错误的原因（页面在 failed=true 时显示，远程机器上开不了 devtools）。
#[wasm_bindgen]
pub fn last_error() -> String {
    transport::take_last_error().unwrap_or_default()
}

#[wasm_bindgen]
pub fn abort(now_ms: f64) {
    DRIVER.with(|d| {
        if let Some(driver) = d.borrow_mut().as_mut() {
            driver.abort(now_ms);
        }
    });
}

fn frame_json(f: &UiFrame) -> String {
    let v = serde_json::json!({
        "phase": format!("{:?}", f.phase),
        "status": f.status,
        "status_raw": f.status_raw,
        "progress": f.progress,
        "bytes": f.bytes,
        "elapsed_ms": f.elapsed_ms,
        "ping": f.ping,
        "jitter": f.jitter,
        "download_mbps": f.download_mbps,
        "upload_mbps": f.upload_mbps,
        "download_mbps_raw": f.download_mbps_raw,
        "upload_mbps_raw": f.upload_mbps_raw,
        "upload_mode": f.upload_mode,
        "failed": f.failed,
    });
    v.to_string()
}

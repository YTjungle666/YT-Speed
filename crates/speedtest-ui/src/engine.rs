//! worker 桥接层：创建 worker、发命令、把回来的消息灌进信号。
//!
//! 为什么不在这里直接调 wasm：测量必须跑在 Worker 线程（主线程要渲染仪表盘和曲线，
//! 实测主线程搬字节比 Worker 低 5~9%，见 PLAN §0.5）。UI 只跟消息打交道。

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, Worker};

/// 一次测试的配置。字段与 `speedtest-wasm` 的 `PlanInput` 对齐（那边有 `#[serde(default)]`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub ping: bool,
    pub download: bool,
    pub upload: bool,
    pub streams: u32,
    pub dl_seconds: f64,
    pub ul_seconds: f64,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            ping: true,
            download: true,
            upload: true,
            streams: 10,
            dl_seconds: 10.0,
            ul_seconds: 8.0,
        }
    }
}

/// 引擎快照。字段与 `frame_json()` 一一对应。
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Frame {
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub status_raw: Option<String>,
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub elapsed_ms: f64,
    #[serde(default)]
    pub ping: Option<String>,
    #[serde(default)]
    pub jitter: Option<String>,
    #[serde(default)]
    pub download_mbps: Option<f64>,
    #[serde(default)]
    pub upload_mbps: Option<f64>,
    #[serde(default)]
    pub download_mbps_raw: Option<f64>,
    #[serde(default)]
    pub upload_mbps_raw: Option<f64>,
    #[serde(default)]
    pub upload_mode: String,
    #[serde(default)]
    pub failed: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct Samples(pub Vec<[f64; 2]>);

/// 页面持有的引擎会话。字段都是 `Copy` 的写信号，组件之间可以直接传。
#[derive(Clone, Copy)]
pub struct EngineSession {
    pub frame: RwSignal<Frame>,
    pub samples: RwSignal<Vec<[f64; 2]>>,
    pub running: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// 当前展示值（原始口径，Mbit/s），仪表盘和曲线都用它
    pub readout: RwSignal<f64>,
    pub phase: RwSignal<String>,
    /// 当前设置（并发/时长），设置页改这里，开始按钮读这里
    pub plan: RwSignal<Plan>,
    worker: StoredValue<Option<Worker>>,
    ready: RwSignal<bool>,
}

impl EngineSession {
    pub fn new() -> Self {
        let frame = RwSignal::new(Frame::default());
        let samples = RwSignal::new(Vec::new());
        let running = RwSignal::new(false);
        let error = RwSignal::new(None::<String>);
        let readout = RwSignal::new(0.0f64);
        let phase = RwSignal::new(String::from("Idle"));
        let plan = RwSignal::new(load_plan());
        let worker = StoredValue::new(None::<Worker>);
        let ready = RwSignal::new(false);

        let session = Self {
            frame,
            samples,
            running,
            error,
            readout,
            phase,
            plan,
            worker,
            ready,
        };
        session.spawn_worker();
        session
    }

    fn spawn_worker(&self) {
        // engine.worker.js 是 ES module（内部 import wasm 绑定），必须用 module worker：
        // 用普通 worker 会静默失败——页面只看到「没有任何响应」，控制台报
        // "Cannot use import statement outside a module"。
        let opts = web_sys::WorkerOptions::new();
        opts.set_type(web_sys::WorkerType::Module);
        let worker = match Worker::new_with_options("/engine.worker.js", &opts) {
            Ok(w) => w,
            Err(e) => {
                self.error.set(Some(format!(
                    "创建 worker 失败：{e:?}（engine.worker.js 是否在 web/ 下？）"
                )));
                return;
            }
        };

        let frame_sig = self.frame;
        let samples_sig = self.samples;
        let running_sig = self.running;
        let error_sig = self.error;
        let readout_sig = self.readout;
        let phase_sig = self.phase;
        let ready_sig = self.ready;

        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            let Some(text) = ev.data().as_string() else {
                // worker 发的都 Object，这里走 serde 反序列化
                let Ok(json) = js_sys::JSON::stringify(&ev.data()) else {
                    return;
                };
                let Some(json) = json.as_string() else { return };
                handle(
                    &json,
                    frame_sig,
                    samples_sig,
                    running_sig,
                    error_sig,
                    readout_sig,
                    phase_sig,
                    ready_sig,
                );
                return;
            };
            handle(
                &text,
                frame_sig,
                samples_sig,
                running_sig,
                error_sig,
                readout_sig,
                phase_sig,
                ready_sig,
            );
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget(); // 页面生命周期内一直需要

        worker
            .post_message(&js_sys::Object::from(
                js_sys::JSON::parse(r#"{"cmd":"init"}"#).unwrap(),
            ))
            .ok();

        self.worker.set_value(Some(worker));
    }

    /// 用当前设置开始一次测试。
    pub fn start_with_plan(&self) {
        let plan = self.plan.get_untracked();
        save_plan(&plan);
        self.start(plan);
    }

    /// 开始一次测试。`base` 是服务端根地址（同源时留空即可）。
    pub fn start(&self, plan: Plan) {
        if self.running.get_untracked() {
            return;
        }
        let Some(worker) = self.worker.get_value() else {
            return;
        };
        self.error.set(None);
        self.samples.set(Vec::new());
        self.readout.set(0.0);
        self.running.set(true);
        self.phase.set("Ping".into());
        let msg = serde_json::json!({
            "cmd": "start",
            "base": "",
            "plan": plan,
        });
        let obj = js_sys::JSON::parse(&msg.to_string()).unwrap();
        worker.post_message(&obj).ok();
    }

    /// 中止正在跑的测试。
    pub fn abort(&self) {
        let Some(worker) = self.worker.get_value() else {
            return;
        };
        let obj = js_sys::JSON::parse(r#"{"cmd":"abort"}"#).unwrap();
        worker.post_message(&obj).ok();
        self.running.set(false);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.get()
    }
}

const PLAN_KEY: &str = "speedtest-rs.plan";

/// 从 localStorage 读回上次的设置（坏数据回退默认值）。
pub fn load_plan() -> Plan {
    let Some(store) = storage() else {
        return Plan::default();
    };
    match store.get_item(PLAN_KEY) {
        Ok(Some(raw)) => serde_json::from_str::<Plan>(&raw).unwrap_or_default(),
        _ => Plan::default(),
    }
}

pub fn save_plan(plan: &Plan) {
    if let Some(store) = storage() {
        if let Ok(s) = serde_json::to_string(plan) {
            let _ = store.set_item(PLAN_KEY, &s);
        }
    }
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

#[allow(clippy::too_many_arguments)]
fn handle(
    json: &str,
    frame_sig: RwSignal<Frame>,
    samples_sig: RwSignal<Vec<[f64; 2]>>,
    running_sig: RwSignal<bool>,
    error_sig: RwSignal<Option<String>>,
    readout_sig: RwSignal<f64>,
    phase_sig: RwSignal<String>,
    ready_sig: RwSignal<bool>,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return;
    };
    let kind = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    match kind {
        "init-done" => ready_sig.set(true),
        "error" => {
            let msg = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误")
                .to_string();
            error_sig.set(Some(msg));
            running_sig.set(false);
        }
        "frame" => {
            if let Some(f) = value
                .get("frame")
                .and_then(|v| serde_json::from_value::<Frame>(v.clone()).ok())
            {
                apply_frame(&f, frame_sig, readout_sig, phase_sig);
            }
        }
        "done" => {
            if let Some(f) = value
                .get("frame")
                .and_then(|v| serde_json::from_value::<Frame>(v.clone()).ok())
            {
                apply_frame(&f, frame_sig, readout_sig, phase_sig);
            }
            if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
                error_sig.set(Some(err.to_string()));
            }
            if let Some(pts) = value
                .get("samples")
                .and_then(|v| serde_json::from_value::<Vec<[f64; 2]>>(v.clone()).ok())
            {
                samples_sig.set(pts);
            } else if let Some(s) = value
                .get("samples")
                .and_then(|v| serde_json::from_value::<Samples>(v.clone()).ok())
            {
                samples_sig.set(s.0);
            }
            running_sig.set(false);
        }
        _ => {}
    }
}

fn apply_frame(
    f: &Frame,
    frame_sig: RwSignal<Frame>,
    readout: RwSignal<f64>,
    phase: RwSignal<String>,
) {
    phase.set(f.phase.clone());
    // 仪表盘显示**原始口径**（×1.06 只在结果面板里另列）。
    // 完成/中止后停在「下载」这个头条数字上，否则会停在上传的最后一次实时值上，
    // 而上传和下载不是一个量级，看起来像结果算错了。
    let stable = match f.phase.as_str() {
        "Done" | "Aborted" | "Idle" => f.download_mbps_raw.or(f.upload_mbps_raw),
        "Upload" => f.upload_mbps_raw,
        _ => None,
    };
    let live = f
        .status_raw
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| *v > 0.0);
    let value = match f.phase.as_str() {
        "Done" | "Aborted" | "Idle" => stable.or(live),
        _ => live.or(stable),
    };
    if let Some(v) = value.filter(|v| v.is_finite() && *v > 0.0) {
        readout.set(v);
    }
    frame_sig.set(f.clone());
}

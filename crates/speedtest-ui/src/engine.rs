//! worker 桥接层：创建 worker、发命令、把回来的消息灌进信号。
//!
//! 为什么不在这里直接调 wasm：测量必须跑在 Worker 线程（主线程要渲染仪表盘和曲线，
//! 实测主线程搬字节比 Worker 低 5~9%，见 PLAN §0.5）。UI 只跟消息打交道。

use std::collections::VecDeque;
use std::fmt;

use leptos::prelude::*;
use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, Worker};

pub const MAX_HISTORY: usize = 20;
pub const MAX_BASE_CHARS: usize = 512;
const MAX_SAMPLES: usize = 2048;

/// 一次测试的配置。字段与 `speedtest-wasm` 的 `PlanInput` 对齐（那边有 `#[serde(default)]`）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub base: String,
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
            base: String::new(),
            ping: true,
            download: true,
            upload: true,
            streams: 10,
            dl_seconds: 10.0,
            ul_seconds: 8.0,
        }
    }
}

impl Plan {
    fn normalized(mut self) -> Self {
        self.base = self.base.trim().trim_end_matches('/').to_string();
        if self.base.chars().count() > MAX_BASE_CHARS {
            self.base = self.base.chars().take(MAX_BASE_CHARS).collect();
        }
        self.streams = self.streams.clamp(1, 32);
        self.dl_seconds = self.dl_seconds.clamp(2.0, 60.0);
        self.ul_seconds = self.ul_seconds.clamp(2.0, 60.0);
        self
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

#[derive(Clone, Debug)]
struct Samples(Vec<[f64; 2]>);

struct SamplesVisitor;

impl<'de> Visitor<'de> for SamplesVisitor {
    type Value = Samples;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded array of [time, Mbps] samples")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut points = VecDeque::with_capacity(MAX_SAMPLES);
        while let Some([time, mbps]) = seq.next_element::<[f64; 2]>()? {
            if !time.is_finite() || !mbps.is_finite() || mbps < 0.0 {
                continue;
            }
            if points.len() == MAX_SAMPLES {
                points.pop_front();
            }
            points.push_back([time, mbps]);
        }
        Ok(Samples(points.into_iter().collect()))
    }
}

impl<'de> Deserialize<'de> for Samples {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SamplesVisitor)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadChannel {
    Ws,
    Xhr,
    #[default]
    None,
}

/// 一条历史记录。序列化字段严格只有这五项。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryEntry {
    pub download: Option<f64>,
    pub upload: Option<f64>,
    pub ping: Option<f64>,
    pub timestamp: u64,
    pub upload_channel: UploadChannel,
}

impl Default for HistoryEntry {
    fn default() -> Self {
        Self {
            download: None,
            upload: None,
            ping: None,
            timestamp: 0,
            upload_channel: UploadChannel::None,
        }
    }
}

impl HistoryEntry {
    fn normalized(mut self) -> Self {
        self.download = finite_non_negative(self.download);
        self.upload = finite_non_negative(self.upload);
        self.ping = finite_non_negative(self.ping);
        self
    }
}

/// 最多 `MAX_HISTORY` 条，按时间从旧到新保存。
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    entries: Vec<HistoryEntry>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_HISTORY),
        }
    }
}

impl History {
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn push(&mut self, entry: HistoryEntry) {
        if self.entries.len() == MAX_HISTORY {
            self.entries.remove(0);
        }
        self.entries.push(entry.normalized());
    }
}

impl Serialize for History {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.entries.serialize(serializer)
    }
}

struct HistoryVisitor;

impl<'de> Visitor<'de> for HistoryVisitor {
    type Value = History;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array with at most 20 speed test results")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut history = History::default();
        while let Some(entry) = seq.next_element::<HistoryEntry>()? {
            history.push(entry);
        }
        Ok(history)
    }
}

impl<'de> Deserialize<'de> for History {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(HistoryVisitor)
    }
}

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
    pub history: RwSignal<History>,
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
        let history = RwSignal::new(load_history());
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
            history,
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
        let session = *self;

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
                    session,
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
                session,
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
        let plan = self.plan.get_untracked().normalized();
        self.plan.set(plan.clone());
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
        let plan = plan.normalized();
        let msg = serde_json::json!({
            "cmd": "start",
            "base": plan.base.clone(),
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

    pub fn clear_history(&self) {
        self.history.set(History::default());
        if let Some(store) = storage() {
            let _ = store.remove_item(HISTORY_KEY);
        }
    }
}

const PLAN_KEY: &str = "speedtest-rs.plan";
const HISTORY_KEY: &str = "speedtest-rs.history";

/// 从 localStorage 读回上次的设置（坏数据回退默认值）。
pub fn load_plan() -> Plan {
    let Some(store) = storage() else {
        return Plan::default();
    };
    match store.get_item(PLAN_KEY) {
        Ok(Some(raw)) => serde_json::from_str::<Plan>(&raw)
            .unwrap_or_default()
            .normalized(),
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

pub fn load_history() -> History {
    let Some(store) = storage() else {
        return History::default();
    };
    match store.get_item(HISTORY_KEY) {
        Ok(Some(raw)) => serde_json::from_str::<History>(&raw).map_or_else(
            |_| History::default(),
            |history| {
                // 旧版本或手工写入的数组也要在读取时立即缩到 20 条，
                // 不能等到下一次测速才让 localStorage 重新满足硬上限。
                save_history(&history);
                history
            },
        ),
        _ => History::default(),
    }
}

fn save_history(history: &History) {
    if let Some(store) = storage() {
        if let Ok(raw) = serde_json::to_string(history) {
            let _ = store.set_item(HISTORY_KEY, &raw);
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
    session: EngineSession,
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
            let was_running = running_sig.get_untracked();
            if let Some(f) = value
                .get("frame")
                .and_then(|v| serde_json::from_value::<Frame>(v.clone()).ok())
            {
                apply_frame(&f, frame_sig, readout_sig, phase_sig);
                let error = value.get("error").and_then(|v| v.as_str());
                if was_running && f.phase == "Done" && !f.failed && error.is_none() {
                    let entry = history_entry(&f);
                    session.history.update(|history| history.push(entry));
                    save_history(&session.history.get_untracked());
                }
            }
            if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
                error_sig.set(Some(err.to_string()));
            }
            if let Some(s) = value
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

fn history_entry(frame: &Frame) -> HistoryEntry {
    let upload_channel = if frame.upload_mbps_raw.is_some() {
        match frame.upload_mode.as_str() {
            "ws" => UploadChannel::Ws,
            "xhr" => UploadChannel::Xhr,
            _ => UploadChannel::None,
        }
    } else {
        UploadChannel::None
    };
    HistoryEntry {
        download: frame.download_mbps_raw,
        upload: frame.upload_mbps_raw,
        ping: frame.ping.as_deref().and_then(|value| value.parse().ok()),
        timestamp: js_sys::Date::now() as u64,
        upload_channel,
    }
}

fn finite_non_negative(value: Option<f64>) -> Option<f64> {
    value.filter(|number| number.is_finite() && *number >= 0.0)
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

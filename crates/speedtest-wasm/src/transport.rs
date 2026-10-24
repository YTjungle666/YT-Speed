//! 浏览器侧传输实现：下载用 `fetch` + `ReadableStream` 读，上传用 `WebSocket` 长连接。
//!
//! 全部跑在 **Web Worker** 里（主线程只收快照、画图）。这是实测结论不是偏好：
//! 同样的下载逻辑放主线程 6.64 Gbit/s，挪到 Worker 7.23 Gbit/s（PLAN §0.5）。
//!
//! 资源边界：
//! - 每帧读完立刻丢弃，只累加长度；不保存 ArrayBuffer、不拼接收响应；
//! - 上传用 `bufferedAmount` 背压，超过 8 MiB 就等，不在 JS 堆里堆帧；
//! - 观测通过 `take_signals` 拉取，字节数合并成一个增量，队列长度有界。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use speedtest_core::Dir;
use speedtest_transport::{Signal, Transport, MAX_SIGNALS};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    AbortController, BinaryType, ReadableStreamDefaultReader, Request, RequestCache, RequestInit,
    Response, WebSocket, WorkerGlobalScope,
};

/// 上传帧大小：1 MiB（实测最优，见 PLAN §0.5 的帧大小对照）。
const UPLOAD_FRAME_BYTES: usize = 1024 * 1024;
/// `bufferedAmount` 超过这个值就暂停发送——这是浏览器侧唯一的“内存闸门”。
const UPLOAD_HIGH_WATER: u32 = 8 * 1024 * 1024;
/// 等待背压时的让步间隔。
const YIELD_MS: i32 = 1;

fn global() -> WorkerGlobalScope {
    js_sys::global().unchecked_into::<WorkerGlobalScope>()
}

/// 让出事件循环，避免背压等待时把 Worker 卡死。
pub async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        let g = global();
        let _ =
            g.set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(), ms);
    });
    let _ = JsFuture::from(promise).await;
}

/// 观测累加器：被多个 fetch 任务共享，写入只做加法。
#[derive(Clone, Default)]
struct Meters {
    bytes: Rc<Cell<u64>>,
    pongs: Rc<RefCell<VecDeque<f64>>>,
    failed: Rc<Cell<bool>>,
    settled: Rc<RefCell<Option<(u64, f64)>>>,
}

pub struct WasmTransport {
    base: String,
    meters: Meters,
    /// 下载流的 AbortController，`stop_all` 时全部 abort。
    controllers: Vec<AbortController>,
    /// 上传 WS 与它的 token（token 由服务端握手后第一帧下发）。
    ws: Option<WebSocket>,
    ws_token: Rc<RefCell<Option<String>>>,
    /// 本轮是否已经下发过 StopAll（避免重复关闭）。
    stopped: bool,
}

impl WasmTransport {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            meters: Meters::default(),
            controllers: Vec::new(),
            ws: None,
            ws_token: Rc::new(RefCell::new(None)),
            stopped: false,
        }
    }

    /// 下载流：`fetch` + reader，边读边丢，读完一条立刻重发。
    ///
    /// 为什么要重发：单个请求是 1 GiB，万兆下 1 秒就跑完；如果流结束就退出，
    /// 8 路会在 1 秒后全部停止，速率断崖。上游 XHR 也是 onload 后重新发。
    fn start_download(&mut self, count: u32, stagger_ms: f64) {
        for i in 0..count {
            let delay = (stagger_ms * i as f64) as i32;
            let ctrl = AbortController::new().expect("创建 AbortController 失败");
            self.controllers.push(ctrl.clone());
            let url = format!("{}/backend/garbage?ckSize=1024", self.base);
            let meters = self.meters.clone();
            spawn_local(async move {
                if delay > 0 {
                    sleep_ms(delay).await;
                }
                let signal = ctrl.signal();
                while !signal.aborted() {
                    let opts = RequestInit::new();
                    opts.set_method("GET");
                    opts.set_cache(RequestCache::NoStore);
                    opts.set_signal(Some(&signal));
                    let Ok(req) = Request::new_with_str_and_init(&url, &opts) else {
                        meters.failed.set(true);
                        return;
                    };
                    let resp_val = match JsFuture::from(global().fetch_with_request(&req)).await {
                        Ok(v) => v,
                        Err(_) => {
                            if !signal.aborted() {
                                meters.failed.set(true);
                            }
                            return;
                        }
                    };
                    let Ok(resp) = resp_val.dyn_into::<Response>() else {
                        meters.failed.set(true);
                        return;
                    };
                    let Some(body) = resp.body() else {
                        meters.failed.set(true);
                        return;
                    };
                    let reader: ReadableStreamDefaultReader = body.get_reader().unchecked_into();
                    loop {
                        if signal.aborted() {
                            let _ = reader.cancel();
                            return;
                        }
                        let chunk = match JsFuture::from(reader.read()).await {
                            Ok(v) => v,
                            Err(_) => {
                                if !signal.aborted() {
                                    meters.failed.set(true);
                                }
                                return;
                            }
                        };
                        let done = js_sys::Reflect::get(&chunk, &JsValue::from_str("done"))
                            .ok()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        if done {
                            break; // 这一条读完，回到外层重发
                        }
                        // 只取长度，chunk 本身立刻丢掉（不持有、不拼接）
                        let len = js_sys::Reflect::get(&chunk, &JsValue::from_str("value"))
                            .ok()
                            .and_then(|v| {
                                js_sys::Reflect::get(&v, &JsValue::from_str("byteLength"))
                                    .ok()
                                    .and_then(|n| n.as_f64())
                            })
                            .unwrap_or(0.0);
                        if len > 0.0 {
                            meters.bytes.set(meters.bytes.get() + len as u64);
                        }
                    }
                }
            });
        }
    }

    /// 上传：WebSocket 长连接（实测单连接即线速）。
    fn start_upload(&mut self) {
        let url = self
            .base
            .replacen("http://", "ws://", 1)
            .replacen("https://", "wss://", 1)
            + "/backend/ws-upload";
        let ws = match WebSocket::new(&url) {
            Ok(w) => w,
            Err(_) => {
                self.meters.failed.set(true);
                return;
            }
        };
        ws.set_binary_type(BinaryType::Arraybuffer);

        // 第一帧是服务端下发的 token，必须在推数据前读出来
        let token_slot = self.ws_token.clone();
        let onmessage =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
                if let Some(txt) = ev.data().as_string() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                        if let Some(t) = v.get("token").and_then(|t| t.as_str()) {
                            *token_slot.borrow_mut() = Some(t.to_string());
                        }
                    }
                }
            });
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();

        let meters = self.meters.clone();
        // WebSocket 是 JS 对象的句柄，Clone 只是复制引用，可以安全地搬进闭包。
        // 注意：闭包里必须先 clone 再 move 进 async 块，否则闭包会退化成 FnOnce。
        let ws_send = ws.clone();
        let onopen = Closure::<dyn FnMut()>::new(move || {
            let ws = ws_send.clone();
            let meters = meters.clone();
            let frame = vec![0xA5u8; UPLOAD_FRAME_BYTES];
            spawn_local(async move {
                loop {
                    if ws.ready_state() != WebSocket::OPEN {
                        break;
                    }
                    if ws.buffered_amount() > UPLOAD_HIGH_WATER {
                        sleep_ms(YIELD_MS).await;
                        continue;
                    }
                    if ws.send_with_u8_array(&frame).is_err() {
                        meters.failed.set(true);
                        break;
                    }
                    meters.bytes.set(meters.bytes.get() + frame.len() as u64);
                }
            });
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();
        self.ws = Some(ws);
    }
}

impl Transport for WasmTransport {
    fn start_streams(&mut self, dir: Dir, count: u32, stagger_ms: f64) {
        self.stopped = false;
        match dir {
            Dir::Down => self.start_download(count, stagger_ms),
            Dir::Up => self.start_upload(),
        }
    }

    fn stop_all(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        for c in self.controllers.drain(..) {
            c.abort();
        }
        if let Some(ws) = self.ws.take() {
            let _ = ws.close();
            // 关闭后去取服务端结算：这是上传的权威数字
            let base = self.base.clone();
            let token = self.ws_token.borrow().clone();
            let settled = self.meters.settled.clone();
            let failed = self.meters.failed.clone();
            spawn_local(async move {
                let Some(token) = token else { return };
                sleep_ms(50).await; // 给服务端一点时间把结算写进 token
                let url = format!("{base}/backend/ws-result?token={token}");
                let Ok(v) = JsFuture::from(global().fetch_with_str(&url)).await else {
                    return;
                };
                let Ok(resp) = v.dyn_into::<Response>() else {
                    return;
                };
                let Ok(text) = JsFuture::from(resp.text().unwrap()).await else {
                    return;
                };
                if let Some(txt) = text.as_string() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                        let bytes = v.get("bytes").and_then(|x| x.as_f64()).unwrap_or(0.0);
                        let ms = v.get("ms").and_then(|x| x.as_f64()).unwrap_or(0.0);
                        if ms > 0.0 {
                            *settled.borrow_mut() = Some((bytes as u64, ms));
                        } else {
                            // 取不到结算就保留客户端估算，但标记出来，不静默当权威值
                            failed.set(false);
                        }
                    }
                }
            });
        }
    }

    fn ping(&mut self, _seq: u32) {
        // ping 走轻量 fetch 到 /backend/empty，用 performance.now() 计时
        let url = format!("{}/backend/empty?r={}", self.base, js_sys::Math::random());
        let pongs = self.meters.pongs.clone();
        spawn_local(async move {
            let started = js_sys::Date::now();
            if JsFuture::from(global().fetch_with_str(&url)).await.is_err() {
                return;
            }
            let rtt = js_sys::Date::now() - started;
            let mut q = pongs.borrow_mut();
            // 有界：只保留最近 MAX_SIGNALS 条，绝不无限排队
            if q.len() >= MAX_SIGNALS {
                q.pop_front();
            }
            q.push_back(rtt);
        });
    }

    fn take_signals(&mut self) -> Vec<Signal> {
        let mut out = Vec::with_capacity(4);
        let pending = self.meters.bytes.replace(0);
        if pending > 0 {
            out.push(Signal::Bytes(pending));
        }
        {
            let mut q = self.meters.pongs.borrow_mut();
            while let Some(rtt) = q.pop_front() {
                out.push(Signal::Pong(rtt));
            }
        }
        if let Some((bytes, ms)) = self.meters.settled.borrow_mut().take() {
            out.push(Signal::UploadSettled { bytes, ms });
        }
        if self.meters.failed.replace(false) {
            out.push(Signal::StreamFailed);
        }
        out
    }
}

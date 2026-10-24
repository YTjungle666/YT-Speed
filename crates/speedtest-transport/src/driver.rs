//! 驱动层：把 `speedtest-core` 的纯状态机接到具体传输上。
//!
//! 分工：
//! - `speedtest-core`：什么时候该干什么（相位、grace、换算、取消）——纯逻辑；
//! - 本模块：把 core 的 `Command` 翻译成传输层的动作，把传输层的观测
//!   （收到多少字节、pong 多久、服务端结算）翻译成 core 的 `Event`，
//!   并维护有界的采样缓冲与 10 Hz 的 UI 快照；
//! - `Transport` 实现：真正碰网络的地方（wasm 用 fetch/WebSocket，native 用于测试）。
//!
//! 这个文件本身不碰 IO，所以能在 native 上全速跑单测——浏览器里跑的那套逻辑
//! 和测试里跑的是同一份。

use speedtest_core::{Command, Dir, Engine, Event, Phase, Settings, TestPlan};

use crate::ring::{Sample, SampleRing};

/// 传输层需要实现的三个动作。故意做得极小，方便在 wasm 和测试里各写一份。
pub trait Transport {
    /// 开传输流；第 i 条流延迟 `i * stagger_ms` 起跑。
    fn start_streams(&mut self, dir: Dir, count: u32, stagger_ms: f64);
    /// 停掉所有流（含取消）。
    fn stop_all(&mut self);
    /// 发一次 ping。
    fn ping(&mut self, seq: u32);
}

/// 发给 UI 的一帧快照。字段都是定长的，直接 postMessage 也行。
#[derive(Clone, Debug, PartialEq)]
pub struct UiFrame {
    pub phase: Phase,
    pub status: Option<String>,
    pub progress: f64,
    pub bytes: u64,
    pub elapsed_ms: f64,
    pub ping: Option<String>,
    pub jitter: Option<String>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub failed: bool,
}

/// UI 上报间隔：10 Hz 够画曲线，又不会把主线程刷爆。
pub const UI_INTERVAL_MS: f64 = 100.0;

pub struct Driver<T: Transport> {
    engine: Engine,
    transport: T,
    ring: SampleRing,
    started_ms: Option<f64>,
    last_ui_ms: f64,
}

impl<T: Transport> Driver<T> {
    pub fn new(settings: Settings, plan: TestPlan, transport: T) -> Self {
        Self {
            engine: Engine::new(settings, plan),
            transport,
            ring: SampleRing::new(),
            started_ms: None,
            last_ui_ms: f64::NEG_INFINITY,
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn snapshot(&self) -> &speedtest_core::Snapshot {
        self.engine.snapshot()
    }

    pub fn samples(&self) -> &SampleRing {
        &self.ring
    }

    /// 开始测试。返回 false 表示已经在跑。
    pub fn start(&mut self, now_ms: f64) -> bool {
        if self.engine.phase() != Phase::Idle {
            return false;
        }
        self.started_ms = Some(now_ms);
        self.pump(Event::Start, now_ms);
        true
    }

    /// 传输层报告新增字节（下载或上传，按当前相位归属）。
    pub fn on_bytes(&mut self, n: u64, now_ms: f64) {
        self.pump(Event::Bytes(n), now_ms);
        self.maybe_sample(now_ms);
    }

    /// 一次 ping 回包。
    pub fn on_pong(&mut self, rtt_ms: f64, now_ms: f64) {
        self.pump(Event::Pong(rtt_ms), now_ms);
    }

    /// 任一传输流失败。
    pub fn on_stream_failed(&mut self, now_ms: f64) {
        self.pump(Event::StreamFailed, now_ms);
    }

    /// 上传结束时的服务端权威结算。
    pub fn on_upload_settled(&mut self, bytes: u64, ms: f64, now_ms: f64) {
        self.pump(Event::UploadSettled { bytes, ms }, now_ms);
    }

    /// 用户取消。
    pub fn abort(&mut self, now_ms: f64) {
        self.pump(Event::Abort, now_ms);
    }

    /// 每 200 ms 调一次（对应 core 的节拍）。返回是否需要给 UI 发新帧。
    pub fn tick(&mut self, now_ms: f64) -> Option<UiFrame> {
        self.pump(Event::Tick, now_ms);
        self.maybe_sample(now_ms);
        if now_ms - self.last_ui_ms >= UI_INTERVAL_MS {
            self.last_ui_ms = now_ms;
            Some(self.ui_frame())
        } else {
            None
        }
    }

    pub fn ui_frame(&self) -> UiFrame {
        let s = self.engine.snapshot();
        UiFrame {
            phase: s.phase,
            status: s.status.clone(),
            progress: s.progress,
            bytes: s.bytes,
            elapsed_ms: s.elapsed_ms,
            ping: s.ping_status.clone(),
            jitter: s.jitter_status.clone(),
            download_mbps: s.download_mbps,
            upload_mbps: s.upload_mbps,
            failed: s.failed,
        }
    }

    /// 送一个事件给 core，并把返回的命令立刻执行掉。
    fn pump(&mut self, ev: Event, now_ms: f64) {
        let cmds = self.engine.apply(ev, now_ms);
        self.dispatch(cmds);
    }

    fn dispatch(&mut self, cmds: Vec<Command>) {
        for cmd in cmds {
            match cmd {
                Command::Ping { seq } => self.transport.ping(seq),
                Command::StartStreams {
                    dir,
                    count,
                    stagger_ms,
                } => self.transport.start_streams(dir, count, stagger_ms),
                Command::StopAll => self.transport.stop_all(),
            }
        }
    }

    /// 在传输阶段按 10 Hz 落一个采样点。点数是定容的（`MAX_SAMPLES`）。
    fn maybe_sample(&mut self, now_ms: f64) {
        let Some(started) = self.started_ms else {
            return;
        };
        let s = self.engine.snapshot();
        if !matches!(s.phase, Phase::Download | Phase::Upload) {
            return;
        }
        let t = now_ms - started;
        if let Some(last) = self.ring.last() {
            if t - last.t_ms < UI_INTERVAL_MS {
                return;
            }
        }
        // 运行中的速率不能靠反解 status 字符串（丢精度），直接用字节/时间算；
        // 换算常数与 core 一致（×1.06、十进兆），换单位时两边要一起改。
        let secs = s.elapsed_ms / 1000.0;
        let mbps = if secs > 0.0 {
            s.bytes as f64 / secs * 8.0 * 1.06 / 1_000_000.0
        } else {
            0.0
        };
        self.ring.push(Sample {
            t_ms: t,
            mbps,
            bytes: s.bytes,
        });
    }
}

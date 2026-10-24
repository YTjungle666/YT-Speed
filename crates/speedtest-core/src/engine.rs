//! 命令/事件状态机。
//!
//! 引擎是**纯函数式**的：所有输入都是 `apply(Event, now_ms)`，所有输出都是
//! `Vec<Command>`，没有任何 await、回调、定时器。调用方负责：
//! - 每 `TICK_MS` 发一次 `Tick`；
//! - 收到传输层字节数时发 `Bytes(n)`；
//! - 执行返回的 `Command`（开流、停流、发 ping）。
//!
//! 相位与计时规则对齐上游 `dlTest()` / `ulTest()` / `pingTest()`：
//!
//! ```text
//! Idle --Start--> Ping --ping_count 个 pong--> Download --时间到--> Upload --时间到--> Done
//!                              \__ Abort --> Aborted （任何相位）
//! ```
//!
//! grace 的语义（容易搞错，按上游逐行实现）：
//! `t = now - start_ms`，`t > grace` 时若已收到字节，则把 `start_ms` 重置到当前时刻、
//! 清零已计字节与 bonus；**若一个字节都没收到就不重置**（极慢链路否则会被清成 0）。
//! 重置发生的那个 tick 不计算速率。

use crate::conv::{to_fixed2, Conv};
use crate::ping::PingAcc;
use crate::settings::{Settings, TestPlan};

/// 状态机节拍，与上游 `setInterval(..., 200)` 一致。
pub const TICK_MS: u64 = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Ping,
    Download,
    Upload,
    Done,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Down,
    Up,
}

/// 引擎要求调用方执行的动作。
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// 发一次 ping 请求（`seq` 从 0 起，回包时带回来）。
    Ping { seq: u32 },
    /// 开传输流；`count` 是并发数，第 i 条流延迟 `i * stagger_ms` 起跑。
    StartStreams {
        dir: Dir,
        count: u32,
        stagger_ms: f64,
    },
    /// 停止所有流（含 abort、阶段切换、测试结束）。
    StopAll,
}

/// 外部输入的观测事件。
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Start,
    /// 每 200 ms 一次。
    Tick,
    /// 本 tick 新增的字节数（下载或上传，按当前相位归属）。
    Bytes(u64),
    /// 一次 ping 的回包，单位毫秒。
    Pong(f64),
    /// 任一传输流失败：按上游语义，失败会让本阶段立即结束并标 `Fail`。
    StreamFailed,
    /// 用户取消。
    Abort,
    /// 上传阶段结束时服务端给出的权威结算（字节数、毫秒）。最终上传值以它为准。
    UploadSettled {
        bytes: u64,
        ms: f64,
    },
}

/// 对外的一次状态快照。`*_status` 是上游那种两位小数字符串。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    pub status: Option<String>,
    pub progress: f64,
    pub bytes: u64,
    pub elapsed_ms: f64,
    pub ping_status: Option<String>,
    pub jitter_status: Option<String>,
    /// 最终结果（阶段结束时定稿），单位 Mbit/s。
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    /// 任一阶段因流失败而结束。
    pub failed: bool,
}

#[derive(Clone, Copy, Debug)]
struct Transfer {
    dir: Dir,
    start_ms: f64,
    bonus_ms: f64,
    grace_done: bool,
    total: u64,
    failed: bool,
    elapsed_ms: f64,
    status: Option<f64>,
}

impl Transfer {
    fn new(dir: Dir, now_ms: f64) -> Self {
        Self {
            dir,
            start_ms: now_ms,
            bonus_ms: 0.0,
            grace_done: false,
            total: 0,
            failed: false,
            elapsed_ms: 0.0,
            status: None,
        }
    }
}

pub struct Engine {
    settings: Settings,
    plan: TestPlan,
    conv: Conv,
    phase: Phase,
    ping: PingAcc,
    ping_seq: u32,
    transfer: Option<Transfer>,
    /// 服务端结算，供上传阶段定稿用。
    settled: Option<(u64, f64)>,
    snapshot: Snapshot,
}

impl Engine {
    pub fn new(settings: Settings, plan: TestPlan) -> Self {
        let conv = settings.conv;
        Self {
            settings,
            plan,
            conv,
            phase: Phase::Idle,
            ping: PingAcc::default(),
            ping_seq: 0,
            transfer: None,
            settled: None,
            snapshot: Snapshot::default(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn ping_acc(&self) -> &PingAcc {
        &self.ping
    }

    /// 状态机的唯一入口。返回需要调用方执行的动作（可能为空）。
    pub fn apply(&mut self, ev: Event, now_ms: f64) -> Vec<Command> {
        let mut cmds = Vec::new();
        match ev {
            Event::Start => {
                if self.phase != Phase::Idle {
                    return cmds;
                }
                if self.plan.ping {
                    self.phase = Phase::Ping;
                    self.ping_seq = 0;
                    cmds.push(Command::Ping { seq: 0 });
                } else {
                    cmds.extend(self.begin_next_phase(now_ms));
                }
            }
            Event::Pong(rtt) => {
                if self.phase != Phase::Ping {
                    return cmds;
                }
                self.ping.on_pong(rtt);
                if self.ping.samples >= self.settings.ping_count {
                    cmds.extend(self.begin_next_phase(now_ms));
                } else {
                    self.ping_seq += 1;
                    cmds.push(Command::Ping { seq: self.ping_seq });
                }
            }
            Event::Bytes(n) => {
                if let Some(t) = self.transfer.as_mut() {
                    t.total = t.total.saturating_add(n);
                }
            }
            Event::StreamFailed => {
                if let Some(t) = self.transfer.as_mut() {
                    t.failed = true;
                }
            }
            Event::Abort => {
                if matches!(self.phase, Phase::Done | Phase::Aborted) {
                    return cmds;
                }
                self.phase = Phase::Aborted;
                self.transfer = None;
                cmds.push(Command::StopAll);
            }
            Event::UploadSettled { bytes, ms } => {
                self.settled = Some((bytes, ms));
            }
            Event::Tick => {
                if matches!(self.phase, Phase::Download | Phase::Upload) {
                    cmds.extend(self.on_tick(now_ms));
                }
            }
        }
        self.refresh_snapshot();
        cmds
    }

    /// 进入下一个阶段；返回需要执行的动作。
    fn begin_next_phase(&mut self, now_ms: f64) -> Vec<Command> {
        let mut cmds = Vec::new();
        match self.phase {
            Phase::Idle | Phase::Ping if self.plan.download => {
                self.phase = Phase::Download;
                self.transfer = Some(Transfer::new(Dir::Down, now_ms));
                cmds.push(Command::StartStreams {
                    dir: Dir::Down,
                    count: self.settings.dl_streams,
                    stagger_ms: self.settings.stream_stagger_ms,
                });
            }
            Phase::Idle | Phase::Ping | Phase::Download if self.plan.upload => {
                self.phase = Phase::Upload;
                self.settled = None;
                self.transfer = Some(Transfer::new(Dir::Up, now_ms));
                cmds.push(Command::StartStreams {
                    dir: Dir::Up,
                    count: self.settings.ul_streams,
                    stagger_ms: self.settings.stream_stagger_ms,
                });
            }
            _ => {
                self.phase = Phase::Done;
                self.transfer = None;
                cmds.push(Command::StopAll);
            }
        }
        cmds
    }

    fn on_tick(&mut self, now_ms: f64) -> Vec<Command> {
        let mut cmds = Vec::new();
        let (grace_secs, max_secs) = match self.phase {
            Phase::Download => (self.settings.dl_grace_secs, self.settings.dl_max_secs),
            _ => (self.settings.ul_grace_secs, self.settings.ul_max_secs),
        };
        let auto = self.settings.time_auto;

        let t = {
            let tr = self.transfer.as_mut().expect("传输阶段必有累加器");
            let t = now_ms - tr.start_ms;
            if t < TICK_MS as f64 {
                return cmds;
            }
            if !tr.grace_done {
                if t > 1000.0 * grace_secs {
                    // 上游：只有已经收到字节才重置基准；极慢链路保持原基准，
                    // 否则会把“慢”误算成“快”。
                    if tr.total > 0 {
                        tr.start_ms = now_ms;
                        tr.bonus_ms = 0.0;
                        tr.total = 0;
                    }
                    tr.grace_done = true;
                }
                return cmds;
            }
            let speed = tr.total as f64 / (t / 1000.0);
            if auto {
                let bonus = 5.0 * speed / 100_000.0;
                tr.bonus_ms += if bonus > 400.0 { 400.0 } else { bonus };
            }
            tr.elapsed_ms = t;
            tr.status = Some(self.conv.mbps(tr.total as f64, t / 1000.0));
            t
        };

        let (done, failed) = {
            let tr = self.transfer.as_ref().expect("传输阶段必有累加器");
            ((t + tr.bonus_ms) / 1000.0 > max_secs, tr.failed)
        };
        if done || failed {
            cmds.extend(self.finish_transfer(now_ms));
        }
        cmds
    }

    /// 结束当前传输阶段并决定下一步。
    fn finish_transfer(&mut self, now_ms: f64) -> Vec<Command> {
        let mut cmds = vec![Command::StopAll];
        let tr = self.transfer.take().expect("传输阶段必有累加器");

        // 上传以服务端结算为准（客户端 bufferedAmount 尚未离机的部分不算数）
        let value = match (tr.dir, self.settled) {
            (Dir::Up, Some((bytes, ms))) if ms > 0.0 => {
                Some(self.conv.mbps(bytes as f64, ms / 1000.0))
            }
            _ => tr.status,
        };

        match tr.dir {
            Dir::Down => {
                self.snapshot.download_mbps = value;
                self.snapshot.failed |= tr.failed;
            }
            Dir::Up => {
                self.snapshot.upload_mbps = value;
                self.snapshot.failed |= tr.failed;
            }
        }

        let next = match (tr.dir, self.phase) {
            (Dir::Down, _) if self.plan.upload => {
                self.phase = Phase::Upload;
                self.settled = None;
                Some(Transfer::new(Dir::Up, now_ms))
            }
            _ => None,
        };

        match next {
            Some(t) => {
                let dir = t.dir;
                self.transfer = Some(t);
                cmds.push(Command::StartStreams {
                    dir,
                    count: self.settings.ul_streams,
                    stagger_ms: self.settings.stream_stagger_ms,
                });
            }
            None => {
                self.phase = Phase::Done;
            }
        }
        cmds
    }

    fn refresh_snapshot(&mut self) {
        self.snapshot.phase = self.phase;
        if let Some(tr) = self.transfer.as_ref() {
            self.snapshot.bytes = tr.total;
            self.snapshot.elapsed_ms = tr.elapsed_ms;
            self.snapshot.status = tr.status.map(to_fixed2);
            let max_secs = match tr.dir {
                Dir::Down => self.settings.dl_max_secs,
                Dir::Up => self.settings.ul_max_secs,
            };
            let p = if tr.grace_done {
                (tr.elapsed_ms + tr.bonus_ms) / (max_secs * 1000.0)
            } else {
                0.0
            };
            self.snapshot.progress = p.clamp(0.0, 1.0);
        } else if matches!(self.phase, Phase::Done | Phase::Aborted) {
            self.snapshot.progress = 1.0;
        }
        if self.ping.samples > 1 {
            self.snapshot.ping_status = Some(self.ping.ping_status());
            self.snapshot.jitter_status = Some(self.ping.jitter_status());
        }
    }
}

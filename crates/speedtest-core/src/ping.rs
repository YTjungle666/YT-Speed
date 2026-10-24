//! ping / jitter 累加器，规则逐条对齐上游 `pingTest()`。
//!
//! 上游的几个非直觉点（全部保留，因为要“与 JS 一致”）：
//! - 第一个 pong 只用来对齐时间基准，不参与 ping/jitter；
//! - `instspd < 1` 时先退回上一次的值，再退到 1（浏览器计时粗化会出现 0 ms）；
//! - ping 取“所有样本里的最小值”，不是平均；
//! - jitter 是加权平均，突增比回落权重高（0.7 / 0.3 对 0.2 / 0.8），
//!   且第 2 个样本的 jitter 直接采用（因为第一个差值通常偏大）。

/// ping/jitter 的累加状态。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PingAcc {
    /// 当前 ping（最小样本），毫秒。
    pub ping: f64,
    /// 当前 jitter（加权平均），毫秒。
    pub jitter: f64,
    /// 已收到的 pong 数。
    pub samples: u32,
    prev_inst: f64,
}

impl PingAcc {
    /// 收到一个 pong。`rtt_ms` 为本次往返时间。
    ///
    /// 返回 `true` 表示这是第一个 pong，因此还没有有效读数（上游此时
    /// `pingStatus` 仍是 `0.00`）。
    pub fn on_pong(&mut self, rtt_ms: f64) -> bool {
        let first = self.samples == 0;
        if first {
            self.samples = 1;
            return true;
        }

        let mut inst = rtt_ms;
        if inst < 1.0 {
            inst = self.prev_inst;
        }
        if inst < 1.0 {
            inst = 1.0;
        }
        let inst_jitter = (inst - self.prev_inst).abs();

        if self.samples == 1 {
            self.ping = inst;
        } else {
            if inst < self.ping {
                self.ping = inst;
            }
            if self.samples == 2 {
                self.jitter = inst_jitter;
            } else if inst_jitter > self.jitter {
                self.jitter = self.jitter * 0.3 + inst_jitter * 0.7;
            } else {
                self.jitter = self.jitter * 0.8 + inst_jitter * 0.2;
            }
        }
        self.prev_inst = inst;
        self.samples += 1;
        false
    }

    /// ping 的两位小数字符串（上游 `pingStatus`）。
    pub fn ping_status(&self) -> String {
        crate::conv::to_fixed2(self.ping)
    }

    /// jitter 的两位小数字符串（上游 `jitterStatus`）。
    pub fn jitter_status(&self) -> String {
        crate::conv::to_fixed2(self.jitter)
    }
}

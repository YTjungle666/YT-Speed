//! 定容采样环形缓冲。
//!
//! 为什么不用 `Vec::push` 一直接：15 s 测试按 10 Hz 采样是 150 点，看着无害，
//! 但 P5 的稳定测试要跑 30 分钟（18000 点），而且页面可能被开着几小时。
//! 这里用**预分配 + 覆盖最旧**的方式，`capacity` 一旦定了就永不变，
//! `len()` 最多等于 `capacity`，内存占用是常量。
//!
//! 曲线只需要最近一段窗口，被覆盖掉的旧点本来就画不出来。

/// 单个采样点。刻意只用 24 字节（f64 + f64 + u64），2048 点是 48 KiB。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// 相对测试开始的毫秒数。
    pub t_ms: f64,
    /// 该时刻的速率（Mbit/s，已乘开销补偿）。
    pub mbps: f64,
    /// 累计字节数。
    pub bytes: u64,
}

/// 容量上限：2048 点 = 10 Hz 下 3.4 分钟，足够画满 15 s 测试与 30 分钟稳定测试的最近窗口。
pub const MAX_SAMPLES: usize = 2048;

pub struct SampleRing {
    buf: Vec<Sample>,
    /// 下一个写入位置（环形）。
    head: usize,
    /// 有效点数，最多等于容量。
    len: usize,
}

impl Default for SampleRing {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleRing {
    /// 预分配满容量，之后**不再分配**。
    pub fn new() -> Self {
        let mut buf = Vec::with_capacity(MAX_SAMPLES);
        buf.resize(MAX_SAMPLES, Sample::default());
        Self {
            buf,
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        MAX_SAMPLES
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 写入一个点；满了就覆盖最旧的。永不分配。
    pub fn push(&mut self, s: Sample) {
        self.buf[self.head] = s;
        self.head = (self.head + 1) % MAX_SAMPLES;
        if self.len < MAX_SAMPLES {
            self.len += 1;
        }
    }

    /// 从旧到新遍历。
    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        let start = if self.len == MAX_SAMPLES {
            self.head
        } else {
            0
        };
        (0..self.len).map(move |i| &self.buf[(start + i) % MAX_SAMPLES])
    }

    /// 只取 `t_ms` 之后的点（给 UI 增量拉取用）。
    pub fn since(&self, t_ms: f64) -> impl Iterator<Item = &Sample> {
        self.iter().filter(move |s| s.t_ms >= t_ms)
    }

    /// 最近一个点。
    pub fn last(&self) -> Option<&Sample> {
        if self.len == 0 {
            None
        } else {
            Some(&self.buf[(self.head + MAX_SAMPLES - 1) % MAX_SAMPLES])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(i: usize) -> Sample {
        Sample {
            t_ms: i as f64,
            mbps: i as f64,
            bytes: i as u64,
        }
    }

    #[test]
    fn capacity_is_fixed_and_never_grows() {
        let mut r = SampleRing::new();
        let cap = r.capacity();
        for i in 0..(cap * 5) {
            r.push(s(i));
        }
        assert_eq!(r.len(), cap, "满容量后 len 应停在 cap");
        assert_eq!(r.capacity(), cap, "容量永不变");
        // 底层 Vec 的容量也没有增长（没有偷偷 realloc）
        assert_eq!(r.buf.capacity(), cap);
    }

    #[test]
    fn keeps_newest_and_drops_oldest() {
        let mut r = SampleRing::new();
        let cap = r.capacity();
        for i in 0..(cap + 10) {
            r.push(s(i));
        }
        assert_eq!(r.last().unwrap().t_ms, (cap + 9) as f64);
        // 最旧的 10 个被覆盖
        assert_eq!(r.iter().next().unwrap().t_ms, 10.0);
        let count = r.iter().count();
        assert_eq!(count, cap);
    }

    #[test]
    fn since_filters_incrementally() {
        let mut r = SampleRing::new();
        for i in 0..100 {
            r.push(s(i));
        }
        let recent: Vec<_> = r.since(95.0).map(|x| x.t_ms).collect();
        assert_eq!(recent, vec![95.0, 96.0, 97.0, 98.0, 99.0]);
    }

    #[test]
    fn empty_ring_is_safe() {
        let r = SampleRing::new();
        assert!(r.is_empty());
        assert!(r.last().is_none());
        assert_eq!(r.iter().count(), 0);
    }
}

//! 全局限速器（聚合口径，而不是每请求/每连接各算一份）。
//!
//! 为什么必须是聚合的：第一版按“每个响应自己睡”实现，8 路并发时实测能跑到
//! 4.8 Gbit/s——限速 1000 就被乘以了连接数，等于没限。测速服务如果要做
//! “温和模式”，用户的预期一定是**整机出口**被压到某个值。
//!
//! 实现是最小泄漏桶：每个请求按字节数在时间轴上**预留**一段，然后睡到自己的
//! 时刻。锁里只做算术，不 await，所以不会有持锁睡眠的问题。

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    /// 每秒允许的字节数；`None` 表示不限速。
    bytes_per_sec: Option<f64>,
    /// 下一个可用时刻（时间轴上的预约指针）。
    next: Mutex<Instant>,
}

impl RateLimiter {
    pub fn new(limit_mbps: f64) -> Self {
        Self {
            bytes_per_sec: if limit_mbps > 0.0 {
                Some(limit_mbps * 1_000_000.0 / 8.0)
            } else {
                None
            },
            next: Mutex::new(Instant::now()),
        }
    }

    pub fn is_limited(&self) -> bool {
        self.bytes_per_sec.is_some()
    }

    /// 为 `n` 字节预留时间；限速时会睡到自己的时刻再返回。
    pub async fn reserve(&self, n: usize) {
        let Some(bps) = self.bytes_per_sec else {
            return;
        };
        let due = {
            let mut next = self.next.lock().expect("限速器锁中毒");
            let now = Instant::now();
            // 关键：不要用 `max(next, now)`。那样每次 sleep 的过冲（tokio 定时器
            // 大约 1 ms 粒度）都会被丢掉，1 Gbps 下单连接实测只跑到 910 Mbit/s（-14%）。
            // 保留欠账让它自然追回来；只有长时间空闲（欠账超过 1 s）才钳到 now。
            let mut start = *next;
            if start + Duration::from_secs(1) < now {
                start = now;
            }
            let due = start + Duration::from_secs_f64(n as f64 / bps);
            *next = due;
            due
        };
        let now = Instant::now();
        if due > now {
            tokio::time::sleep(due - now).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_returns_immediately() {
        let l = RateLimiter::new(0.0);
        assert!(!l.is_limited());
        let t = Instant::now();
        l.reserve(64 * 1024 * 1024).await;
        assert!(t.elapsed() < Duration::from_millis(20));
    }

    #[tokio::test]
    async fn reservations_accumulate_on_one_timeline() {
        // 100 Mbit/s = 12.5 MB/s；预留 4 MB 应约 0.32 s
        let l = RateLimiter::new(100.0);
        assert!(l.is_limited());
        let t = Instant::now();
        l.reserve(4 * 1024 * 1024).await;
        let elapsed = t.elapsed().as_secs_f64();
        assert!(
            (elapsed - 0.335).abs() < 0.08,
            "4 MiB @100Mbit/s 应约 0.335 s，实际 {elapsed:.3} s"
        );
    }

    #[tokio::test]
    async fn many_small_reservations_hit_the_target_rate() {
        // 50 × 1 MiB @ 1000 Mbit/s 应约 0.42 s；实际慢 10% 说明 sleep 过冲没被追回
        let l = RateLimiter::new(1000.0);
        let t = Instant::now();
        for _ in 0..50 {
            l.reserve(1024 * 1024).await;
        }
        let elapsed = t.elapsed().as_secs_f64();
        let target = 50.0 * 1024.0 * 1024.0 / (1000.0 * 1_000_000.0 / 8.0);
        assert!(
            (elapsed - target).abs() / target < 0.05,
            "50 MiB @1000Mbit/s 应 {target:.3} s，实际 {elapsed:.3} s"
        );
    }

    #[tokio::test]
    async fn two_concurrent_reservations_share_one_budget() {
        // 并发两路各 2 MB，总时间应约等于单独 4 MB，而不是各 2 MB 同时完成
        let l = std::sync::Arc::new(RateLimiter::new(100.0));
        let a = l.clone();
        let b = l.clone();
        let t = Instant::now();
        let (r1, r2) = tokio::join!(
            async move { a.reserve(2 * 1024 * 1024).await },
            async move { b.reserve(2 * 1024 * 1024).await }
        );
        let _ = (r1, r2);
        let elapsed = t.elapsed().as_secs_f64();
        assert!(
            elapsed > 0.25,
            "两路合计 4 MiB 应被压到约 0.335 s，实际 {elapsed:.3} s（说明限速不是聚合的）"
        );
    }
}

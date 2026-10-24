//! 状态机行为测试：正常跑完、中途取消、零字节、极慢链路、grace 重置、
//! 自动缩短、上传以服务端结算为准、流失败。
//!
//! 时间全部由测试传入（`apply(ev, now_ms)`），所以没有 sleep，也不用假时钟实现。

use speedtest_core::{Command, Dir, Engine, Event, Phase, Settings, TestPlan};

const RATE: f64 = 125_000_000.0; // 125 MB/s → 1000 Mbit/s 有效负载，×1.06 后 1060.00
const STEP: u64 = 200;

fn engine_with(settings: Settings, plan: TestPlan) -> Engine {
    // 默认关掉自动缩短，先验公式与相位；自动缩短单独一个用例
    let settings = Settings {
        time_auto: false,
        ..settings
    };
    Engine::new(settings, plan)
}

/// 把 ping 阶段跑完（10 个样本）。
fn finish_ping(e: &mut Engine, now: &mut u64) {
    let mut cmds = e.apply(Event::Start, 0.0);
    while cmds.iter().any(|c| matches!(c, Command::Ping { .. })) {
        *now += 20;
        cmds = e.apply(Event::Pong(5.0), *now as f64);
    }
}

#[test]
fn full_test_hits_steady_state_value() {
    let mut e = engine_with(Settings::default(), TestPlan::default());
    let mut now = 0u64;
    finish_ping(&mut e, &mut now);

    assert_eq!(e.phase(), Phase::Download);
    now += 1000; // ping 结束的时刻

    // 下载：每 200 ms 进一批字节，稳定后应恒为 1060.00
    let dl_start = now;
    let mut saw_steady = false;
    while e.phase() == Phase::Download {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
        // 注意：结束下载的那一拍里相位已切到 Upload，快照上的 status 属于新阶段，
        // 所以只在仍处于 Download 时校验稳态读数。
        if now - dl_start > 2000 && e.phase() == Phase::Download {
            assert_eq!(e.snapshot().status.as_deref(), Some("1060.00"));
            saw_steady = true;
        }
    }
    assert!(saw_steady, "应该出现稳态读数");
    assert!((e.snapshot().download_mbps.unwrap() - 1060.0).abs() < 0.01);

    // 上传：同样速率
    assert_eq!(e.phase(), Phase::Upload);
    let ul_start = now;
    while e.phase() == Phase::Upload {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
        if now - ul_start > 3500 && e.phase() == Phase::Upload {
            assert_eq!(e.snapshot().status.as_deref(), Some("1060.00"));
        }
    }
    assert_eq!(e.phase(), Phase::Done);
    assert!((e.snapshot().upload_mbps.unwrap() - 1060.0).abs() < 0.01);
}

#[test]
fn grace_reset_discards_bytes_and_restarts_clock() {
    let mut e = engine_with(Settings::default(), TestPlan::download_only());
    let mut now = 0u64;
    e.apply(Event::Start, 0.0);

    // t=200..1400：字节照进，但仍在 grace 内，不出读数
    for _ in 0..7 {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
        assert!(e.snapshot().status.is_none(), "grace 内不应有读数");
    }

    // t=1600：超过 1.5 s，重置基准并清零已计字节 —— 这一拍不算速率
    now += STEP;
    e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
    e.apply(Event::Tick, now as f64);
    assert!(e.snapshot().status.is_none(), "重置那一拍不计算");
    assert_eq!(e.snapshot().bytes, 0, "重置应清零已计字节");

    // t=1800：重置后满 200 ms，读数等于真实速率（不是把 grace 期的量算进来）
    now += STEP;
    e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
    e.apply(Event::Tick, now as f64);
    assert_eq!(e.snapshot().status.as_deref(), Some("1060.00"));
}

#[test]
fn abort_mid_download_stops_everything() {
    let mut e = engine_with(Settings::default(), TestPlan::download_only());
    e.apply(Event::Start, 0.0);
    let mut now = 0u64;
    for _ in 0..20 {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
    }
    let cmds = e.apply(Event::Abort, now as f64);
    assert_eq!(cmds, vec![Command::StopAll]);
    assert_eq!(e.phase(), Phase::Aborted);
    // 取消后不再有最终值，且后续 tick 无副作用
    assert!(e.snapshot().download_mbps.is_none());
    assert!(e.apply(Event::Tick, now as f64 + 400.0).is_empty());
}

#[test]
fn zero_bytes_reports_zero_not_fail() {
    let mut e = engine_with(Settings::default(), TestPlan::download_only());
    e.apply(Event::Start, 0.0);
    let mut now = 0u64;
    while e.phase() == Phase::Download {
        now += STEP;
        e.apply(Event::Tick, now as f64);
    }
    assert_eq!(e.phase(), Phase::Done);
    assert_eq!(e.snapshot().download_mbps, Some(0.0));
    // 15 s 到点结束；极慢链路不会被 grace 重置成“0 字节立刻完成”
    assert!(now >= 15_000, "应该跑满 15 秒而不是提前结束");
}

#[test]
fn very_slow_link_1kbps_does_not_reset_or_explode() {
    let mut e = engine_with(Settings::default(), TestPlan::download_only());
    e.apply(Event::Start, 0.0);
    let mut now = 0u64;
    let bytes_per_tick = 25u64; // 1 kbit/s ≈ 125 B/s
    let mut last_status: Option<String> = None;
    while e.phase() == Phase::Download {
        now += STEP;
        e.apply(Event::Bytes(bytes_per_tick), now as f64);
        e.apply(Event::Tick, now as f64);
        if let Some(s) = e.snapshot().status.clone() {
            last_status = Some(s);
        }
    }
    let value = e.snapshot().download_mbps.unwrap();
    assert!(
        value > 0.0 && value < 0.01,
        "1 kbit/s 应换算成 0.001 量级，实得 {value}"
    );
    assert_eq!(last_status.as_deref(), Some("0.00"));
}

#[test]
fn time_auto_shortens_fast_tests() {
    let settings = Settings {
        time_auto: true,
        ..Settings::default()
    };
    let mut e = Engine::new(settings, TestPlan::download_only());
    e.apply(Event::Start, 0.0);
    let mut now = 0u64;
    while e.phase() == Phase::Download {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
    }
    // 每拍 bonus 封顶 400 ms，speed 极高 → t + bonus 提前超过 15 s
    assert!(now < 15_000, "高速链路应被自动缩短，实得 {now} ms");
    assert!(now > 3000, "但不该一上来就结束，实得 {now} ms");
}

#[test]
fn upload_uses_server_side_settlement() {
    let mut e = engine_with(
        Settings::default(),
        TestPlan {
            ping: false,
            download: false,
            upload: true,
        },
    );
    let mut now = 0u64;
    e.apply(Event::Start, 0.0);
    assert_eq!(e.phase(), Phase::Upload);

    // 客户端自己数的字节（会偏高：还压在 bufferedAmount 里 ）
    for _ in 0..20 {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
    }
    // 服务端口径：同样的字节但耗时更长 → 小于客户端估算
    let settled_bytes = (RATE * 0.2 * 20.0) as u64;
    e.apply(
        Event::UploadSettled {
            bytes: settled_bytes,
            ms: 8000.0,
        },
        now as f64,
    );
    while e.phase() == Phase::Upload {
        now += STEP;
        e.apply(Event::Tick, now as f64);
    }
    let v = e.snapshot().upload_mbps.unwrap();
    let expected = settled_bytes as f64 * 8.0 * 1.06 / 8.0 / 1_000_000.0;
    assert!(
        (v - expected).abs() < 0.01,
        "应以服务端结算为准：{v} vs {expected}"
    );
}

#[test]
fn late_settlement_overrides_finished_upload() {
    let mut e = engine_with(
        Settings::default(),
        TestPlan {
            ping: false,
            download: false,
            upload: true,
        },
    );
    let mut now = 0u64;
    e.apply(Event::Start, 0.0);
    while e.phase() == Phase::Upload {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
    }
    assert_eq!(e.phase(), Phase::Done);
    let client_side = e.snapshot().upload_mbps.expect("阶段结束应有值");

    // 结算迟到：服务端数的字节更少、耗时更长（客户端把还压在缓冲里的也算进来了）
    e.apply(
        Event::UploadSettled {
            bytes: 100_000_000,
            ms: 8_000.0,
        },
        now as f64 + 100.0,
    );
    let settled = e.snapshot().upload_mbps.expect("迟到结算也应覆盖");
    assert!(
        settled < client_side,
        "服务端口径应更低：{settled} vs {client_side}"
    );
    let expected = 100_000_000.0 * 8.0 * 1.06 / 8.0 / 1_000_000.0;
    assert!((settled - expected).abs() < 0.01, "{settled} vs {expected}");

    // 实测值不乘 1.06，两者比值应正好是补偿系数
    let settled_raw = e.snapshot().upload_mbps_raw.expect("应有实测值");
    let expected_raw = 100_000_000.0 * 8.0 / 8.0 / 1_000_000.0;
    assert!(
        (settled_raw - expected_raw).abs() < 0.01,
        "{settled_raw} vs {expected_raw}"
    );
    assert!((settled / settled_raw - 1.06).abs() < 1e-9);
}

#[test]
fn stream_failure_ends_phase_and_flags_result() {
    let mut e = engine_with(Settings::default(), TestPlan::download_only());
    e.apply(Event::Start, 0.0);
    let mut now = 0u64;
    for _ in 0..12 {
        now += STEP;
        e.apply(Event::Bytes((RATE * 0.2) as u64), now as f64);
        e.apply(Event::Tick, now as f64);
    }
    e.apply(Event::StreamFailed, now as f64);
    let cmds = e.apply(Event::Tick, now as f64 + STEP as f64);
    assert!(cmds.contains(&Command::StopAll));
    assert_eq!(e.phase(), Phase::Done);
    assert!(e.snapshot().failed);
}

#[test]
fn ping_phase_matches_upstream_rules() {
    let s = Settings {
        ping_count: 6,
        ..Settings::default()
    };
    let mut e = engine_with(
        s,
        TestPlan {
            ping: true,
            download: false,
            upload: false,
        },
    );
    let mut now = 0u64;
    let mut cmds = e.apply(Event::Start, 0.0);
    assert_eq!(cmds, vec![Command::Ping { seq: 0 }]);

    // 第一次回包只对齐基准；之后 12, 9, 15, 9, 20
    for rtt in [10.0, 12.0, 9.0, 15.0, 9.0, 20.0] {
        now += 20;
        cmds = e.apply(Event::Pong(rtt), now as f64);
    }
    // 最后一个 pong 之后引擎已经进入 Done，命令里只剩 StopAll
    assert!(cmds.contains(&Command::StopAll));
    assert_eq!(e.phase(), Phase::Done, "样本够数应结束");
    let snap = e.snapshot();
    assert_eq!(snap.ping_status.as_deref(), Some("9.00"), "ping 取最小样本");
    // 逐步按上游公式演算（prev 是上一次被钳制后的样本值）：
    //   #2(12): ping=12                                      prev=12
    //   #3( 9): |9-12|=3，样本序号 2 → jitter=3               prev=9
    //   #4(15): |15-9|=6>3    → 3*0.3+6*0.7    = 5.1         prev=15
    //   #5( 9): |9-15|=6>5.1  → 5.1*0.3+6*0.7  = 5.73        prev=9
    //   #6(20): |20-9|=11>5.73→ 5.73*0.3+11*0.7= 9.419       → "9.42"
    assert_eq!(
        snap.jitter_status.as_deref(),
        Some("9.42"),
        "jitter 为加权平均"
    );
}

#[test]
fn sub_millisecond_pong_is_clamped() {
    let s = Settings {
        ping_count: 4,
        ..Settings::default()
    };
    let mut e = engine_with(
        s,
        TestPlan {
            ping: true,
            download: false,
            upload: false,
        },
    );
    e.apply(Event::Start, 0.0);
    // 0 ms 时退回上一次的值，再退到 1；jitter 不会因为 0 而失真
    for rtt in [5.0, 0.0, 0.0] {
        e.apply(Event::Pong(rtt), 20.0);
    }
    // 上游的钳制链是 `inst < 1 → 用上一次的值；仍 < 1 → 用 1`。
    // 第一个回包不计值，所以 prev 仍是 0：第二个回包的 0 ms 最终被钳到 1 ms。
    // 这是上游的真实行为（浏览器计时粗化时会给 0），照抄不美化。
    assert_eq!(e.snapshot().ping_status.as_deref(), Some("1.00"));
    assert_eq!(e.snapshot().jitter_status.as_deref(), Some("0.00"));
}

#[test]
fn start_is_idempotent_and_stopall_on_finish() {
    let mut e = engine_with(Settings::default(), TestPlan::download_only());
    let first = e.apply(Event::Start, 0.0);
    assert_eq!(
        first,
        vec![Command::StartStreams {
            dir: Dir::Down,
            count: 6,
            stagger_ms: 300.0
        }]
    );
    assert!(e.apply(Event::Start, 0.0).is_empty(), "重复 Start 应被忽略");

    let mut now = 0u64;
    while e.phase() == Phase::Download {
        now += STEP;
        e.apply(Event::Tick, now as f64);
    }
    // 结束时最后一个命令必须是 StopAll（调用方据此清理所有流）
    let cmds = e.apply(Event::Tick, now as f64 + 400.0);
    assert!(cmds.is_empty());
}

//! 录制回放：把一段真实时序（时间戳 + 事件）存成文本，重放两次必须得到同样的结果。
//!
//! 这是“确定性回归”的落点：以后改动调度逻辑时，先跑回放，再看行为用例。
//! 录制格式故意做成一行一个事件（`<ms> <event> [参数]`），方便从浏览器或 CLI 的
//! trace 里直接剪贴进来。

use speedtest_core::{Engine, Event, Phase, Settings, TestPlan};

/// 一段真实感的录制：ping 有抖动，下载先低后高（慢启动），上传基本稳定。
/// 数字取自 10GbE 内网的一次实测量级（8 路合计约 1 GB/s）。
const RECORDING: &str = "\
0    start
20   pong 4.2
40   pong 5.1
60   pong 4.6
80   pong 6.0
100  pong 4.3
120  pong 4.9
140  pong 5.5
160  pong 4.4
180  pong 4.8
200  pong 4.7
400  bytes 1200000
600  bytes 8400000
800  bytes 21500000
1000 bytes 24800000
1200 bytes 25000000
1400 bytes 25000000
1600 bytes 25000000
1800 bytes 25000000
2000 bytes 25000000
2200 bytes 25000000
2400 bytes 25000000
2600 bytes 25000000
2800 bytes 25000000
3000 bytes 25000000
3200 bytes 25000000
3400 bytes 25000000
3600 bytes 25000000
3800 bytes 25000000
4000 bytes 25000000
4200 bytes 25000000
4400 bytes 25000000
4600 bytes 25000000
4800 bytes 25000000
5000 bytes 25000000
5200 bytes 25000000
5400 bytes 25000000
5600 bytes 25000000
5800 bytes 25000000
6000 bytes 25000000
6200 bytes 25000000
6400 bytes 25000000
6600 bytes 25000000
6800 bytes 25000000
7000 bytes 25000000
7200 bytes 25000000
7400 bytes 25000000
7600 bytes 25000000
7800 bytes 25000000
8000 bytes 25000000
8200 bytes 25000000
8400 bytes 25000000
8600 bytes 25000000
8800 bytes 25000000
9000 bytes 25000000
9200 bytes 25000000
9400 bytes 25000000
9600 bytes 25000000
9800 bytes 25000000
10000 bytes 25000000
10200 bytes 25000000
10400 bytes 25000000
10600 bytes 25000000
10800 bytes 25000000
11000 bytes 25000000
11200 bytes 25000000
11400 bytes 25000000
11600 bytes 25000000
11800 bytes 25000000
12000 bytes 25000000
12200 bytes 25000000
12400 bytes 25000000
12600 bytes 25000000
12800 bytes 25000000
13000 bytes 25000000
13200 bytes 25000000
13400 bytes 25000000
13600 bytes 25000000
13800 bytes 25000000
14000 bytes 25000000
14200 bytes 25000000
14400 bytes 25000000
14600 bytes 25000000
14800 bytes 25000000
15000 bytes 25000000
15200 bytes 25000000
";

fn parse(line: &str) -> Option<(f64, Event)> {
    let mut it = line.split_whitespace();
    let ms: f64 = it.next()?.parse().ok()?;
    let ev = match it.next()? {
        "start" => Event::Start,
        "tick" => Event::Tick,
        "bytes" => Event::Bytes(it.next()?.parse().ok()?),
        "pong" => Event::Pong(it.next()?.parse().ok()?),
        "fail" => Event::StreamFailed,
        "abort" => Event::Abort,
        other => panic!("未知事件 {other}"),
    };
    Some((ms, ev))
}

/// 回放并返回引擎，同时记录回放过程中出现过的最大读数（稳态值）。
///
/// 为什么要单独记录最大读数：录制结束时字节流随之中断，而引擎还要靠 tick 走完
/// 剩余时长，尾部那几拍自然会摊薄速率。真实测试里字节是持续到结束的，所以
/// “稳态读数”比“最后一拍读数”更能代表录制所描述的那条链路。
fn replay(recording: &str, settings: Settings, plan: TestPlan) -> (Engine, Option<String>) {
    let mut e = Engine::new(settings, plan);
    let mut best: Option<f64> = None;
    let mut last_ms = 0.0;
    for line in recording.lines().filter(|l| !l.trim().is_empty()) {
        let (ms, ev) = parse(line).expect("录制行必须可解析");
        // 录制里只有显式事件；Tick 由回放器在每个事件后补齐（引擎只认 200 ms 节拍）
        if (ms - last_ms) >= 200.0 {
            e.apply(Event::Tick, ms);
            last_ms = ms;
        }
        e.apply(ev, ms);
        // 抓“运行中的读数”（status），而不是最终值：最终值要等阶段结束才有
        if let Some(s) = e.snapshot().status.as_deref() {
            if let Ok(v) = s.parse::<f64>() {
                if best.map_or(true, |b| v > b) {
                    best = Some(v);
                }
            }
        }
    }
    // 收尾：补 Tick 直到阶段自然结束，避免录制截断在最后一拍
    let mut ms = last_ms;
    let mut guard = 0;
    while e.phase() == Phase::Download || e.phase() == Phase::Upload {
        ms += 200.0;
        e.apply(Event::Tick, ms);
        guard += 1;
        assert!(guard < 10_000, "回放未收敛");
    }
    (e, best.map(speedtest_core::to_fixed2))
}

fn recording_settings() -> Settings {
    Settings {
        time_auto: false, // 录制按固定时长跑，关掉自动缩短才能对表
        ..Settings::default()
    }
}

#[test]
fn replay_is_deterministic() {
    let (a, best_a) = replay(RECORDING, recording_settings(), TestPlan::download_only());
    let (b, best_b) = replay(RECORDING, recording_settings(), TestPlan::download_only());

    assert_eq!(a.phase(), Phase::Done);
    assert_eq!(a.snapshot(), b.snapshot(), "同一份录制必须得到同一份结果");
    assert_eq!(a.ping_acc(), b.ping_acc());
    assert_eq!(best_a, best_b, "稳态读数也必须一致");
}

#[test]
fn replay_values_are_sane() {
    let (e, best) = replay(RECORDING, recording_settings(), TestPlan::download_only());
    let dl = e.snapshot().download_mbps.expect("应有下载结果");

    // 稳态是 25 MB / 200 ms = 125 MB/s → 1060.00 Mbit/s（×1.06）
    assert_eq!(best.as_deref(), Some("1060.00"), "稳态读数");
    // 最后一拍因为录制尾部没有新字节会略低，但仍然在同一量级
    assert!(dl > 950.0 && dl < 1061.0, "收尾读数 {dl} 应仍在同一量级");
}

#[test]
fn replay_ping_phase_matches_recording() {
    let (e, _) = replay(
        RECORDING,
        recording_settings(),
        TestPlan {
            ping: true,
            download: true,
            upload: false,
        },
    );
    // 录制里 10 个 pong 的样本是 4.2 5.1 4.6 6.0 4.3 4.9 5.5 4.4 4.8 4.7
    let ping = e.ping_acc();
    assert_eq!(ping.samples, 10);
    // 第一个 pong（4.2）只用来对齐时间基准、不计入；最小值来自第二个样本 4.3
    assert!(
        (ping.ping - 4.3).abs() < 1e-9,
        "ping 取最小样本：{}",
        ping.ping
    );
    assert!(
        ping.jitter > 0.0 && ping.jitter < 10.0,
        "jitter={}",
        ping.jitter
    );
    assert_eq!(e.snapshot().ping_status.as_deref(), Some("4.30"));
}

#[test]
fn replay_truncated_mid_download_leaves_phase_open() {
    // 只喂前 30 行（下载刚开始），再取消：应停在 Aborted 且没有最终值
    let truncated: String = RECORDING.lines().take(30).collect::<Vec<_>>().join("\n");
    let mut e = Engine::new(recording_settings(), TestPlan::download_only());
    let mut last = 0.0;
    for line in truncated.lines() {
        let (ms, ev) = parse(line).unwrap();
        if ms - last >= 200.0 {
            e.apply(Event::Tick, ms);
            last = ms;
        }
        e.apply(ev, ms);
    }
    e.apply(Event::Abort, last);
    assert_eq!(e.phase(), Phase::Aborted);
    assert!(e.snapshot().download_mbps.is_none());
}

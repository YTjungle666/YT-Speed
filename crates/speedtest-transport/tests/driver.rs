//! 驱动层行为测试：用 MockTransport 记录 core 下发的动作，验证“命令→动作”的翻译
//! 与 UI 快照节流，全部在 native 上跑（浏览器里跑的是同一份代码）。

use speedtest_core::{Dir, Phase, Settings, TestPlan};
use speedtest_transport::{Driver, Transport, MAX_SAMPLES, UI_INTERVAL_MS};

#[derive(Debug, PartialEq)]
enum Action {
    Ping(u32),
    Start(Dir, u32, f64),
    Stop,
}

#[derive(Default)]
struct Mock {
    actions: Vec<Action>,
}

impl Transport for Mock {
    fn start_streams(&mut self, dir: Dir, count: u32, stagger_ms: f64) {
        self.actions.push(Action::Start(dir, count, stagger_ms));
    }
    fn stop_all(&mut self) {
        self.actions.push(Action::Stop);
    }
    fn ping(&mut self, seq: u32) {
        self.actions.push(Action::Ping(seq));
    }
}

fn driver(plan: TestPlan) -> Driver<Mock> {
    let settings = Settings {
        time_auto: false,
        ..Settings::default()
    };
    Driver::new(settings, plan, Mock::default())
}

#[test]
fn start_issues_ping_then_streams() {
    let mut d = driver(TestPlan::default());
    assert!(d.start(0.0));
    assert_eq!(d.transport().actions, vec![Action::Ping(0)]);

    let mut now = 0.0;
    for _ in 0..10 {
        now += 20.0;
        d.on_pong(4.0, now);
    }
    // ping 跑完立刻开下载流：8 路、错开 300 ms
    assert_eq!(
        *d.transport().actions.last().unwrap(),
        Action::Start(Dir::Down, 8, 300.0)
    );
    assert_eq!(d.snapshot().phase, Phase::Download);
}

#[test]
fn duplicate_start_is_ignored() {
    let mut d = driver(TestPlan::download_only());
    assert!(d.start(0.0));
    assert!(!d.start(10.0), "第二次 start 应返回 false");
    assert_eq!(
        d.transport().actions,
        vec![Action::Start(Dir::Down, 8, 300.0)]
    );
}

#[test]
fn abort_emits_stop_exactly_once() {
    let mut d = driver(TestPlan::download_only());
    d.start(0.0);
    d.on_bytes(1_000_000, 200.0);
    d.abort(300.0);
    assert_eq!(d.snapshot().phase, Phase::Aborted);
    assert_eq!(d.transport().actions.last().unwrap(), &Action::Stop);

    // 二次 abort 不应再下发 Stop
    let before = d.transport().actions.len();
    d.abort(400.0);
    assert_eq!(d.transport().actions.len(), before);
}

#[test]
fn ui_frames_are_throttled_to_10hz() {
    let mut d = driver(TestPlan::download_only());
    d.start(0.0);
    let mut frames = 0;
    let mut now = 0.0;
    // 2 秒，每 20 ms 一次 tick：应只得到约 2s/100ms = 20 帧
    while now < 2000.0 {
        now += 20.0;
        d.on_bytes(2_000_000, now);
        if d.tick(now).is_some() {
            frames += 1;
        }
    }
    let expected = (2000.0 / UI_INTERVAL_MS) as i32;
    assert!(
        (frames - expected).abs() <= 2,
        "2 秒应有约 {expected} 帧，实得 {frames}"
    );
}

#[test]
fn samples_stay_bounded_over_long_run() {
    let mut d = driver(TestPlan::download_only());
    d.start(0.0);
    // 模拟 30 分钟稳定测试：18 万个 tick
    let mut now = 0.0;
    while now < 30.0 * 60_000.0 {
        now += 20.0;
        d.on_bytes(1_000_000, now);
        d.tick(now);
        // 引擎 15 s 就结束了，这里只看采样缓冲是否被撑爆
        if d.snapshot().phase != Phase::Download {
            break;
        }
    }
    assert!(d.samples().len() <= MAX_SAMPLES);
    assert_eq!(d.samples().capacity(), MAX_SAMPLES);
}

#[test]
fn upload_settlement_overrides_client_count() {
    let mut d = driver(TestPlan {
        ping: false,
        download: false,
        upload: true,
    });
    d.start(0.0);
    assert_eq!(
        d.transport().actions.last().unwrap(),
        &Action::Start(Dir::Up, 1, 300.0)
    );
    let mut now = 0.0;
    for _ in 0..20 {
        now += 200.0;
        d.on_bytes(25_000_000, now);
        d.tick(now);
    }
    // 服务端结算：同样字节、更长耗时 → 结果应更小
    d.on_upload_settled(500_000_000, 8000.0, now);
    while d.snapshot().phase == Phase::Upload {
        now += 200.0;
        d.tick(now);
    }
    let v = d.snapshot().upload_mbps.expect("应有上传结果");
    let expected = 500_000_000.0 * 8.0 * 1.06 / 8.0 / 1_000_000.0;
    assert!((v - expected).abs() < 0.01, "{v} vs {expected}");
}

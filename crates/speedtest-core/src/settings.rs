//! 测试参数。默认值全部照抄上游 `speedtest_worker.js`，数值来源见各字段注释。

use crate::conv::Conv;

/// 上游默认设置（只保留本方案用得到的字段；遥测/ISP 相关项已按 PLAN §0.1 删除）。
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// `time_dl_max`：下载最大时长（秒）。
    pub dl_max_secs: f64,
    /// `time_ul_max`：上传最大时长（秒）。
    pub ul_max_secs: f64,
    /// `time_dlGraceTime`：下载先跑多久再开始计数（等 TCP 窗口长起来）。
    pub dl_grace_secs: f64,
    /// `time_ulGraceTime`：上传 grace，比下载长，因为要等发送缓冲填满。
    pub ul_grace_secs: f64,
    /// `time_auto`：按速率自动缩短测试时间。
    pub time_auto: bool,
    /// `count_ping`：ping 采样次数。
    pub ping_count: u32,
    /// 换算参数（开销补偿 + 十进/二进制兆）。
    pub conv: Conv,
    /// 下载并发流数。实测 8 路是这台 Windows 的最优点，见 PLAN §0.5。
    pub dl_streams: u32,
    /// 上传并发：WebSocket 单连接即线速，1 路足够。
    pub ul_streams: u32,
    /// 并发流的错开启动间隔（毫秒）。上游 `xhr_multistreamDelay = 300`：
    /// 所有流不要同时起跑，否则会一起撞上慢启动、也一起结束留下空档。
    /// 由传输层实现，引擎只把它随命令下发。
    pub stream_stagger_ms: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dl_max_secs: 15.0,
            ul_max_secs: 15.0,
            dl_grace_secs: 1.5,
            ul_grace_secs: 3.0,
            time_auto: true,
            ping_count: 10,
            conv: Conv::default(),
            // 实测 6 路 > 8 路：交错扫描（同一轮里 6/8/10/12 各跑一遍，抵消机器冷热）
            // 十万兆客户端 6 路 7525 / 8 路 7170 Mbit/s，本机回环 6 路 28264 / 8 路 27532。
            // 更多路数反而更慢（10 路 7350、12 路 6681），因为浏览器每路都要独立调度。
            dl_streams: 6,
            ul_streams: 1,
            // 100 ms：grace 1.5 s 结束时 6 条流都已完成 slow start（原来 300 ms 时
            // 第 6 条流正好在计时窗口起点才起跑，前 1/6 窗口是 5 条流在跑）。
            stream_stagger_ms: 100.0,
        }
    }
}

impl Settings {
    /// 温和模式：限速 500 Mbit/s 的服务端配合使用（服务端限速，客户端不变）。
    pub fn gentle() -> Self {
        Self::default()
    }
}

/// 一次测试要跑哪些阶段。留空表示跳过该阶段。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TestPlan {
    pub ping: bool,
    pub download: bool,
    pub upload: bool,
}

impl Default for TestPlan {
    fn default() -> Self {
        Self {
            ping: true,
            download: true,
            upload: true,
        }
    }
}

impl TestPlan {
    pub fn download_only() -> Self {
        Self {
            ping: false,
            download: true,
            upload: false,
        }
    }
}

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
            dl_streams: 8,
            ul_streams: 1,
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

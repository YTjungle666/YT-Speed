//! 服务端配置：命令行参数优先，其次环境变量，最后是默认值。
//!
//! 按 PLAN §4.2，限速这类可调项将来要进 UI 设置页（默认值 + 0 = 关闭）；
//! 这里先做成启动参数，/healthz 会把它回显出来，前端据此显示当前限速。

use std::net::SocketAddr;
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 8080;
/// 下载块大小：8 MiB 预生成、按 1 MiB 切片轮转（PLAN §3）。
pub const BLOCK_BYTES: usize = 8 * 1024 * 1024;
pub const SLICE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    /// 静态资源目录；为空表示只提供 API。
    pub web_dir: Option<PathBuf>,
    /// 下载限速（Mbit/s），0 = 不限。
    pub max_download_mbps: f64,
    /// 上传限速（Mbit/s），0 = 不限。
    pub max_upload_mbps: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            web_dir: Some(PathBuf::from("web")),
            max_download_mbps: 0.0,
            max_upload_mbps: 0.0,
        }
    }
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl Config {
    /// `SPEEDTEST_LISTEN`（`ip:port` 或纯端口）、`SPEEDTEST_WEB_DIR`、
    /// `SPEEDTEST_MAX_DOWNLOAD_MBPS`、`SPEEDTEST_MAX_UPLOAD_MBPS`。
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("SPEEDTEST_LISTEN") {
            if let Ok(addr) = v.parse() {
                cfg.listen = addr;
            } else if let Ok(port) = v.parse::<u16>() {
                cfg.listen.set_port(port);
            }
        }
        if let Ok(v) = std::env::var("SPEEDTEST_WEB_DIR") {
            cfg.web_dir = if v.is_empty() {
                None
            } else {
                Some(PathBuf::from(v))
            };
        }
        cfg.max_download_mbps = env_f64("SPEEDTEST_MAX_DOWNLOAD_MBPS", 0.0);
        cfg.max_upload_mbps = env_f64("SPEEDTEST_MAX_UPLOAD_MBPS", 0.0);
        cfg
    }
}

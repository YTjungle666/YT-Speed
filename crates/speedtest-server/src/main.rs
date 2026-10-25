//! `speedtestd`：LibreSpeed 服务端（Rust）。
//!
//! 端点在 PLAN §3 固定，`.php` 别名一并保留，第三方 CLI 可以零改动接入。
//! 不提供遥测/结果库（PLAN §0.1），所以没有 `/results/*`。

mod assets;
mod config;
mod garbage;
mod getip;
mod limits;
mod pace;
mod upload;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::config::Config;
use crate::garbage::Payload;
use crate::upload::Settlements;

/// 让 WS 处理器只依赖结算表，而不是整个 `App`（axum 的 `FromRef` 子状态提取）。
impl axum::extract::FromRef<App> for Arc<Settlements> {
    fn from_ref(app: &App) -> Self {
        app.settlements.clone()
    }
}

#[derive(Clone)]
struct App {
    /// sid → 该会话所有下载流的取消标志。手机端 abort 无效时的唯一可靠刹车。
    stops: Arc<std::sync::Mutex<std::collections::HashMap<u64, StopEntry>>>,
    cfg: Config,
    payload: Payload,
    settlements: Arc<Settlements>,
    /// 下载/上传的**聚合**限速器（所有连接共享一条时间轴）。
    dl_limiter: Arc<pace::RateLimiter>,
    ul_limiter: Arc<pace::RateLimiter>,
    /// WS 连接数闸门：超过 `MAX_WS_CONNECTIONS` 直接 503，绝不无界排队。
    ws_slots: Arc<tokio::sync::Semaphore>,
}

#[derive(serde::Deserialize)]
struct GarbageQuery {
    #[serde(rename = "ckSize")]
    ck_size: Option<String>,
    /// `stream=1`：不给 Content-Length，一直发到客户端断开（省掉请求间空档）。
    stream: Option<String>,
    /// 客户端自己的窗口上限（秒）。手机被冻结时服务端就写到这一刻为止。
    maxsecs: Option<String>,
    /// 会话 id：客户端阶段结束时用它一次性关掉本会话的所有流。
    sid: Option<String>,
}

/// 客户端阶段打点：只打一行日志，用来对齐「客户端何时结束某阶段」与服务端流的结束时间。
#[derive(serde::Deserialize)]
struct MarkQuery {
    ev: Option<String>,
    t: Option<String>,
}

struct StopEntry {
    at: std::time::Instant,
    flags: Vec<Arc<std::sync::atomic::AtomicBool>>,
}

impl App {
    /// 给一条流注册取消标志；顺带清过期条目、超上限丢最旧的（表必须有界）。
    fn register_stream(&self, sid: u64) -> Arc<std::sync::atomic::AtomicBool> {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut map = self.stops.lock().unwrap();
        let ttl = std::time::Duration::from_secs(limits::STOP_SESSION_TTL_SECS);
        map.retain(|_, e| e.at.elapsed() < ttl);
        while map.len() >= limits::MAX_STOP_SESSIONS {
            let oldest = map
                .iter()
                .min_by_key(|(_, e)| e.at)
                .map(|(k, _)| *k);
            match oldest {
                Some(k) => {
                    map.remove(&k);
                }
                None => break,
            }
        }
        map.entry(sid)
            .or_insert_with(|| StopEntry {
                at: std::time::Instant::now(),
                flags: Vec::new(),
            })
            .flags
            .push(flag.clone());
        flag
    }

    /// 客户端主动结束一个会话：把它的所有流置为取消。
    fn stop_session(&self, sid: u64) -> usize {
        let mut map = self.stops.lock().unwrap();
        let n = match map.remove(&sid) {
            Some(e) => {
                for f in &e.flags {
                    f.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                e.flags.len()
            }
            None => 0,
        };
        n
    }
}

#[derive(serde::Deserialize)]
struct StopQuery {
    sid: Option<String>,
}

async fn stop(State(app): State<App>, Query(q): Query<StopQuery>) -> String {
    let sid = q.sid.as_deref().and_then(|v| v.trim().parse::<u64>().ok());
    match sid {
        Some(sid) => {
            let n = app.stop_session(sid);
            tracing::info!(target: "stop", sid, streams = n, "CLIENT STOP");
            format!("{{\"stopped\":{n}}}")
        }
        None => "{\"error\":\"sid 无效\"}".to_string(),
    }
}

async fn mark(Query(q): Query<MarkQuery>) -> &'static str {
    tracing::info!(
        target: "mark",
        ev = q.ev.unwrap_or_default(),
        client_t = q.t.unwrap_or_default(),
        "CLIENT MARK"
    );
    "ok"
}

async fn garbage(
    State(app): State<App>,
    Query(q): Query<GarbageQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let ck = garbage::parse_ck_size(q.ck_size.as_deref());
    let endless = matches!(q.stream.as_deref(), Some("1") | Some("true"));
    let max_secs = q
        .maxsecs
        .as_deref()
        .and_then(|v| v.trim().parse::<u64>().ok());
    // 手机端「abort 掐不断」到底是哪个浏览器，只有 UA 能回答（远程机器开不了 devtools）
    if endless {
        let ua = headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-");
        tracing::debug!(target: "garbage", ua, maxsecs = ?max_secs, "DOWNLOAD stream start");
    }

    let sid = q.sid.as_deref().and_then(|v| v.trim().parse::<u64>().ok());
    let cancel = match (endless, sid) {
        (true, Some(sid)) => Some(app.register_stream(sid)),
        _ => None,
    };
    let mut resp = garbage::garbage_response(
        &app.payload,
        ck,
        endless,
        max_secs,
        cancel,
        app.dl_limiter.clone(),
    );
    let h = resp.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, OPTIONS".parse().unwrap(),
    );
    h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, "*".parse().unwrap());
    resp
}

async fn options() -> Response {
    let mut resp = StatusCode::NO_CONTENT.into_response();
    let h = resp.headers_mut();
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*".parse().unwrap());
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "GET, POST, OPTIONS".parse().unwrap(),
    );
    h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, "*".parse().unwrap());
    h.insert(header::ACCESS_CONTROL_MAX_AGE, "600".parse().unwrap());
    resp
}

async fn healthz(State(app): State<App>) -> Response {
    let body = serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("SPEEDTEST_COMMIT").unwrap_or("unknown"),
        "max_download_mbps": app.cfg.max_download_mbps,
        "max_upload_mbps": app.cfg.max_upload_mbps,
    });
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(body)).into_response()
}

async fn index(State(app): State<App>) -> Response {
    assets::index(app.cfg.web_dir.clone()).await
}

async fn static_file(State(app): State<App>, UrlPath(rel): UrlPath<String>) -> Response {
    match app.cfg.web_dir.clone() {
        Some(root) => assets::serve(root, rel).await,
        None => (StatusCode::NOT_FOUND, "static assets disabled").into_response(),
    }
}

async fn empty_get() -> Response {
    upload::empty_get().await
}

/// 包装：把限速值从 `App` 交给上传模块，避免它依赖整个 `App`。
async fn empty_post(State(app): State<App>, body: axum::body::Body) -> Response {
    upload::empty_post(body, app.ul_limiter.clone()).await
}

async fn ws_upload(State(app): State<App>, ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    // 连接数闸门：拿不到许可就 503，不排队、不等待（排队会把内存留给等待者）
    let Ok(permit) = app.ws_slots.clone().try_acquire_owned() else {
        tracing::warn!("WS 连接数已达上限，拒绝新连接");
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many websocket connections",
        )
            .into_response();
    };
    upload::ws_upload(ws, app.settlements.clone(), app.ul_limiter.clone(), permit)
}

fn router(app: App) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/healthz", get(healthz))
        // 下载
        .route("/backend/garbage", get(garbage).options(options))
        .route("/backend/garbage.php", get(garbage).options(options))
        // 上传黑洞 / ping 目标
        .route(
            "/backend/empty",
            get(empty_get).post(empty_post).options(options),
        )
        .route(
            "/backend/empty.php",
            get(empty_get).post(empty_post).options(options),
        )
        // 客户端 IP
        .route("/backend/getIP", get(getip::get_ip))
        .route("/backend/getIP.php", get(getip::get_ip))
        // 稳定测试页（P5 之前给个占位，别让链接 404）
        .route("/stability", get(index))
        .route("/stability.html", get(index))
        // WS 上传与结算
        .route("/backend/ws-upload", get(ws_upload))
        .route("/backend/ws-result", get(upload::ws_result))
        .route("/backend/mark", get(mark))
        .route("/backend/stop", get(stop))
        // 兜底：静态资产
        .route("/assets/{*path}", get(static_file))
        .route("/favicon.ico", get(static_file_with("favicon.ico")))
        .route("/{*path}", get(fallback))
        .with_state(app)
}

fn static_file_with(name: &'static str) -> axum::routing::MethodRouter<App> {
    get(move |State(app): State<App>| async move {
        match app.cfg.web_dir.clone() {
            Some(root) => assets::serve(root, name.to_string()).await,
            None => (StatusCode::NOT_FOUND, "static assets disabled").into_response(),
        }
    })
}

/// 未知路径：如果是 GET 且不带扩展名，交给 SPA 入口；否则 404。
async fn fallback(
    State(app): State<App>,
    method: Method,
    UrlPath(path): UrlPath<String>,
) -> Response {
    if method != Method::GET {
        return (StatusCode::METHOD_NOT_ALLOWED, "method not allowed").into_response();
    }
    if PathBuf::from(&path).extension().is_none() {
        return assets::index(app.cfg.web_dir.clone()).await;
    }
    match app.cfg.web_dir.clone() {
        Some(root) => assets::serve(root, path).await,
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn parse_args(cfg: &mut Config) {
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--listen" | "-l" => {
                if let Some(v) = it.next() {
                    match v.parse::<SocketAddr>() {
                        Ok(a) => cfg.listen = a,
                        Err(_) => {
                            if let Ok(p) = v.parse::<u16>() {
                                cfg.listen.set_port(p);
                            } else {
                                eprintln!("--listen 需要 ip:port 或端口号");
                                std::process::exit(2);
                            }
                        }
                    }
                }
            }
            "--web-dir" => {
                cfg.web_dir = it
                    .next()
                    .map(PathBuf::from)
                    .filter(|p| !p.as_os_str().is_empty());
            }
            "--max-download-mbps" => {
                cfg.max_download_mbps = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            }
            "--max-upload-mbps" => {
                cfg.max_upload_mbps = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            }
            "--help" | "-h" => {
                println!(
                    "speedtestd {}\n\n\
                     --listen <ip:port>          监听地址（默认 0.0.0.0:8080）\n\
                     --web-dir <dir>             静态资产目录（空字符串关闭）\n\
                     --max-download-mbps <n>     下载限速，0 = 不限\n\
                     --max-upload-mbps <n>       上传限速，0 = 不限\n\n\
                     环境变量同义：SPEEDTEST_LISTEN / SPEEDTEST_WEB_DIR /\n\
                     SPEEDTEST_MAX_DOWNLOAD_MBPS / SPEEDTEST_MAX_UPLOAD_MBPS",
                    env!("CARGO_PKG_VERSION")
                );
                std::process::exit(0);
            }
            _ => {
                eprintln!("未知参数 {arg}（--help 查看用法）");
                std::process::exit(2);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,speedtest_server=debug".into()),
        )
        .with_target(false)
        .init();

    let mut cfg = Config::from_env();
    parse_args(&mut cfg);

    // 8 MiB 随机块在启动时生成一次
    let payload = Payload::generate();
    let app = App {
        cfg: cfg.clone(),
        payload,
        settlements: Arc::new(Settlements::default()),
        dl_limiter: Arc::new(pace::RateLimiter::new(cfg.max_download_mbps)),
        ul_limiter: Arc::new(pace::RateLimiter::new(cfg.max_upload_mbps)),
        stops: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        ws_slots: Arc::new(tokio::sync::Semaphore::new(limits::MAX_WS_CONNECTIONS)),
    };

    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!(
        listen = %cfg.listen,
        web_dir = ?cfg.web_dir,
        max_download_mbps = cfg.max_download_mbps,
        max_upload_mbps = cfg.max_upload_mbps,
        "speedtestd 已启动"
    );

    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
    tracing::info!("收到终止信号，正在退出");
}

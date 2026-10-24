//! `speedtestd`：LibreSpeed 服务端（Rust）。
//!
//! 端点在 PLAN §3 固定，`.php` 别名一并保留，第三方 CLI 可以零改动接入。
//! 不提供遥测/结果库（PLAN §0.1），所以没有 `/results/*`。

mod assets;
mod config;
mod garbage;
mod getip;
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
    cfg: Config,
    payload: Payload,
    settlements: Arc<Settlements>,
}

#[derive(serde::Deserialize)]
struct GarbageQuery {
    #[serde(rename = "ckSize")]
    ck_size: Option<String>,
}

async fn garbage(State(app): State<App>, Query(q): Query<GarbageQuery>) -> Response {
    let ck = garbage::parse_ck_size(q.ck_size.as_deref());
    let mut resp = garbage::garbage_response(&app.payload, ck, app.cfg.max_download_mbps);
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
    upload::empty_post(body, app.cfg.max_upload_mbps).await
}

async fn ws_upload(State(app): State<App>, ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    upload::ws_upload(ws, app.settlements.clone(), app.cfg.max_upload_mbps)
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

//! 静态资产。P4 会把 Leptos 产物放到 `web/`，这里只负责正确地把它们发出去：
//! 内容类型（`.wasm` 必须是 `application/wasm`，否则 `instantiateStreaming` 会退回
//! 数组缓冲并拖慢启动）、缓存策略（带内容哈希的文件长缓存，入口 HTML 不缓存）、
//! 以及禁止目录穿越。

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// 只允许普通的相对路径片段，挡掉 `..`、绝对路径和 Windows 盘符。
fn sanitize(rel: &str) -> Option<PathBuf> {
    let candidate = Path::new(rel);
    if candidate
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(candidate.to_path_buf())
}

/// 带内容哈希的资产（如 `index-3f9a1c2b.js` / `app.wasm`）长缓存，其余短缓存。
fn cache_control(path: &Path) -> &'static str {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let hashed = name
        .rsplit_once('-')
        .map(|(_, tail)| tail.split('.').next().unwrap_or(""))
        .map(|hash| hash.len() >= 8 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or(false);
    if hashed || name.ends_with(".wasm") {
        "public, max-age=31536000, immutable"
    } else if name.ends_with(".html") || name.is_empty() {
        "no-cache"
    } else {
        "public, max-age=3600"
    }
}

pub async fn serve(root: PathBuf, rel: String) -> Response {
    let Some(safe) = sanitize(&rel) else {
        return (StatusCode::FORBIDDEN, "bad path").into_response();
    };
    let path = root.join(&safe);
    match tokio::fs::read(&path).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(&path))
            .header(header::CACHE_CONTROL, cache_control(&path))
            .header("x-content-type-options", "nosniff")
            .body(Body::from(bytes))
            .expect("响应构造失败")
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// 入口：没有 `index.html` 时给一个自带说明的占位页，避免部署时一片空白。
pub async fn index(root: Option<PathBuf>) -> Response {
    if let Some(root) = root {
        let p = root.join("index.html");
        if let Ok(bytes) = tokio::fs::read(&p).await {
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(bytes))
                .expect("响应构造失败")
                .into_response();
        }
    }
    let html = concat!(
        "<!doctype html><meta charset=utf-8><title>speedtestd</title>",
        "<h1>speedtestd 正在运行</h1>",
        "<p>还没有前端资产。P4 会把 Leptos 产物放进 <code>web/</code>。</p>",
        "<p>可用的后端端点：<code>/backend/garbage?ckSize=64</code>、",
        "<code>/backend/empty</code>、<code>/backend/getIP</code>、",
        "<code>/backend/ws-upload</code>、<code>/healthz</code></p>"
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(html))
        .expect("响应构造失败")
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(sanitize("../etc/passwd").is_none());
        assert!(sanitize("/etc/passwd").is_none());
        assert!(sanitize("assets/../../secret").is_none());
        assert!(sanitize("assets/app.wasm").is_some());
        assert!(sanitize("index.html").is_some());
    }

    #[test]
    fn wasm_gets_the_right_type_and_cache() {
        let p = Path::new("app.wasm");
        assert_eq!(content_type(p), "application/wasm");
        assert!(cache_control(p).contains("immutable"));

        let hashed = Path::new("assets/index-3f9a1c2b.js");
        assert_eq!(content_type(hashed), "text/javascript; charset=utf-8");
        assert!(cache_control(hashed).contains("immutable"));

        let html = Path::new("index.html");
        assert_eq!(cache_control(html), "no-cache");
    }
}

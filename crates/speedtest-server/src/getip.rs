//! `/backend/getIP`：局域网里只回 IP，ISP/距离字段保留但恒为空。
//!
//! 上游返回 `{processedString, rawIspInfo}`；老客户端（含 speedtest-cli 的某些分支）
//! 会读这两个键，所以字段名必须保留，即使我们不做 GeoIP（PLAN §0.1 已删 mmdb）。

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Serialize)]
pub struct IpInfo {
    /// 展示给用户的字符串，例如 `192.168.1.5` 或 `192.168.1.5 (private)`。
    #[serde(rename = "processedString")]
    pub processed_string: String,
    /// ISP 信息。本方案不做 GeoIP，恒为空字符串（字段保留，见模块注释）。
    #[serde(rename = "rawIspInfo")]
    pub raw_isp_info: String,
}

/// 从 `X-Forwarded-For` → `X-Real-IP` → 连接对端 的顺序取客户端地址。
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<IpAddr> {
    if let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        // 取最左边一个（最接近客户端）
        if let Some(first) = v.split(',').next() {
            if let Ok(ip) = first.trim().parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }
    if let Some(v) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        if let Ok(ip) = v.trim().parse::<IpAddr>() {
            return Some(ip);
        }
    }
    peer.map(|p| p.ip())
}

/// 地址分类，用于在页面上给出人话解释。
pub fn describe(ip: IpAddr) -> &'static str {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                "loopback"
            } else if v4.is_private() {
                "private"
            } else if v4.is_link_local() {
                "link-local"
            } else if v4.is_unspecified() {
                "unspecified"
            } else {
                ""
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                "loopback"
            } else if v6.is_unspecified() {
                "unspecified"
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                "link-local"
            } else if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                // fc00::/7 unique local
                "private"
            } else {
                ""
            }
        }
    }
}

pub async fn get_ip(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    let info = match client_ip(&headers, Some(peer)) {
        Some(ip) => {
            let tag = describe(ip);
            IpInfo {
                processed_string: if tag.is_empty() {
                    ip.to_string()
                } else {
                    format!("{ip} ({tag})")
                },
                raw_isp_info: String::new(),
            }
        }
        None => IpInfo {
            processed_string: "unknown".into(),
            raw_isp_info: String::new(),
        },
    };
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(info)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn prefers_forwarded_for_then_real_ip_then_peer() {
        let peer: SocketAddr = "10.0.0.9:1234".parse().unwrap();

        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.7, 10.0.0.1"),
        );
        assert_eq!(
            client_ip(&h, Some(peer)),
            Some("203.0.113.7".parse().unwrap())
        );

        let mut h = HeaderMap::new();
        h.insert("x-real-ip", HeaderValue::from_static("198.51.100.2"));
        assert_eq!(
            client_ip(&h, Some(peer)),
            Some("198.51.100.2".parse().unwrap())
        );

        let h = HeaderMap::new();
        assert_eq!(client_ip(&h, Some(peer)), Some(peer.ip()));

        // 头部是垃圾时退回 peer，不要 panic
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(client_ip(&h, Some(peer)), Some(peer.ip()));
    }

    #[test]
    fn classifies_lan_addresses() {
        assert_eq!(describe(IpAddr::V4(Ipv4Addr::LOCALHOST)), "loopback");
        assert_eq!(
            describe(IpAddr::V4(Ipv4Addr::new(10, 10, 10, 66))),
            "private"
        );
        assert_eq!(
            describe(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))),
            "link-local"
        );
        assert_eq!(describe(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))), "");
        assert_eq!(describe(IpAddr::V6(Ipv6Addr::LOCALHOST)), "loopback");
        assert_eq!(describe("fd00::1".parse().unwrap()), "private");
        assert_eq!(describe("fe80::1".parse().unwrap()), "link-local");
    }
}

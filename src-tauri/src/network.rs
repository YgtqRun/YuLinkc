//! 网络介质识别（有线/无线）与 HTTP 探活。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GetBestInterface, GET_ADAPTERS_ADDRESSES_FLAGS,
    IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::{IF_OPER_STATUS, IfOperStatusUp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Medium {
    Wireless,
    Wired,
    Other,
}

/// 识别当前承载默认路由的活动网卡介质。
pub fn detect_medium() -> Result<Medium, String> {
    let mut best_index = 0u32;
    // 0.0.0.0 的 GetBestInterface 返回默认路由接口索引
    let best_ret = unsafe { GetBestInterface(0u32, &mut best_index) };
    let mut adapters: Vec<(u32, u32, IF_OPER_STATUS)> = Vec::new();

    let mut size = 16u32 * 1024;
    let mut buf = vec![0u64; (size as usize + 7) / 8];
    let mut ret = ERROR_BUFFER_OVERFLOW.0;
    for _ in 0..3 {
        ret = unsafe {
            GetAdaptersAddresses(
                2, // AF_INET
                GET_ADAPTERS_ADDRESSES_FLAGS(0),
                None,
                Some(buf.as_mut_ptr() as *mut u8 as *mut IP_ADAPTER_ADDRESSES_LH),
                &mut size,
            )
        };
        if ret == ERROR_BUFFER_OVERFLOW.0 {
            let needed = size as usize;
            buf = vec![0u64; (needed + 7) / 8];
            continue;
        }
        break;
    }
    if ret != ERROR_SUCCESS.0 {
        return Err(format!("GetAdaptersAddresses 失败: {ret}"));
    }

    let mut cursor = buf.as_ptr() as *mut u8 as *const IP_ADAPTER_ADDRESSES_LH;
    while !cursor.is_null() {
        let a = unsafe { &*cursor };
        let index = unsafe { a.Anonymous1.Anonymous.IfIndex };
        adapters.push((index, a.IfType, a.OperStatus));
        cursor = a.Next;
    }

    if best_ret == ERROR_SUCCESS.0 {
        if let Some((_, iftype, status)) = adapters.iter().find(|(idx, _, _)| *idx == best_index) {
            if *status == IfOperStatusUp {
                return Ok(medium_of(*iftype));
            }
        }
    }

    // 兜底：选第一个 UP 的以太网/802.11 网卡
    for (_, iftype, status) in adapters {
        if status == IfOperStatusUp && matches!(iftype, IF_TYPE_ETHERNET_CSMACD | IF_TYPE_IEEE80211)
        {
            return Ok(medium_of(iftype));
        }
    }
    Ok(Medium::Other)
}

fn medium_of(iftype: u32) -> Medium {
    match iftype {
        IF_TYPE_IEEE80211 => Medium::Wireless,
        IF_TYPE_ETHERNET_CSMACD => Medium::Wired,
        _ => Medium::Other,
    }
}

/// 执行 HTTP GET（仅 http://），返回 (状态码, 正文前 8KB)。
/// 自动跟随最多 4 次 3xx 重定向。失败返回 None。
pub fn http_get(url: &str, timeout: Duration) -> Option<(u16, String)> {
    http_get_limit(url, timeout, 8192)
}

/// 执行 HTTP GET 并读取最多 `max_bytes` 字节正文，自动跟随最多 4 次 3xx 重定向。
pub fn http_get_limit(
    url: &str,
    timeout: Duration,
    max_bytes: usize,
) -> Option<(u16, String)> {
    let mut current = url.to_string();
    for _hop in 0..5 {
        let Some((host, port, path)) = parse_http_url(&current) else {
            log::warn!("探活 URL 无法解析（仅支持 http）: {current}");
            return None;
        };
        let addr =
            match (host.as_str(), port).to_socket_addrs().ok().and_then(|mut it| it.next()) {
                Some(a) => a,
                None => return None,
            };
        let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => s,
            Err(_) => return None,
        };
        let _ = stream.set_read_timeout(Some(timeout));
        let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        if stream.write_all(req.as_bytes()).is_err() {
            return None;
        }
        let mut body = Vec::with_capacity(4096);
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    body.extend_from_slice(&chunk[..n]);
                    if body.len() >= max_bytes {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        let Some(status) = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|s| s.parse::<u16>().ok())
        else {
            return None;
        };
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if let Some(next) =
                find_header(&text, "location").and_then(|loc| resolve_url(&current, &loc))
            {
                current = next;
                continue;
            }
        }
        return Some((status, text));
    }
    log::warn!("探活 URL 重定向次数过多: {url}");
    None
}

/// 认证页可达性：任何 HTTP 响应（2xx-4xx）都算可达。
pub fn http_ok(url: &str, timeout: Duration) -> bool {
    matches!(http_get(url, timeout), Some((code, _)) if (200..500).contains(&code))
}

/// 外网探活详情：返回 (是否在线, 原因说明)。失败原因会进入日志，便于定位误判。
pub fn external_probe(url: &str, timeout: Duration) -> (bool, String) {
    match http_get(url, timeout) {
        None => (false, format!("连接失败或超时（{timeout:?}）")),
        Some((204, _)) => (true, "HTTP 204".into()),
        Some((200, body)) => {
            if body.contains("Microsoft Connect Test") {
                (true, "HTTP 200 命中探活文本".into())
            } else if body.trim().is_empty() {
                (true, "HTTP 200 空正文".into())
            } else {
                (
                    false,
                    format!("HTTP 200 但正文非预期（{}）", first_text_snippet(&body)),
                )
            }
        }
        Some((code, body)) => (
            false,
            format!("HTTP {code}（{}）", first_text_snippet(&body)),
        ),
    }
}

/// 取正文第一行非空内容（截断，用于日志）。
fn first_text_snippet(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}

/// 大小写不敏感地查找响应头字段（请求/响应行之外的 Header 行）。
fn find_header(text: &str, name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    text.lines()
        .skip(1)
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            if key.trim().to_ascii_lowercase() == lower {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
}

/// 解析 Location 头：绝对 http:// 直接使用；其余基于当前 URL 简单拼接。
fn resolve_url(base: &str, location: &str) -> Option<String> {
    if location.starts_with("http://") {
        return Some(location.to_string());
    }
    let (host, port, path) = parse_http_url(base)?;
    let authority = if port == 80 {
        host
    } else {
        format!("{host}:{port}")
    };
    let origin = format!("http://{authority}");
    if location.starts_with('/') {
        Some(format!("{origin}{location}"))
    } else {
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        Some(format!("{origin}{dir}/{location}"))
    }
}

fn parse_http_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host_port, path) = match rest.split_once('/') {
        Some((hp, p)) => (hp, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => {
            (h.to_string(), p.parse::<u16>().ok()?)
        }
        _ => (host_port.to_string(), 80u16),
    };
    Some((host, port, path))
}

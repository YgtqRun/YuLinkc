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

/// 执行 HTTP GET（仅 http://），返回 (状态码, 正文前 8KB)。失败返回 None。
pub fn http_get(url: &str, timeout: Duration) -> Option<(u16, String)> {
    let Some((host, port, path)) = parse_http_url(url) else {
        log::warn!("探活 URL 无法解析（仅支持 http）: {url}");
        return None;
    };
    let addr = match (host.as_str(), port).to_socket_addrs().ok().and_then(|mut it| it.next()) {
        Some(a) => a,
        None => return None,
    };
    let mut stream = match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => s,
        Err(_) => return None,
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
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
                if body.len() >= 8192 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok());
    status.map(|code| (code, text))
}

/// 认证页可达性：任何 HTTP 响应（2xx-4xx）都算可达。
pub fn http_ok(url: &str, timeout: Duration) -> bool {
    matches!(http_get(url, timeout), Some((code, _)) if (200..500).contains(&code))
}

/// 外网探活：不能只认"有响应"——认证前的劫持页也会回 200。
/// 默认探活地址是微软连通性测试页，需正文命中；204 空响应也算通过。
pub fn external_ok(url: &str, timeout: Duration) -> bool {
    match http_get(url, timeout) {
        Some((204, _)) => true,
        Some((200, body)) => {
            body.contains("Microsoft Connect Test") || body.trim().is_empty()
        }
        _ => false,
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

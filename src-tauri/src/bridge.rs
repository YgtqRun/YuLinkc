//! 页面 → Rust 的本地回传通道。
//!
//! Rust 在 127.0.0.1 随机端口起一次性 HTTP 监听，认证页通过
//! `new Image().src = http://127.0.0.1:<port>/report?run=&state=&msg=` 上报结果；
//! Image 信标跨域不受 CORS 限制，HTTP 页面可用。服务端解析一行 GET 即返回 204。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct BeaconEvent {
    pub run: u64,
    pub state: String,
    pub msg: String,
}

/// 启动信标监听线程，返回实际监听端口。
pub fn spawn_beacon_listener(tx: Sender<BeaconEvent>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut s) = stream {
                let event = read_request_head(&mut s).and_then(|line| parse_beacon_line(&line));
                if let Some(ev) = event {
                    let _ = tx.send(ev);
                }
                let _ = s.write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        }
    });
    Ok(port)
}

/// 读取一个 HTTP 请求的请求行（读到头结束或超时为止）。
pub(crate) fn read_request_head(stream: &mut TcpStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if buf.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    text.lines().next().map(|s| s.to_string())
}

fn parse_beacon_line(request_line: &str) -> Option<BeaconEvent> {
    let mut parts = request_line.split_whitespace();
    if !matches!(parts.next()?, "GET" | "POST") {
        return None;
    }
    let target = parts.next()?;
    let path = target.split('?').next()?;
    if path != "/report" {
        return None;
    }
    let query = target.split_once('?')?.1;
    let mut run = 0u64;
    let mut state = String::new();
    let mut msg = String::new();
    for kv in query.split('&') {
        let (key, value) = kv.split_once('=').unwrap_or((kv, ""));
        match key {
            "run" => run = value.parse().unwrap_or(0),
            "state" => state = percent_decode(value),
            "msg" => msg = percent_decode(value),
            _ => {}
        }
    }
    if state.is_empty() {
        return None;
    }
    Some(BeaconEvent { run, state, msg })
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

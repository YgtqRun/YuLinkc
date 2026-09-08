//! 本地 mock 认证页服务（开发与 E2E 测试用）。

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;

use crate::bridge::read_request_head;

const WIRELESS_HTML: &str = include_str!("../../mock/wireless.html");
const WIRED_HTML: &str = include_str!("../../mock/wired.html");

pub fn spawn_mock_server() -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(mut s) = stream {
                let _ = serve_mock_connection(&mut s);
            }
        }
    });
    Ok(port)
}

fn serve_mock_connection(stream: &mut TcpStream) -> std::io::Result<()> {
    let Some(request_line) = read_request_head(stream) else {
        return Ok(());
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    println!("[mock] {method} {target}");
    if method != "GET" {
        return write_response(stream, 405, "Method Not Allowed", b"");
    }
    let path = target.split('?').next().unwrap_or("/");
    let (status, reason, body): (u16, &str, &[u8]) = match path {
        "/" | "/index.html" | "/wireless.html" => (200, "OK", WIRELESS_HTML.as_bytes()),
        "/wired.html" => (200, "OK", WIRED_HTML.as_bytes()),
        "/ext-ok" => (204, "No Content", b""),
        _ => (404, "Not Found", b"not found"),
    };
    write_response(stream, status, reason, body)
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)
}

//! P0 POC：屏幕外可见窗口 + eval 注入 + mock 认证页 + Image 信标回传
//!
//! 运行方式（PowerShell）：
//!   $env:YULINK_POC=1; $env:YULINK_POC_MODE="all"; npm run tauri dev
//!
//! 环境变量：
//!   YULINK_POC          = 1 启用 POC（未设置时走正常应用逻辑）
//!   YULINK_POC_MODE     = success | fail | captcha | wired | all
//!   YULINK_POC_RUNS     = 每轮循环次数（用于创建/销毁压力测试）
//!   YULINK_AUTH_VISIBLE = 0 强制隐藏 / 1 强制显示认证窗口（详见 auth_window.rs）

use std::{
    env,
    io::Write,
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use crate::bridge::{read_request_head, spawn_beacon_listener, BeaconEvent};
use crate::auth_window::{
    cleanup_auth_windows, create_auth_window, destroy_auth_window, AuthWindowVisibility,
};
use tauri::{AppHandle, Manager};

const WIRELESS_HTML: &str = include_str!("../../mock/wireless.html");
const WIRED_HTML: &str = include_str!("../../mock/wired.html");
const AUTH_JS_TEMPLATE: &str = include_str!("../../assets/auth/auth.js");

const CFG_TOKEN: &str = "__YL_CFG__";

// ===================== 本地 mock HTTP 服务 =====================

fn spawn_mock_server() -> std::io::Result<u16> {
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

// ===================== POC 主流程 =====================

struct Scenario {
    name: &'static str,
    path: String,
    sms_code: &'static str,
    expected: &'static str,
}

pub fn run_poc(app: &AppHandle) -> Result<(), String> {
    let mock_port = spawn_mock_server().map_err(|e| format!("mock 服务启动失败: {e}"))?;
    let (beacon_tx, beacon_rx) = mpsc::channel::<BeaconEvent>();
    let beacon_port = spawn_beacon_listener(beacon_tx)
        .map_err(|e| format!("信标服务启动失败: {e}"))?;

    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }

    let visibility = AuthWindowVisibility::resolve();
    println!("[POC] 认证窗口显示策略: {}", visibility.describe());

    let mode = env::var("YULINK_POC_MODE").unwrap_or_else(|_| "all".to_string());
    let repeat = env::var("YULINK_POC_RUNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);

    let scenarios: Vec<Scenario> = match mode.as_str() {
        "success" => vec![Scenario {
            name: "无线-成功",
            path: "/wireless.html?result=success".into(),
            sms_code: "123456",
            expected: "ok",
        }],
        "fail" => vec![Scenario {
            name: "无线-认证失败",
            path: "/wireless.html?result=fail".into(),
            sms_code: "123456",
            expected: "failed",
        }],
        "captcha" => vec![Scenario {
            name: "无线-动态密码错误",
            path: "/wireless.html?result=success&badcode=1".into(),
            sms_code: "000000",
            expected: "captcha-error",
        }],
        "wired" => vec![Scenario {
            name: "有线-成功",
            path: "/wired.html?result=success".into(),
            sms_code: "123456",
            expected: "ok",
        }],
        "all" => vec![
            Scenario {
                name: "无线-成功",
                path: "/wireless.html?result=success".into(),
                sms_code: "123456",
                expected: "ok",
            },
            Scenario {
                name: "无线-认证失败",
                path: "/wireless.html?result=fail".into(),
                sms_code: "123456",
                expected: "failed",
            },
            Scenario {
                name: "无线-动态密码错误",
                path: "/wireless.html?result=success&badcode=1".into(),
                sms_code: "000000",
                expected: "captcha-error",
            },
            Scenario {
                name: "有线-成功",
                path: "/wired.html?result=success".into(),
                sms_code: "123456",
                expected: "ok",
            },
        ],
        other => return Err(format!("未知的 YULINK_POC_MODE: {other}")),
    };

    let base_url = format!("http://127.0.0.1:{mock_port}");
    let beacon = format!("http://127.0.0.1:{beacon_port}/report");
    println!(
        "[POC] mock={base_url} beacon={beacon} mode={mode} repeat={repeat} 场景数={}",
        scenarios.len()
    );

    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut run_id = 1u64;

    for round in 1..=repeat {
        for scenario in &scenarios {
            println!(
                "[POC] run#{run_id} ({round}/{repeat}) {}：期望结果 {expected}",
                scenario.name,
                expected = scenario.expected
            );
            match run_single(
                app,
                &beacon_rx,
                &base_url,
                &beacon,
                scenario,
                run_id,
                visibility,
            ) {
                Ok(actual) => {
                    if actual == scenario.expected {
                        passed += 1;
                        println!("[POC] run#{run_id} 结果: PASS（收到 {actual}）");
                    } else {
                        failed += 1;
                        println!(
                            "[POC] run#{run_id} 结果: FAIL（期望 {}，收到 {actual}）",
                            scenario.expected
                        );
                    }
                }
                Err(e) => {
                    failed += 1;
                    println!("[POC] run#{run_id} 结果: ERROR {e}");
                }
            }
            run_id += 1;
        }
    }

    cleanup_auth_windows(app, "auth-");
    println!("[POC] ===== 汇总：{passed} 通过，{failed} 失败 =====");
    if failed == 0 {
        Ok(())
    } else {
        Err(format!("{failed} 个场景未通过"))
    }
}

fn run_single(
    app: &AppHandle,
    rx: &Receiver<BeaconEvent>,
    base_url: &str,
    beacon: &str,
    scenario: &Scenario,
    run_id: u64,
    visibility: AuthWindowVisibility,
) -> Result<String, String> {
    cleanup_auth_windows(app, "auth-");
    let label = format!("auth-{run_id}");
    let url = format!("{base_url}{}", scenario.path);
    println!("[POC] run#{run_id} 创建认证窗口({label}) -> {url}");
    create_auth_window(app, &label, &url, visibility)?;

    let cfg = serde_json::json!({
        "account": "20240001",
        "password": "demo-pass",
        "isp": "2",
        "smsCode": scenario.sms_code,
        "beacon": beacon,
        "run": run_id,
        "timeoutMs": 15000,
        "pollMs": 500,
        "okWaitMs": 3500,
        "okWaitCount": 10
    });
    let js = AUTH_JS_TEMPLATE.replace(CFG_TOKEN, &cfg.to_string());

    let eval_period = Duration::from_millis(400);
    let eval_window = Duration::from_secs(30);
    let terminal_window = Duration::from_secs(25);
    let mut last_eval = Instant::now() - eval_period;
    let eval_deadline = Instant::now() + eval_window;
    let mut evals = 0u32;
    let mut started_at: Option<Instant> = None;
    let mut last_url_log = Instant::now();

    loop {
        // 先消费本窗口回传的事件
        while let Ok(ev) = rx.try_recv() {
            if ev.run != run_id {
                println!(
                    "[POC] run#{run_id} 忽略旧事件 run#{} state={}",
                    ev.run, ev.state
                );
                continue;
            }
            println!("[POC] run#{run_id} beacon: {} {}", ev.state, ev.msg);
            match ev.state.as_str() {
                "probe" => {}
                "started" => {
                    if started_at.is_none() {
                        started_at = Some(Instant::now());
                        println!("[POC] run#{run_id} 注入确认：eval 已在认证窗口执行");
                    }
                }
                "filled" => {}
                // 页面跳转（登录成功重定向）视为成功
                "navigating" => {
                    destroy_auth_window(app, &label);
                    return Ok("ok".to_string());
                }
                terminal @ ("ok" | "failed" | "captcha-error" | "error") => {
                    destroy_auth_window(app, &label);
                    return Ok(terminal.to_string());
                }
                _ => {}
            }
        }

        match started_at {
            Some(t0) => {
                if t0.elapsed() > terminal_window {
                    destroy_auth_window(app, &label);
                    return Ok("timeout(rust)".to_string());
                }
            }
            None => {
                if Instant::now() > eval_deadline {
                    destroy_auth_window(app, &label);
                    return Ok("eval-no-response".to_string());
                }
                if last_url_log.elapsed() >= Duration::from_secs(2) {
                    last_url_log = Instant::now();
                    if let Some(win) = app.get_webview_window(&label) {
                        if let Ok(cur) = win.url() {
                            println!("[POC] run#{run_id} 当前 URL: {cur}");
                        }
                    }
                }
                if last_eval.elapsed() >= eval_period {
                    last_eval = Instant::now();
                    evals += 1;
                    if evals <= 3 || evals % 10 == 0 {
                        println!("[POC] run#{run_id} 第 {evals} 次注入尝试…");
                    }
                    let win = app
                        .get_webview_window(&label)
                        .ok_or_else(|| "认证窗口已不存在".to_string())?;
                    if let Err(e) = win.eval(&js) {
                        println!("[POC] run#{run_id} eval 第 {evals} 次失败: {e}");
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

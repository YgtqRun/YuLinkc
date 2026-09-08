//! 认证登录会话：创建认证窗口 → 注入脚本 → 等待信标结果 → 销毁窗口。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::auth_window::{
    cleanup_auth_windows, create_auth_window, destroy_auth_window, AuthWindowVisibility,
};
use crate::bridge::{spawn_beacon_listener, BeaconEvent};
use crate::inject::build_auth_js;

static SESSION_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcomeKind {
    Ok,
    CredentialsFailed(String),
    SmsCodeFailed(String),
    PageError(String),
    Timeout,
    EvalNoResponse,
}

#[derive(Debug, Clone)]
pub struct LoginOutcome {
    pub kind: LoginOutcomeKind,
}

impl LoginOutcome {
    pub fn is_ok(&self) -> bool {
        self.kind == LoginOutcomeKind::Ok
    }

    pub fn message(&self) -> String {
        match &self.kind {
            LoginOutcomeKind::Ok => "登录成功".into(),
            LoginOutcomeKind::CredentialsFailed(msg) => format!("认证失败：{msg}"),
            LoginOutcomeKind::SmsCodeFailed(msg) => format!("动态密码错误：{msg}"),
            LoginOutcomeKind::PageError(msg) => format!("认证页异常：{msg}"),
            LoginOutcomeKind::Timeout => "登录超时".into(),
            LoginOutcomeKind::EvalNoResponse => "注入脚本无响应".into(),
        }
    }
}

/// 执行一次认证登录。凭据由调用方解密后传入，页面侧不落盘。
pub fn run_login(
    app: &AppHandle,
    portal_url: &str,
    account: &str,
    password: &str,
    sms_code: &str,
    selectors: &crate::store::Selectors,
    visibility: AuthWindowVisibility,
) -> Result<LoginOutcome, String> {
    cleanup_auth_windows(app, "auth-");

    let run_id = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    let label = format!("auth-{run_id}");
    let (beacon_tx, beacon_rx) = mpsc::channel::<BeaconEvent>();
    let beacon_port = spawn_beacon_listener(beacon_tx)
        .map_err(|e| format!("信标服务启动失败: {e}"))?;
    let beacon = format!("http://127.0.0.1:{beacon_port}/report");

    log::info!("开始登录会话 run#{run_id} -> {portal_url}");
    create_auth_window(app, &label, portal_url, visibility)?;

    let cfg = serde_json::json!({
        "account": account,
        "password": password,
        "isp": "2",
        "smsCode": sms_code,
        "beacon": beacon,
        "run": run_id,
        "timeoutMs": 15000,
        "pollMs": 1000,
        "clickDelayMs": 1200,
        "okWaitCount": 10,
        "selectors": selectors
    });
    let js = build_auth_js(&cfg);

    let eval_period = Duration::from_millis(400);
    let eval_window = Duration::from_secs(20);
    let terminal_window = Duration::from_secs(30);
    let mut last_eval = Instant::now() - eval_period;
    let eval_deadline = Instant::now() + eval_window;
    let mut evals = 0u32;
    let mut started_at: Option<Instant> = None;

    let outcome = 'outer: loop {
        while let Ok(ev) = beacon_rx.try_recv() {
            if ev.run != run_id {
                continue;
            }
            log::info!("run#{run_id} beacon: {} {}", ev.state, ev.msg);
            match ev.state.as_str() {
                "started" => {
                    started_at.get_or_insert_with(Instant::now);
                }
                "filled" => {}
                "ok" => break 'outer LoginOutcome {
                    kind: LoginOutcomeKind::Ok,
                },
                "failed" => break 'outer LoginOutcome {
                    kind: LoginOutcomeKind::CredentialsFailed(ev.msg),
                },
                "captcha-error" => break 'outer LoginOutcome {
                    kind: LoginOutcomeKind::SmsCodeFailed(ev.msg),
                },
                "error" => break 'outer LoginOutcome {
                    kind: LoginOutcomeKind::PageError(ev.msg),
                },
                _ => {}
            }
        }

        match started_at {
            Some(t0) => {
                if t0.elapsed() > terminal_window {
                    break LoginOutcome {
                        kind: LoginOutcomeKind::Timeout,
                    };
                }
            }
            None => {
                if Instant::now() > eval_deadline {
                    break LoginOutcome {
                        kind: LoginOutcomeKind::EvalNoResponse,
                    };
                }
                if last_eval.elapsed() >= eval_period {
                    last_eval = Instant::now();
                    evals += 1;
                    let win = app
                        .get_webview_window(&label)
                        .ok_or_else(|| "认证窗口已不存在".to_string())?;
                    if let Err(e) = win.eval(&js) {
                        log::warn!("run#{run_id} eval 第 {evals} 次失败: {e}");
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    destroy_auth_window(app, &label);
    log::info!("run#{run_id} 结束: {}", outcome.message());
    Ok(outcome)
}

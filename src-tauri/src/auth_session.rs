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
    /// 页面没有登录表单但检测到“注销”按钮：会话已在线，无需重复登录。
    AlreadyOnline,
    /// 仅检测模式下发现登录表单：需要登录，但按设置不执行登录。
    LoginFormPresent,
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
            LoginOutcomeKind::AlreadyOnline => "认证页显示已登录（检测到注销按钮）".into(),
            LoginOutcomeKind::LoginFormPresent => {
                "认证页需要登录（检测模式，未执行登录）".into()
            }
            LoginOutcomeKind::CredentialsFailed(msg) => format!("认证失败：{msg}"),
            LoginOutcomeKind::SmsCodeFailed(msg) => format!("动态密码错误：{msg}"),
            LoginOutcomeKind::PageError(msg) => format!("认证页异常：{msg}"),
            LoginOutcomeKind::Timeout => "登录超时".into(),
            LoginOutcomeKind::EvalNoResponse => {
                "认证页无响应（网络不可达或门户不可访问）".into()
            }
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
    detect_only: bool,
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
        "detectOnly": detect_only,
        "selectors": selectors
    });
    let js = build_auth_js(&cfg);

    let eval_period = Duration::from_millis(400);
    let eval_window = Duration::from_secs(20);
    let terminal_window = Duration::from_secs(30);
    let mut last_eval = Instant::now() - eval_period;
    let mut eval_deadline = Instant::now() + eval_window;
    let mut evals = 0u32;
    let mut started_at: Option<Instant> = None;
    // 整个会话最多“刷新页面重试一次”：处理门户缓存/中间页导致表单不出现，
    // 以及 started 后文档被替换造成的静默超时。
    let mut refreshed = false;

    let outcome = 'outer: loop {
        let mut terminal: Option<LoginOutcome> = None;

        while let Ok(ev) = beacon_rx.try_recv() {
            if ev.run != run_id {
                continue;
            }
            log::info!("run#{run_id} beacon: {} {}", ev.state, ev.msg);
            terminal = match ev.state.as_str() {
                "started" => {
                    started_at.get_or_insert_with(Instant::now);
                    None
                }
                "filled" => None,
                // 页面整页跳转（登录成功后的重定向）也算成功
                "ok" | "navigating" => Some(LoginOutcome {
                    kind: LoginOutcomeKind::Ok,
                }),
                // 没有登录表单但存在注销按钮 → 已登录，不需要重复登录
                "online" => Some(LoginOutcome {
                    kind: LoginOutcomeKind::AlreadyOnline,
                }),
                // 仅检测模式：登录表单存在 → 需要登录，但设置已关闭自动登录
                "form-present" => Some(LoginOutcome {
                    kind: LoginOutcomeKind::LoginFormPresent,
                }),
                "failed" => Some(LoginOutcome {
                    kind: LoginOutcomeKind::CredentialsFailed(ev.msg),
                }),
                "captcha-error" => Some(LoginOutcome {
                    kind: LoginOutcomeKind::SmsCodeFailed(ev.msg),
                }),
                "error" => Some(LoginOutcome {
                    kind: LoginOutcomeKind::PageError(ev.msg),
                }),
                _ => None,
            };
            if terminal.is_some() {
                break;
            }
        }

        // 页面给出终态但属于“表单没等到/整轮无结果”时，先刷新一次再试。
        if let Some(o) = terminal {
            if !refreshed && login_outcome_needs_reload(&o) {
                log::warn!("run#{run_id} {}，刷新认证页后重试一次", o.message());
                refreshed = true;
                reload_auth_page(&app, &label);
                started_at = None;
                eval_deadline = Instant::now() + eval_window;
                last_eval = Instant::now() - eval_period;
                continue;
            }
            break 'outer o;
        }

        match started_at {
            Some(t0) => {
                if t0.elapsed() > terminal_window {
                    let outcome = LoginOutcome {
                        kind: LoginOutcomeKind::Timeout,
                    };
                    if !refreshed {
                        log::warn!("run#{run_id} 等待页面结果超时，刷新认证页后重试一次");
                        refreshed = true;
                        reload_auth_page(&app, &label);
                        started_at = None;
                        eval_deadline = Instant::now() + eval_window;
                        last_eval = Instant::now() - eval_period;
                        continue;
                    }
                    break 'outer outcome;
                }
            }
            None => {
                if Instant::now() > eval_deadline {
                    let outcome = LoginOutcome {
                        kind: LoginOutcomeKind::EvalNoResponse,
                    };
                    if !refreshed {
                        log::warn!("run#{run_id} 注入脚本始终无响应，刷新认证页后重试一次");
                        refreshed = true;
                        reload_auth_page(&app, &label);
                        eval_deadline = Instant::now() + eval_window;
                        last_eval = Instant::now() - eval_period;
                        continue;
                    }
                    break 'outer outcome;
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

/// 哪些终态值得“刷新一次再试”：认证页没等到登录表单，或整轮没有结果。
/// 账号/动态密码错误（failed / captcha-error）是真实业务结果，不刷新。
fn login_outcome_needs_reload(o: &LoginOutcome) -> bool {
    match &o.kind {
        LoginOutcomeKind::PageError(msg) => msg.starts_with("timeout:"),
        LoginOutcomeKind::Timeout => true,
        _ => false,
    }
}

/// 让认证窗口执行一次整页刷新（导航到新文档后注入脚本的幂等守卫会复位）。
fn reload_auth_page(app: &AppHandle, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.eval("location.reload(); true;");
    }
}

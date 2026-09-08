//! 登录调度器：周期性探活、断线自动重登、即时登录触发。

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::auth_session::run_login;
use crate::auth_window::AuthWindowVisibility;
use crate::network;
use crate::state::RuntimeState;
use crate::store::ConfigState;

const EXTERNAL_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const PORTAL_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

pub enum SchedMessage {
    LoginNow,
}

/// 供托盘/命令持有的调度器句柄。
#[derive(Clone)]
pub struct Scheduler {
    tx: Sender<SchedMessage>,
}

impl Scheduler {
    pub fn request_login(&self) {
        let _ = self.tx.send(SchedMessage::LoginNow);
    }
}

pub fn spawn(app: &AppHandle) -> Scheduler {
    let (tx, rx) = mpsc::channel::<SchedMessage>();
    let app2 = app.clone();
    thread::spawn(move || {
        let mut force_login = false;
        loop {
            let wait_secs = check_once(&app2, force_login);
            force_login = false;
            match rx.recv_timeout(Duration::from_secs(wait_secs)) {
                Ok(SchedMessage::LoginNow) => force_login = true,
                Err(RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
        }
    });
    Scheduler { tx }
}

/// 执行一轮巡检，返回下一次巡检间隔（秒）。
/// `force_login=true` 时跳过"外网通即已连接"的快捷判断，直接尝试登录（立即登录按钮）。
fn check_once(app: &AppHandle, force_login: bool) -> u64 {
    let cfg = match app.state::<ConfigState>().0.lock() {
        Ok(g) => g.clone(),
        Err(_) => return 30,
    };
    let state = app.state::<RuntimeState>();
    let prefs = &cfg.preferences;

    if cfg.account.is_none() {
        state.set(app, "unconfigured", "未配置账号，点击托盘打开设置".into());
        return 60;
    }

    // 外网通 → 视为已连接，无需登录（立即登录请求除外）
    if !force_login
        && network::external_ok(&prefs.external_probe_url, EXTERNAL_PROBE_TIMEOUT)
    {
        state.set(app, "connected", "已连接".into());
        return prefs.check_interval_online_sec.max(10);
    }

    // 选择认证页：先按介质排序，再按可达性挑第一个能响应的
    let medium = network::detect_medium().unwrap_or(network::Medium::Other);
    let candidates = portal_candidates(medium, prefs);
    let portal = candidates
        .iter()
        .find(|url| network::http_ok(url, PORTAL_PROBE_TIMEOUT))
        .cloned();
    if force_login {
        log::info!(
            "手动触发登录（介质: {}, 候选认证页: {}）",
            medium_display(medium),
            candidates.join(", ")
        );
    }
    let Some(portal) = portal else {
        state.set(
            app,
            "network-down",
            format!("认证页不可达（已尝试 {}），等待网络恢复", candidates.join(" / ")),
        );
        return prefs.check_interval_offline_sec.max(10);
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if !cfg.sms_valid_at(now) {
        state.set(app, "needs-sms", "动态密码缺失或已过期".into());
        return 60;
    }

    let Some((account, password)) = cfg.account_password() else {
        state.set(app, "unconfigured", "账号配置读取失败".into());
        return 60;
    };
    let Some(sms_code) = cfg.sms_code() else {
        state.set(app, "needs-sms", "动态密码缺失或已过期".into());
        return 60;
    };

    state.set(app, "logging-in", "正在自动登录".into());
    let outcome = run_login(
        app,
        &portal,
        &account,
        &password,
        &sms_code,
        &cfg.selectors,
        AuthWindowVisibility::resolve(),
    );

    match outcome {
        Ok(o) if o.is_ok() => {
            log::info!("认证页确认登录成功（{}）", o.message());
            let confirmed = mock_portal_mode()
                || external_restored(app, &prefs.external_probe_url);
            if confirmed {
                state.set(app, "connected", "已连接".into());
                return prefs.check_interval_online_sec.max(10);
            }
            // 与登录脚本一致：页面已判定成功；外网确认只是补充信息
            state.set(app, "connected", "已连接（等待外网确认）".into());
            return 20;
        }
        Ok(o) => {
            // 与登录脚本一致：失败不清除动态密码，有效期内可重试
            state.set(
                app,
                "failed",
                format!("{}（动态密码保留，可在有效期内重试）", o.message()),
            );
        }
        Err(e) => {
            state.set(app, "failed", format!("登录会话异常：{e}（将自动重试）"));
        }
    }
    prefs.check_interval_offline_sec.max(10)
}

/// 登录成功后重试外网探测（最多 3 次）。
fn external_restored(app: &AppHandle, probe_url: &str) -> bool {
    let state = app.state::<RuntimeState>();
    for i in 1..=3 {
        state.set(app, "logging-in", format!("正在确认外网恢复（{i}/3）"));
        if network::external_ok(probe_url, EXTERNAL_PROBE_TIMEOUT) {
            return true;
        }
        thread::sleep(Duration::from_secs(2));
    }
    false
}

fn mock_portal_mode() -> bool {
    std::env::var("YULINK_MOCK_PORTAL")
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn portal_candidates(
    medium: network::Medium,
    prefs: &crate::store::Preferences,
) -> Vec<String> {
    let wireless = prefs.portal_wireless.clone();
    let wired = prefs.portal_wired.clone();
    match medium {
        network::Medium::Wireless => vec![wireless, wired],
        network::Medium::Wired => vec![wired, wireless],
        network::Medium::Other => vec![wireless, wired],
    }
}

fn medium_display(medium: network::Medium) -> &'static str {
    match medium {
        network::Medium::Wireless => "无线",
        network::Medium::Wired => "有线",
        network::Medium::Other => "其他",
    }
}

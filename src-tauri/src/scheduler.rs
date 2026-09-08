//! 登录调度器：周期性探活、断线自动重登、即时登录触发。

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::auth_session::run_login;
use crate::auth_window::AuthWindowVisibility;
use crate::network;
use crate::state::RuntimeState;
use crate::store::{ConfigPaths, ConfigState};

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
    thread::spawn(move || loop {
        let wait_secs = check_once(&app2);
        match rx.recv_timeout(Duration::from_secs(wait_secs)) {
            Err(RecvTimeoutError::Disconnected) => break,
            _ => {}
        }
    });
    Scheduler { tx }
}

/// 执行一轮巡检，返回下一次巡检间隔（秒）。
fn check_once(app: &AppHandle) -> u64 {
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

    // 外网通 → 视为已连接，无需登录
    if network::http_ok(&prefs.external_probe_url, EXTERNAL_PROBE_TIMEOUT) {
        state.set(app, "connected", "已连接".into());
        return prefs.check_interval_online_sec.max(10);
    }

    // 外网不通：确认介质与认证页
    let medium = network::detect_medium().unwrap_or(network::Medium::Other);
    let portal = medium.portal_url(prefs);
    if !network::http_ok(&portal, PORTAL_PROBE_TIMEOUT) {
        state.set(app, "network-down", "认证页不可达，等待网络恢复".into());
        return prefs.check_interval_offline_sec.max(10);
    }

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
        AuthWindowVisibility::resolve(),
    );

    match outcome {
        Ok(o) if o.is_ok() => {
            if mock_portal_mode() || external_restored(app, &prefs.external_probe_url) {
                state.set(app, "connected", "已连接".into());
                return prefs.check_interval_online_sec.max(10);
            }
            state.set(app, "failed", "登录已提交，但外网未恢复".into());
        }
        Ok(o) if o.invalidates_sms() => {
            // 门户明确报动态密码错误 → 立即作废并请求补码
            if let Ok(mut cfg_guard) = app.state::<ConfigState>().0.lock() {
                cfg_guard.sms_code = None;
                let _ = cfg_guard.save(&app.state::<ConfigPaths>().dir);
            }
            state.set(app, "needs-sms", format!("动态密码错误，请重新获取：{}", o.message()));
            return 60;
        }
        Ok(o) => {
            state.set(
                app,
                "failed",
                format!("{}（将自动重试）", o.message()),
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
        if network::http_ok(probe_url, EXTERNAL_PROBE_TIMEOUT) {
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

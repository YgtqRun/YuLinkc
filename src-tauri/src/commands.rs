//! Vue 界面调用的 Tauri 命令：设置读写（DPAPI 解密后返回给自有界面）、状态查询、
//! 开机自启、立即登录（M3 前为占位）。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::store::{
    AppConfig, ConfigPaths, ConfigState, PERMANENT_HOURS,
};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub account_configured: bool,
    pub username: String,
    pub password: String,
    pub isp: String,
    pub has_sms_code: bool,
    pub sms_code: String,
    pub sms_hours: f64,
    pub sms_saved_at: i64,
    pub sms_status_text: String,
    pub autostart: bool,
    pub status: StatusView,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    /// unconfigured | needs-sms | ready | login-in-progress | connected | failed
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub username: String,
    pub password: String,
    pub isp: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsInput {
    pub code: String,
    /// 小时数；-1 表示永久
    pub hours: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsRequest {
    /// 提供时整体覆盖账号；留空字段表示不修改账号
    pub account: Option<AccountInput>,
    /// 提供且 code 非空时覆盖动态密码；否则仅更新有效期设定（或保留原码）
    pub sms: Option<SmsInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub ok: bool,
    pub message: String,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn build_view(cfg: &AppConfig) -> SettingsView {
    let now = now_ms();
    let account = cfg.account.as_ref();
    let (username, password) = cfg.account_password().unwrap_or_default();
    let sms = cfg.sms_code();
    let account_configured = cfg.account.is_some();
    let has_sms_code = sms.is_some();

    let kind = if !account_configured {
        "unconfigured"
    } else if !cfg.sms_valid_at(now) {
        "needs-sms"
    } else {
        "ready"
    };
    let text = match kind {
        "unconfigured" => "未配置账号".to_string(),
        "needs-sms" => "动态密码缺失或已过期".to_string(),
        _ => "已就绪，可自动登录".to_string(),
    };

    SettingsView {
        account_configured,
        username,
        password,
        isp: account.map(|a| a.isp.clone()).unwrap_or_else(|| "2".into()),
        has_sms_code,
        sms_code: sms.unwrap_or_default(),
        sms_hours: cfg
            .sms_code
            .as_ref()
            .map(|s| s.hours)
            .unwrap_or(27.0),
        sms_saved_at: cfg.sms_code.as_ref().map(|s| s.saved_at).unwrap_or(0),
        sms_status_text: cfg.sms_status_text(now),
        autostart: cfg.preferences.autostart,
        status: StatusView {
            kind: kind.to_string(),
            text,
        },
    }
}

fn save_config(
    cfg: &AppConfig,
    paths: &ConfigPaths,
    config_state: &ConfigState,
) -> Result<(), String> {
    cfg.save(&paths.dir)?;
    if let Ok(mut guard) = config_state.0.lock() {
        *guard = cfg.clone();
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings(
    config_state: State<'_, ConfigState>,
) -> Result<SettingsView, String> {
    log::info!("设置界面读取配置");
    let cfg = config_state
        .0
        .lock()
        .map_err(|_| "配置状态锁不可用".to_string())?;
    Ok(build_view(&cfg))
}

#[tauri::command]
pub fn save_settings(
    req: SaveSettingsRequest,
    paths: State<'_, ConfigPaths>,
    config_state: State<'_, ConfigState>,
) -> Result<SettingsView, String> {
    let mut cfg = config_state
        .0
        .lock()
        .map_err(|_| "配置状态锁不可用".to_string())?
        .clone();

    if let Some(acc) = req.account {
        let username = acc.username.trim();
        if username.is_empty() || acc.password.is_empty() {
            return Err("账号与密码需同时填写".into());
        }
        cfg.set_account(username, &acc.password, &acc.isp)?;
    }

    if let Some(sms) = req.sms {
        let code = sms.code.trim();
        if !code.is_empty() {
            if !(sms.hours > 0.0 || sms.hours == PERMANENT_HOURS) {
                return Err("有效期需大于 0 小时，或选择永久".into());
            }
            cfg.set_sms_code(code, now_ms(), sms.hours)?;
        }
    }

    save_config(&cfg, &paths, &config_state)?;
    Ok(build_view(&cfg))
}

#[tauri::command]
pub fn clear_sms_code(
    paths: State<'_, ConfigPaths>,
    config_state: State<'_, ConfigState>,
) -> Result<SettingsView, String> {
    let mut cfg = config_state
        .0
        .lock()
        .map_err(|_| "配置状态锁不可用".to_string())?
        .clone();
    cfg.sms_code = None;
    save_config(&cfg, &paths, &config_state)?;
    Ok(build_view(&cfg))
}

#[tauri::command]
pub fn clear_all(
    paths: State<'_, ConfigPaths>,
    config_state: State<'_, ConfigState>,
) -> Result<SettingsView, String> {
    let mut cfg = config_state
        .0
        .lock()
        .map_err(|_| "配置状态锁不可用".to_string())?
        .clone();
    cfg.account = None;
    cfg.sms_code = None;
    save_config(&cfg, &paths, &config_state)?;
    Ok(build_view(&cfg))
}

#[tauri::command]
pub fn set_autostart(
    app: AppHandle,
    enabled: bool,
) -> Result<CommandResult, String> {
    crate::tray::set_autostart(&app, enabled)?;
    Ok(CommandResult {
        ok: true,
        message: if enabled { "开机自启已开启" } else { "开机自启已关闭" }.into(),
    })
}

/// M2 占位：校验配置是否就绪；M3 接入真正的登录状态机后替换实现。
#[tauri::command]
pub fn login_now(config_state: State<'_, ConfigState>) -> Result<CommandResult, String> {
    let now = now_ms();
    let cfg = config_state
        .0
        .lock()
        .map_err(|_| "配置状态锁不可用".to_string())?;
    if cfg.account.is_none() {
        return Err("请先填写校园网账号和密码".into());
    }
    if !cfg.sms_valid_at(now) {
        return Err("动态密码缺失或已过期，请填写后重试".into());
    }
    Ok(CommandResult {
        ok: true,
        message: "配置已就绪：登录引擎将在下一步（M3）接入".into(),
    })
}

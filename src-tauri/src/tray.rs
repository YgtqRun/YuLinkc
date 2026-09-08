//! 系统托盘：左键打开设置窗口，右键菜单提供 打开设置 / 立即登录 / 开机自启 / 退出。

use log::{info, warn};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::store::{ConfigPaths, ConfigState};

pub const MAIN_WINDOW: &str = "main";
pub const TRAY_ID: &str = "main-tray";

/// 需要动态改文案的菜单项句柄。
pub struct TrayHandles {
    pub autostart_item: MenuItem<Wry>,
}

pub fn setup(app: &AppHandle<Wry>) -> Result<(), String> {
    let open = MenuItem::with_id(app, "open", "打开设置", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let login = MenuItem::with_id(app, "login", "立即登录", true, None::<&str>)
        .map_err(|e| e.to_string())?;

    let auto_state = app.autolaunch().is_enabled().unwrap_or(false);
    let auto_label = if auto_state { "开机自启：开" } else { "开机自启：关" };
    let autostart = MenuItem::with_id(app, "autostart", auto_label, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|e| e.to_string())?;

    let menu = Menu::with_items(app, &[&open, &login, &autostart, &quit])
        .map_err(|e| e.to_string())?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "缺少默认窗口图标，无法创建托盘".to_string())?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("御连 YuLink")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_settings(app),
            "login" => {
                // M3 集成登录状态机后，这里触发真实登录流程
                warn!("立即登录：待 M3 登录状态机接入");
                show_settings(app);
            }
            "autostart" => toggle_autostart(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_settings(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("创建托盘失败: {e}"))?;

    app.manage(TrayHandles { autostart_item: autostart });
    Ok(())
}

/// 显示并聚焦设置主窗口（已隐藏时重新显示）。
pub fn show_settings(app: &AppHandle<Wry>) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        info!("从托盘打开设置窗口");
    } else {
        warn!("设置主窗口不存在（label={MAIN_WINDOW}）");
    }
}

fn toggle_autostart(app: &AppHandle<Wry>) {
    let auto = app.autolaunch();
    let enabled = auto.is_enabled().unwrap_or(false);
    match set_autostart(app, !enabled) {
        Ok(()) => {
            info!("开机自启已{}", if enabled { "关闭" } else { "开启" });
        }
        Err(e) => {
            warn!("切换开机自启失败: {e}");
        }
    }
}

/// 设置开机自启并同步持久化配置与托盘菜单文案。供托盘与 UI 命令共用。
pub(crate) fn set_autostart(app: &AppHandle<Wry>, enabled: bool) -> Result<(), String> {
    let auto = app.autolaunch();
    let result = if enabled {
        auto.enable()
    } else {
        auto.disable()
    };
    result.map_err(|e| format!("切换开机自启失败: {e}"))?;

    let paths = app.state::<ConfigPaths>();
    let state = app.state::<ConfigState>();
    if let Ok(mut cfg) = state.0.lock() {
        cfg.preferences.autostart = enabled;
        if let Err(e) = cfg.save(&paths.dir) {
            warn!("保存开机自启配置失败: {e}");
            return Err(format!("保存开机自启配置失败: {e}"));
        }
    }
    if let Some(handles) = app.try_state::<TrayHandles>() {
        let label = if enabled { "开机自启：开" } else { "开机自启：关" };
        if let Err(e) = handles.autostart_item.set_text(label) {
            warn!("更新托盘菜单文案失败: {e}");
        }
    }
    Ok(())
}

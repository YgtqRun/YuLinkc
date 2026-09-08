//! 系统托盘：左键打开设置窗口，右键菜单提供 打开设置 / 立即登录 / 开机自启 / 退出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use log::{info, warn};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::scheduler::Scheduler;
use crate::store::{ConfigPaths, ConfigState};

pub const MAIN_WINDOW: &str = "main";
pub const TRAY_ID: &str = "main-tray";
const ENTER_DELAY_MS: u64 = 80;
const EXIT_WAIT_MS: u64 = 230;

/// 退出动画进行中标记，避免焦点丢失与托盘点击重复触发
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);

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
                if let Some(sched) = app.try_state::<Scheduler>() {
                    sched.request_login();
                    info!("托盘：已触发立即登录");
                } else {
                    warn!("调度器未就绪，无法触发立即登录");
                }
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
                toggle_settings(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|e| format!("创建托盘失败: {e}"))?;

    app.manage(TrayHandles { autostart_item: autostart });
    Ok(())
}

/// 显示并聚焦设置主窗口（已隐藏时重新显示），位置先贴到屏幕右下角。
pub fn show_settings(app: &AppHandle<Wry>) {
    if EXIT_PENDING.load(Ordering::SeqCst) {
        return;
    }
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        place_bottom_right(&win);
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        info!("从托盘打开设置窗口");
        // 等窗口真正显示稳定后再让 Vue 播放右侧滑入动画
        let anim_win = win.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(ENTER_DELAY_MS));
            let _ = anim_win.eval("window.__yulinkEnter?.()");
        });
    } else {
        warn!("设置主窗口不存在（label={MAIN_WINDOW}）");
    }
}

/// 收起：先让 Vue 向右滑出，动画结束后再隐藏窗口（窗口本身不移动）。
pub fn hide_settings(app: &AppHandle<Wry>) {
    let Some(win) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    if !win.is_visible().unwrap_or(false) {
        return;
    }
    if EXIT_PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = win.eval("window.__yulinkExit?.()");
    let hide_win = win.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(EXIT_WAIT_MS));
        let _ = hide_win.hide();
        EXIT_PENDING.store(false, Ordering::SeqCst);
    });
}

/// 托盘左键：Win11 控制中心式开合——已显示且聚焦则收起，否则显示。
pub fn toggle_settings(app: &AppHandle<Wry>) {
    if EXIT_PENDING.load(Ordering::SeqCst) {
        return;
    }
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        if win.is_visible().unwrap_or(false) && win.is_focused().unwrap_or(false) {
            hide_settings(app);
            return;
        }
        show_settings(app);
    }
}

/// 把窗口放到当前显示器工作区右下角（距边缘 20px，位于任务栏上方）。
fn place_bottom_right(win: &tauri::WebviewWindow<Wry>) {
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let work = monitor.work_area();
    let Ok(size) = win.outer_size() else {
        return;
    };
    let margin = 20i32;
    let x = work.position.x + work.size.width as i32 - size.width as i32 - margin;
    let y = work.position.y + work.size.height as i32 - size.height as i32 - margin;
    let _ = win.set_position(PhysicalPosition::new(x.max(0), y.max(0)));
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

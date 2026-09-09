//! 系统托盘：左键打开设置窗口，右键菜单提供 立即登录 / 退出。
//! （“打开设置”与“开机自启”从右键菜单移除：左键点击即可打开设置，
//! 开机自启保留在设置页内。）

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use log::{info, warn};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Wry};
use tauri_plugin_autostart::ManagerExt;

use crate::scheduler::Scheduler;
use crate::state::RuntimeState;
use crate::store::{ConfigPaths, ConfigState};

pub const MAIN_WINDOW: &str = "main";
pub const TRAY_ID: &str = "main-tray";
const ENTER_DELAY_MS: u64 = 80;
const EXIT_WAIT_MS: u64 = 230;

/// 退出动画进行中标记，避免焦点丢失与托盘点击重复触发
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);

pub fn setup(app: &AppHandle<Wry>) -> Result<(), String> {
    let login = MenuItem::with_id(app, "login", "立即登录", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|e| e.to_string())?;

    let menu = Menu::with_items(app, &[&login, &quit])
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
            "login" => {
                if let Some(sched) = app.try_state::<Scheduler>() {
                    sched.request_login();
                    info!("托盘：已触发立即登录");
                } else {
                    warn!("调度器未就绪，无法触发立即登录");
                }
                show_settings(app);
            }
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

    // 应用启动时若有运行状态，立即同步托盘图标颜色
    if let Some(state) = app.try_state::<RuntimeState>() {
        if let Some(payload) = state.current() {
            update_tray_status(app, &payload.kind);
        }
    }
    Ok(())
}

/// 状态变化时更新托盘图标颜色与提示（网络状态可视化）。
pub fn update_tray_status(app: &AppHandle<Wry>, kind: &str) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let (r, g, b, label) = match kind {
        "connected" | "ready" => (31, 161, 85, "已连接"),
        "needs-sms" => (208, 139, 0, "需要动态密码"),
        "failed" => (216, 59, 59, "登录失败"),
        "logging-in" => (29, 100, 216, "登录中"),
        "away-mode" => (120, 125, 132, "离校模式"),
        "network-down" => (120, 125, 132, "网络未就绪"),
        _ => (154, 160, 168, "检测中"),
    };
    let _ = tray.set_icon(status_overlay_icon(app, r, g, b));
    let _ = tray.set_tooltip(Some(format!("御连 YuLink - {label}")));
}

/// 在原应用图标右下角叠加状态点（白圈 + 状态色），找不到原图标时返回 None 不改动。
fn status_overlay_icon(
    app: &AppHandle<Wry>,
    r: u8,
    g: u8,
    b: u8,
) -> Option<tauri::image::Image<'static>> {
    let base = app.default_window_icon()?;
    let width = base.width();
    let height = base.height();
    let pixels = base.rgba();
    if pixels.len() != (width * height * 4) as usize {
        return None;
    }

    let mut rgba = pixels.to_vec();
    let size = width.min(height) as f64;
    let dot_r = (size * 0.18).round().max(4.0);
    let cx = width as f64 - 1.0 - dot_r * 0.85;
    let cy = height as f64 - 1.0 - dot_r * 0.85;
    let inner_r = dot_r * 0.62;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d > dot_r {
                continue;
            }
            let (cr, cg, cb, alpha) = if d <= inner_r {
                (r, g, b, 255u8)
            } else if d <= dot_r {
                // 白圈边缘做 1px 抗锯齿
                let a = if d > dot_r - 1.0 {
                    (((dot_r - d) * 255.0).round() as u8).min(255)
                } else {
                    255
                };
                (255, 255, 255, a)
            } else {
                continue;
            };
            let idx = ((y * width + x) * 4) as usize;
            rgba[idx] = cr;
            rgba[idx + 1] = cg;
            rgba[idx + 2] = cb;
            rgba[idx + 3] = alpha;
        }
    }
    Some(tauri::image::Image::new_owned(rgba, width, height))
}

/// 显示并聚焦设置主窗口（已隐藏时重新显示），位置先贴到屏幕右下角。
pub fn show_settings(app: &AppHandle<Wry>) {
    if EXIT_PENDING.load(Ordering::SeqCst) {
        return;
    }
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        // 显示前先把卡片置于右侧外隐藏，首帧不闪现
        let _ = win.eval("window.__yulinkPrepare?.()");
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

/// 设置开机自启并同步持久化配置。供 UI 设置页调用。
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
    Ok(())
}

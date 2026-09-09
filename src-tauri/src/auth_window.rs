//! 认证页（WebView2 窗口）生命周期与显示策略。
//!
//! 显示策略配置（优先级从高到低）：
//! 1. 环境变量 `YULINK_AUTH_VISIBLE`：`1/true/show` 显示，`0/false/hidden` 隐藏；
//! 2. debug 构建默认**显示**认证页（方便开发/联调时观察真实页面）；
//! 3. release 构建默认**隐藏**（屏幕外可见，规避隐藏 WebView eval no-op，见实现方案 §5.2）。

use std::sync::mpsc;
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthWindowVisibility {
    /// 在屏幕上正常显示（可调、带焦点），用于开发调试。
    Visible,
    /// 屏幕外创建但保持 visible 状态（-32000,-32000，不进任务栏），生产默认。
    Hidden,
}

impl AuthWindowVisibility {
    /// 解析认证窗口显示配置。
    pub fn resolve() -> Self {
        if let Ok(value) = std::env::var("YULINK_AUTH_VISIBLE") {
            match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "show" | "visible" | "on" => return Self::Visible,
                "0" | "false" | "hide" | "hidden" | "off" => return Self::Hidden,
                _ => {}
            }
        }
        if cfg!(debug_assertions) {
            Self::Visible
        } else {
            Self::Hidden
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Visible => "屏幕内显示（开发调试）",
            Self::Hidden => "完全隐藏（生产默认）",
        }
    }
}

/// 创建认证窗口。保持 `visible` 状态是前提：完全隐藏时 WebView2 的 eval 可能被 no-op。
pub fn create_auth_window(
    app: &AppHandle,
    label: &str,
    url: &str,
    visibility: AuthWindowVisibility,
) -> Result<(), String> {
    let parsed = tauri::Url::parse(url).map_err(|e| format!("URL 解析失败: {e}"))?;
    let nav_url = parsed.clone();
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let app2 = app.clone();
    let label_owned = label.to_string();

    app.run_on_main_thread(move || {
        let mut builder = WebviewWindowBuilder::new(&app2, label_owned, WebviewUrl::External(parsed))
            .title("YuLink Auth")
            .inner_size(900.0, 700.0)
            .visible(true);

        builder = match visibility {
            AuthWindowVisibility::Visible => builder
                .position(160.0, 140.0)
                .decorations(true)
                .resizable(true)
                .skip_taskbar(false)
                .focused(true),
            AuthWindowVisibility::Hidden => builder
                .decorations(false)
                .resizable(false)
                .skip_taskbar(true)
                // 主窗口隐藏时前端 JS 均正常执行，证明隐藏 WebView 可运行注入脚本；
                // 认证窗口直接不可见，彻底避免离屏坐标在部分机器上仍会显示的问题。
                .visible(false),
        };

        let result = if cfg!(debug_assertions) {
            // 开发阶段打印页面加载事件，便于观察注入时序
            builder
                .on_page_load(|_w, payload| {
                    println!("[auth] page_load {} {:?}", payload.url(), payload.event());
                })
                .build()
        } else {
            builder.build()
        }
        .and_then(|win| {
            // 建窗后再次显式导航，规避初始导航未触发的情况
            win.navigate(nav_url)?;
            Ok(win)
        })
        .map(|_| ())
        .map_err(|e| e.to_string());

        let _ = tx.send(result);
    })
    .map_err(|e| format!("主线程调度失败: {e}"))?;

    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("等待窗口创建超时: {e}"))?
}

pub fn destroy_auth_window(app: &AppHandle, label: &str) {
    let app2 = app.clone();
    let label_owned = label.to_string();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = app2.get_webview_window(&label_owned) {
            let _ = w.destroy();
        }
    });
    // 给 WebView2 一点时间完成销毁，避免立即用同 label 重建
    std::thread::sleep(Duration::from_millis(800));
}

/// 清理所有以指定前缀命名的认证窗口（上一轮失败时可能未销毁干净）。
pub fn cleanup_auth_windows(app: &AppHandle, prefix: &str) {
    let app2 = app.clone();
    let prefix_owned = prefix.to_string();
    let _ = app.run_on_main_thread(move || {
        let labels: Vec<String> = app2
            .webview_windows()
            .keys()
            .filter(|l| l.starts_with(&prefix_owned))
            .cloned()
            .collect();
        for label in labels {
            if let Some(w) = app2.get_webview_window(&label) {
                let _ = w.destroy();
            }
        }
    });
    std::thread::sleep(Duration::from_millis(1000));
}

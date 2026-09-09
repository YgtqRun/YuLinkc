//! 认证页（WebView2 窗口）生命周期与显示策略。
//!
//! 显示策略完全由配置（设置页“显示认证窗口”）驱动：
//! 开启时正常显示，方便开发/联调观察真实页面；
//! 关闭（默认）时隐藏（规避隐藏 WebView eval no-op 的做法见实现方案 §5.2）。

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
    /// 按设置页“显示认证窗口”开关解析显示策略。
    pub fn from_config(show: bool) -> Self {
        if show {
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

        // 只走 `WebviewUrl::External` 这一次导航；不要建窗后再 navigate 一次，
        // 否则第一次文档刚执行注入脚本就整页被替换（日志表现为 started 后静默超时）。
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

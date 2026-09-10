mod auth_window;
mod auth_session;
mod autostart;
mod bridge;
mod commands;
mod inject;
mod logger;
mod mock;
mod network;
mod poc;
mod scheduler;
mod state;
mod store;
mod tray;
mod update;

use tauri::Manager;

/// 开机自启看护线程的巡检间隔（秒）。
const AUTOSTART_WATCH_INTERVAL_SEC: u64 = 60;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!("检测到第二次启动，聚焦已有实例");
            tray::show_settings(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::clear_sms_code,
            commands::clear_all,
            commands::set_autostart,
            commands::login_now,
            commands::get_runtime_status,
            commands::get_app_info,
            commands::check_update,
        ])
        .on_window_event(|window, event| {
            // 主窗口点关闭时隐藏到托盘，而不是退出进程
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == tray::MAIN_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // Win11 控制中心式：点击外部失焦即收起（登录进行中除外）
            if let tauri::WindowEvent::Focused(false) = event {
                if window.label() == tray::MAIN_WINDOW {
                    let logging_in = window
                        .app_handle()
                        .try_state::<state::RuntimeState>()
                        .and_then(|s| s.current())
                        .map(|p| p.kind == "logging-in")
                        .unwrap_or(false);
                    if !logging_in {
                        tray::hide_settings(window.app_handle());
                    }
                }
            }
        });

    builder = builder.setup(|app| {
        let config_dir = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("yulink"));

        // 轮转日志（失败不阻塞启动）
        if let Err(e) = logger::FileLogger::init(&config_dir.join("logs"), log::LevelFilter::Info)
        {
            eprintln!("[yulink] 日志初始化失败: {e}");
            // GUI 下看不到 stderr，把失败原因落到配置目录，便于事后排查。
            let _ = std::fs::write(
                config_dir.join("logger-init-error.txt"),
                format!("[yulink] 日志初始化失败: {e}\r\n"),
            );
        }

        // 配置：加载默认值或磁盘配置，注册为全局状态
        if let Err(e) = std::fs::create_dir_all(&config_dir) {
            eprintln!("[yulink] 创建配置目录失败: {e}");
        }
        let config = match store::AppConfig::load(&config_dir) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[yulink] 配置加载失败，使用默认值: {e}");
                store::AppConfig::default()
            }
        };
        if let Err(e) = config.save(&config_dir) {
            eprintln!("[yulink] 初始化配置失败: {e}");
        }
        app.manage(store::ConfigPaths { dir: config_dir });
        app.manage(store::ConfigState(std::sync::Mutex::new(config)));
        app.manage(state::RuntimeState::default());
        log::info!("御连 YuLink 启动");

        reconcile_autostart(app);
        spawn_autostart_watchdog(app);

        let handle = app.handle().clone();

        // E2E：用本地 mock 认证页顶替门户，并让外网探活必然失败
        if std::env::var("YULINK_MOCK_PORTAL")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            if let Ok(port) = mock::spawn_mock_server() {
                if let Ok(mut cfg) = app.state::<store::ConfigState>().0.lock() {
                    cfg.preferences.portal_wireless =
                        format!("http://127.0.0.1:{port}/wireless.html?result=success");
                    cfg.preferences.portal_wired =
                        format!("http://127.0.0.1:{port}/wired.html?result=success");
                    cfg.preferences.external_probe_url = "http://127.0.0.1:1/".into();
                    log::info!("E2E mock 门户端口: {port}");
                }
            }
            // YULINK_E2E_SEED=1：预置测试账号与动态密码（仅 E2E，勿在生产使用）
            if std::env::var("YULINK_E2E_SEED")
                .map(|v| v == "1")
                .unwrap_or(false)
            {
                if let Ok(mut cfg) = app.state::<store::ConfigState>().0.lock() {
                    let _ = cfg.set_account("20240001", "demo-pass", "2");
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    let _ = cfg.set_sms_code("123456", now, 1.0);
                    let _ = cfg.save(&app.state::<store::ConfigPaths>().dir);
                }
            }
        }

        // POC 模式：YULINK_POC=1 时启动即跑验证（mock 页 + 窗口注入 + 信标回传）
        let poc_enabled = std::env::var("YULINK_POC")
            .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
            .unwrap_or(false);
        if poc_enabled {
            log::info!("POC 模式启动");
            std::thread::spawn(move || {
                let code = match poc::run_poc(&handle) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("[POC] 失败: {e}");
                        1
                    }
                };
                std::process::exit(code);
            });
        } else {
            let scheduler = scheduler::spawn(&handle);
            app.manage(scheduler);
            if let Err(e) = tray::setup(&handle) {
                eprintln!("[yulink] 托盘初始化失败: {e}");
                log::error!("托盘初始化失败: {e}");
            } else {
                log::info!("托盘已就绪（左键打开设置）");
            }
        }
        Ok(())
    });

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 启动自检：对齐“开机自启登记项”与当前 exe，结果只写日志。
///
/// 免安装版被改名或挪目录后，注册表里记录的旧路径会让启动项静默失效
/// （开机时 Windows 找不到文件），这里在每次启动时补回来。
fn reconcile_autostart(app: &tauri::App) {
    let enabled = app
        .state::<store::ConfigState>()
        .0
        .lock()
        .map(|cfg| cfg.preferences.autostart)
        .unwrap_or(true);

    match autostart::sync(enabled) {
        Ok(autostart::Sync::Repaired { previous }) => match previous {
            Some(old) => log::info!("已自动修复开机自启项（程序位置变化：{old}）"),
            None => log::info!("已自动重新登记开机自启项"),
        },
        Ok(autostart::Sync::DisabledByUser) => {
            log::info!("系统的“启动应用”已关闭开机自启，设置项同步为关闭");
            if let Ok(mut cfg) = app.state::<store::ConfigState>().0.lock() {
                cfg.preferences.autostart = false;
            }
            if let Err(e) = save_config_state(app) {
                log::warn!("保存开机自启状态失败: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("开机自启同步失败: {e}"),
    }

    // “开机启动加速”：配置开着但注册表没写成功时补一次。
    let boost = app
        .state::<store::ConfigState>()
        .0
        .lock()
        .map(|cfg| cfg.preferences.startup_boost)
        .unwrap_or(false);
    if boost && !autostart::startup_boost_enabled() {
        if let Err(e) = autostart::set_startup_boost(true) {
            log::warn!("关闭启动项延迟失败: {e}");
        }
    }
}

/// 把内存中的配置写回磁盘（启动自检回写用）。
fn save_config_state(app: &tauri::App) -> Result<(), String> {
    let dir = app.state::<store::ConfigPaths>().dir.clone();
    let cfg = app
        .state::<store::ConfigState>()
        .0
        .lock()
        .map_err(|_| "配置状态锁不可用".to_string())?
        .clone();
    cfg.save(&dir)
}

/// 运行期看护：程序在运行中被改名/挪目录后，定时把登记路径修回当前 exe。
fn spawn_autostart_watchdog(app: &tauri::App) {
    let handle = app.handle().clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(
            AUTOSTART_WATCH_INTERVAL_SEC,
        ));
        let enabled = handle
            .state::<store::ConfigState>()
            .0
            .lock()
            .map(|cfg| cfg.preferences.autostart)
            .unwrap_or(false);
        if !enabled {
            continue;
        }
        match autostart::ensure_current() {
            Ok(true) => log::info!("检测到程序位置变化，已更新开机自启登记项"),
            Ok(false) => {}
            Err(e) => log::warn!("开机自启看护失败: {e}"),
        }
    });
}

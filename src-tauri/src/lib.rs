mod auth_window;
mod bridge;
mod commands;
mod logger;
mod poc;
mod store;
mod tray;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log::info!("检测到第二次启动，聚焦已有实例");
            tray::show_settings(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::clear_sms_code,
            commands::clear_all,
            commands::set_autostart,
            commands::login_now,
        ])
        .on_window_event(|window, event| {
            // 主窗口点关闭时隐藏到托盘，而不是退出进程
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == tray::MAIN_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
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
        log::info!("御连 YuLink 启动");

        let handle = app.handle().clone();

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
        } else if let Err(e) = tray::setup(&handle) {
            eprintln!("[yulink] 托盘初始化失败: {e}");
            log::error!("托盘初始化失败: {e}");
        } else {
            log::info!("托盘已就绪（左键打开设置）");
        }
        Ok(())
    });

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

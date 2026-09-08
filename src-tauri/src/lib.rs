// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod auth_window;
mod poc;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet]);

    // POC 模式：YULINK_POC=1 时启动即跑验证（mock 页 + 屏幕外窗口注入 + 信标回传）
    let poc_enabled = std::env::var("YULINK_POC")
        .map(|v| v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(false);
    if poc_enabled {
        builder = builder.setup(|app| {
            let handle = app.handle().clone();
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
            Ok(())
        });
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

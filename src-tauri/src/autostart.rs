//! 开机自启登记项（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`）自管理。
//!
//! 背景：早期版本用 tauri-plugin-autostart 登记自启，它在“开启”那一刻把**当时的
//! exe 绝对路径**写进注册表，而系统判断启动项是否存在只看“值在不在”、不比较路径。
//! 于是免安装版一旦被改名或挪目录，登记项仍指向旧路径：开机时 Windows 找不到文件，
//! 启动项静默失效，而设置页依旧显示“已开启”。
//!
//! 这里改为自己维护该注册表项：
//! - 值名固定为 [`VALUE_NAME`]（与 exe 文件名无关），值内容为**带引号**的当前 exe
//!   路径——旧实现不加引号，路径含空格时会被 Windows 截断成错误的程序名；
//! - 启动时与运行期都比对“登记路径 vs 本进程 exe”，不一致就改写为当前路径，
//!   实现改名/挪目录后的自愈；
//! - 尊重“任务管理器 → 启动应用”里被用户关掉的状态（Run 值通常还在，但
//!   `StartupApproved\Run` 会留下禁用标记），不会擅自把启动项重新打开。

use winreg::enums::{RegType::REG_BINARY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use winreg::{RegKey, RegValue};

/// 当前用户的开机启动项位置。
const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
/// 任务管理器“启动”页记录的启用/禁用状态。
const STARTUP_APPROVED_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// “已启用”标记：首字节 0x02、其余为零；被禁用时写入的是 0x03 + 禁用时间戳。
const APPROVED_ENABLED: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
/// 资源管理器给“登录后启动项”加的延迟（毫秒）；写 0 表示不延迟。
const SERIALIZE_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Serialize";
const STARTUP_DELAY_VALUE: &str = "StartupDelayInMSec";

/// 自启项在注册表里的值名。沿用旧版（tauri-plugin-autostart 用 Cargo 包名）的名字，
/// 这样老用户的启动项原地升级：值名与 exe 文件名无关，改 exe 名字不会换项。
///
/// 注意：注册表值名不区分大小写，`yulink` 与 `YuLink` 是同一个值，不要试图“改名”，
/// 否则删除旧写法会连同刚写入的新值一起删掉。
pub const VALUE_NAME: &str = "yulink";

/// 启动自检结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// 配置里未开启自启，已确保登记项不存在。
    Disabled,
    /// 登记项已指向当前程序，无需改动。
    Ok,
    /// 登记项缺失或指向旧路径，本次已改写为当前程序路径。
    Repaired { previous: Option<String> },
    /// 用户在 Windows 任务管理器里关掉了自启：遵循用户选择，不改写登记项。
    DisabledByUser,
}

// ===================== 注册表基元 =====================

fn read_value(name: &str) -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ)
        .ok()?
        .get_value::<String, _>(name)
        .ok()
}

fn write_value(name: &str, value: &str) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .create_subkey(RUN_KEY)
        .map(|(key, _)| key)
        .map_err(|e| format!("打开开机自启注册表项失败: {e}"))?;
    key.set_value(name, &value)
        .map_err(|e| format!("写入开机自启注册表项失败: {e}"))
}

fn delete_value(name: &str) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .map_err(|e| format!("打开开机自启注册表项失败: {e}"))?;
    match key.delete_value(name) {
        Ok(()) => Ok(()),
        // 本来就没有这项，不算失败。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("删除开机自启注册表项失败: {e}")),
    }
}

/// 任务管理器里的启用状态：`Some(false)` 表示被用户关掉了。
fn startup_approved(name: &str) -> Option<bool> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let raw = hkcu
        .open_subkey_with_flags(STARTUP_APPROVED_KEY, KEY_READ)
        .ok()?
        .get_raw_value(name)
        .ok()?;
    if raw.bytes.len() < 12 {
        return None;
    }
    Some(raw.bytes[1..].iter().all(|b| *b == 0))
}

/// 标记为“已启用”：用户之前在任务管理器禁用过时，重新开启需要清掉禁用标记。
fn mark_startup_approved(name: &str) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(STARTUP_APPROVED_KEY, KEY_SET_VALUE) {
        let _ = key.set_raw_value(
            name,
            &RegValue {
                vtype: REG_BINARY,
                bytes: APPROVED_ENABLED.to_vec(),
            },
        );
    }
}

fn clear_startup_approved(name: &str) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(STARTUP_APPROVED_KEY, KEY_SET_VALUE) {
        let _ = key.delete_value(name);
    }
}

// ===================== 路径工具 =====================

fn current_exe() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| format!("获取程序路径失败: {e}"))
}

/// 注册表里登记的命令行：路径加引号，避免含空格的目录被 Windows 截断。
fn command_line(exe: &str) -> String {
    format!("\"{exe}\"")
}

/// 从登记值里取出 exe 路径；兼容旧版不带引号的 `路径 参数` 写法。
pub fn parse_registered_exe(raw: &str) -> String {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        let end = rest.find('"').unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    // 只把 ASCII 转小写，保证下标与原文一致（用于按字节切片）。
    let lowered: String = raw
        .chars()
        .map(|c| if c.is_ascii() { c.to_ascii_lowercase() } else { c })
        .collect();
    match lowered.find(".exe") {
        Some(idx) => raw[..idx + 4].to_string(),
        None => raw.to_string(),
    }
}

/// Windows 路径比较：忽略大小写、斜杠方向与结尾分隔符。
pub fn same_exe(a: &str, b: &str) -> bool {
    fn normalize(path: &str) -> String {
        path.trim()
            .trim_matches('"')
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    }
    let (a, b) = (normalize(a), normalize(b));
    !a.is_empty() && a == b
}

// ===================== 对外接口 =====================

/// 按配置同步自启项，供启动自检调用。
pub fn sync(enabled: bool) -> Result<Sync, String> {
    sync_named(VALUE_NAME, enabled)
}

/// 设置页开关：开启时强制改写为当前路径，关闭时删除登记项。
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        let exe = current_exe()?;
        write_value(VALUE_NAME, &command_line(&exe))?;
        mark_startup_approved(VALUE_NAME);
        Ok(())
    } else {
        delete_value(VALUE_NAME)?;
        clear_startup_approved(VALUE_NAME);
        Ok(())
    }
}

/// 运行期看护：程序在运行中被改名/挪目录时，登记路径会失效，这里就地改写。
/// 登记项不存在（未开启或用户已删除）或路径一致时不做任何改动。
pub fn ensure_current() -> Result<bool, String> {
    let exe = current_exe()?;
    let Some(raw) = read_value(VALUE_NAME) else {
        return Ok(false);
    };
    if same_exe(&parse_registered_exe(&raw), &exe) {
        return Ok(false);
    }
    write_value(VALUE_NAME, &command_line(&exe))?;
    mark_startup_approved(VALUE_NAME);
    Ok(true)
}

/// 是否已关闭“登录后启动项延迟”（`StartupDelayInMSec` = 0）。
pub fn startup_boost_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(SERIALIZE_KEY, KEY_READ)
        .ok()
        .and_then(|key| key.get_value::<u32, _>(STARTUP_DELAY_VALUE).ok())
        == Some(0)
}

/// 设置“登录后启动项延迟”：开启写 0 毫秒，关闭时删除该项、恢复系统默认。
///
/// 该值属于当前用户，对**所有**自启项生效，所以由设置页显式开关控制。
pub fn set_startup_boost(enabled: bool) -> Result<(), String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if enabled {
        let (key, _) = hkcu
            .create_subkey(SERIALIZE_KEY)
            .map_err(|e| format!("打开启动延迟注册表项失败: {e}"))?;
        key.set_value(STARTUP_DELAY_VALUE, &0u32)
            .map_err(|e| format!("关闭启动项延迟失败: {e}"))
    } else {
        let key = hkcu
            .open_subkey_with_flags(SERIALIZE_KEY, KEY_SET_VALUE)
            .map_err(|e| format!("打开启动延迟注册表项失败: {e}"))?;
        match key.delete_value(STARTUP_DELAY_VALUE) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("恢复启动项延迟设置失败: {e}")),
        }
    }
}

// ===================== 内部实现 =====================

/// 与 [`sync`] 相同，但可指定值名（单元测试用独立值名，避免动到真实启动项）。
fn sync_named(name: &str, enabled: bool) -> Result<Sync, String> {
    if !enabled {
        delete_value(name)?;
        clear_startup_approved(name);
        return Ok(Sync::Disabled);
    }

    let exe = current_exe()?;
    let previous = read_value(name).map(|raw| parse_registered_exe(&raw));

    // “启动应用”开关关掉时只在 StartupApproved\Run 留禁用标记，此时不擅自恢复。
    if startup_approved(name) == Some(false) {
        return Ok(Sync::DisabledByUser);
    }
    if let Some(path) = previous.as_deref() {
        if same_exe(path, &exe) {
            return Ok(Sync::Ok);
        }
    }

    write_value(name, &command_line(&exe))?;
    mark_startup_approved(name);
    Ok(Sync::Repaired { previous })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// 测试用的独立值名，避免动到真实的 `YuLink` / `yulink` 启动项。
    fn test_value_name() -> String {
        format!(
            "YuLinkSelfTest-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn parses_quoted_and_legacy_values() {
        assert_eq!(
            parse_registered_exe("\"C:\\a b\\YuLink.exe\""),
            "C:\\a b\\YuLink.exe"
        );
        assert_eq!(
            parse_registered_exe("\"C:\\a b\\YuLink.exe\" --minimized"),
            "C:\\a b\\YuLink.exe"
        );
        // 旧版不带引号：路径后面可能跟着参数或尾随空格
        assert_eq!(parse_registered_exe("C:\\a\\YuLink.exe "), "C:\\a\\YuLink.exe");
        assert_eq!(
            parse_registered_exe("C:\\a\\YuLink.EXE --minimized"),
            "C:\\a\\YuLink.EXE"
        );
        assert_eq!(parse_registered_exe(""), "");
    }

    #[test]
    fn compares_paths_loosely() {
        assert!(same_exe("C:\\Tools\\YuLink.exe", "c:/tools/yulink.exe"));
        assert!(same_exe("C:\\Tools\\YuLink.exe\\", "C:\\Tools\\YuLink.exe"));
        assert!(same_exe("\"C:\\Tools\\YuLink.exe\"", "C:\\Tools\\YuLink.exe"));
        assert!(!same_exe("C:\\Tools\\YuLinkc.exe", "C:\\Tools\\YuLink.exe"));
        assert!(!same_exe("", "C:\\Tools\\YuLink.exe"));
    }

    /// 在注册表里跑一遍“exe 改名后自愈”的流程（用独立值名，结束后清理）。
    #[test]
    fn registry_entry_self_heals() {
        if std::env::var("YULINK_SKIP_REGISTRY_TESTS").is_ok() {
            return;
        }
        let name = test_value_name();
        let exe = current_exe().expect("current_exe");

        // 1) 登记项缺失 → 写入当前 exe
        assert_eq!(
            sync_named(&name, true).unwrap(),
            Sync::Repaired { previous: None }
        );
        let written = read_value(&name).expect("登记值已写入");
        assert!(written.starts_with('"'), "登记值应带引号: {written}");
        assert_eq!(parse_registered_exe(&written), exe);

        // 2) 登记项指向旧名字（模拟 exe 被改名）→ 启动自检改写为当前路径
        write_value(&name, "\"D:\\Old\\yulink-v0.exe\"").unwrap();
        assert_eq!(
            sync_named(&name, true).unwrap(),
            Sync::Repaired {
                previous: Some("D:\\Old\\yulink-v0.exe".into())
            }
        );
        assert!(same_exe(
            &parse_registered_exe(&read_value(&name).unwrap()),
            &exe
        ));

        // 3) 已指向当前程序 → 不再改动
        assert_eq!(sync_named(&name, true).unwrap(), Sync::Ok);

        // 4) 关闭 → 登记项与启用标记都被清掉
        assert_eq!(sync_named(&name, false).unwrap(), Sync::Disabled);
        assert_eq!(read_value(&name), None);
        assert_eq!(startup_approved(&name), None);
    }
}

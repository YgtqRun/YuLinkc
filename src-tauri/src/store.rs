//! 配置存取：JSON + Windows DPAPI 加密。
//!
//! 明文（密码、动态密码）经 CryptProtectData 加密后以 `dpapi:v1:<base64>` 落盘，
//! 仅在当前 Windows 用户下可解密。配置文件位于 `<config>/config.json`。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

const CONFIG_FILE: &str = "config.json";
#[allow(dead_code)] // M2 设置界面接入后不再需要手动许可
const DPAPI_PREFIX: &str = "dpapi:v1:";

/// 永久有效的标记（与油猴脚本 PERMANENT = -1 对齐）
pub const PERMANENT_HOURS: f64 = -1.0;

/// 由 setup 管理，供各模块定位配置文件。
#[derive(Clone)]
pub struct ConfigPaths {
    pub dir: PathBuf,
}

/// 内存中的配置状态（Tauri managed）。
pub struct ConfigState(pub Mutex<AppConfig>);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub account: Option<Account>,
    pub sms_code: Option<SmsCode>,
    pub preferences: Preferences,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub username: String,
    pub password_enc: String,
    pub isp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SmsCode {
    pub code_enc: String,
    pub saved_at: i64,
    /// 有效小时数；-1 表示永久
    pub hours: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub autostart: bool,
    pub adapter_mode: String,
    pub portal_wireless: String,
    pub portal_wired: String,
    pub external_probe_url: String,
    pub check_interval_online_sec: u64,
    pub check_interval_offline_sec: u64,
    pub login_max_retries: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            account: None,
            sms_code: None,
            preferences: Preferences::default(),
        }
    }
}

impl Default for Account {
    fn default() -> Self {
        Self {
            username: String::new(),
            password_enc: String::new(),
            isp: "2".into(),
        }
    }
}

impl Default for SmsCode {
    fn default() -> Self {
        Self {
            code_enc: String::new(),
            saved_at: 0,
            hours: 27.0,
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            autostart: true,
            adapter_mode: "auto".into(),
            portal_wireless: "http://172.26.255.2/".into(),
            portal_wired: "http://172.26.255.3/".into(),
            external_probe_url: "http://www.msftconnecttest.com/connecttest.txt".into(),
            check_interval_online_sec: 120,
            check_interval_offline_sec: 30,
            login_max_retries: 3,
        }
    }
}

#[allow(dead_code)] // 账号/动态密码存取由 M2/M3 使用，当前先由单元测试覆盖
impl AppConfig {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(CONFIG_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path).map_err(|e| format!("读取配置失败: {e}"))?;
        serde_json::from_str(&raw).map_err(|e| format!("解析配置失败: {e}"))
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        fs::create_dir_all(dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
        let path = dir.join(CONFIG_FILE);
        let raw = serde_json::to_string_pretty(self).map_err(|e| format!("序列化配置失败: {e}"))?;
        fs::write(&path, raw).map_err(|e| format!("写入配置失败: {e}"))
    }

    /// 保存账号并加密密码。
    pub fn set_account(&mut self, username: &str, password: &str, isp: &str) -> Result<(), String> {
        let password_enc = encrypt_secret(password.as_bytes())?;
        self.account = Some(Account {
            username: username.to_string(),
            password_enc,
            isp: isp.to_string(),
        });
        Ok(())
    }

    /// 解密后的账号密码（内存中短命使用，不落日志）。
    pub fn account_password(&self) -> Option<(String, String)> {
        let acc = self.account.as_ref()?;
        let pwd = decrypt_secret(&acc.password_enc).ok()?;
        Some((acc.username.clone(), String::from_utf8(pwd).ok()?))
    }

    pub fn set_sms_code(&mut self, code: &str, saved_at: i64, hours: f64) -> Result<(), String> {
        let code_enc = encrypt_secret(code.as_bytes())?;
        self.sms_code = Some(SmsCode {
            code_enc,
            saved_at,
            hours,
        });
        Ok(())
    }

    pub fn sms_code(&self) -> Option<String> {
        let sms = self.sms_code.as_ref()?;
        let bytes = decrypt_secret(&sms.code_enc).ok()?;
        Some(String::from_utf8(bytes).ok()?)
    }

    /// 动态密码在 `now`（unix 毫秒）是否有效。
    pub fn sms_valid_at(&self, now: i64) -> bool {
        let Some(sms) = self.sms_code.as_ref() else {
            return false;
        };
        if sms.hours < 0.0 {
            return true; // 永久
        }
        let remain = sms.hours * 3_600_000.0 - (now - sms.saved_at) as f64;
        remain > 0.0
    }

    /// 动态密码剩余毫秒；永久或无配置返回 None。
    pub fn sms_remaining_ms(&self, now: i64) -> Option<i64> {
        let sms = self.sms_code.as_ref()?;
        if sms.hours < 0.0 {
            return None;
        }
        let remain = (sms.hours * 3_600_000.0 - (now - sms.saved_at) as f64).round() as i64;
        Some(remain.max(0))
    }

    /// 动态密码剩余状态文案（与油猴脚本 captchaStatusText 一致）。
    pub fn sms_status_text(&self, now: i64) -> String {
        let Some(sms) = self.sms_code.as_ref() else {
            return "未设置".into();
        };
        if sms.code_enc.is_empty() {
            return "未设置".into();
        }
        if sms.hours < 0.0 {
            return "永久有效".into();
        }
        if !(sms.hours > 0.0) {
            return "配置异常".into();
        }
        let remain = sms.hours * 3_600_000.0 - (now - sms.saved_at) as f64;
        if remain <= 0.0 {
            return "已过期".into();
        }
        let remain_h = remain / 3_600_000.0;
        if remain_h >= 24.0 {
            format!("有效，剩余约 {:.1} 天", remain_h / 24.0)
        } else if remain_h >= 1.0 {
            format!("有效，剩余约 {:.1} 小时", remain_h)
        } else {
            let minutes = ((remain / 60_000.0).ceil() as i64).max(1);
            format!("有效，剩余约 {minutes} 分钟")
        }
    }
}

// ===================== DPAPI =====================

#[allow(dead_code)]
fn encrypt_secret(plain: &[u8]) -> Result<String, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: plain
            .len()
            .try_into()
            .map_err(|_| "明文过长".to_string())?,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| format!("CryptProtectData 失败: {e}"))?;
    }
    let encrypted =
        unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    }
    Ok(format!("{DPAPI_PREFIX}{}", BASE64.encode(encrypted)))
}

#[allow(dead_code)]
fn decrypt_secret(secret: &str) -> Result<Vec<u8>, String> {
    let payload = secret
        .strip_prefix(DPAPI_PREFIX)
        .ok_or_else(|| "密文缺少 dpapi:v1: 前缀".to_string())?;
    let encrypted =
        BASE64.decode(payload).map_err(|e| format!("base64 解码失败: {e}"))?;

    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted
            .len()
            .try_into()
            .map_err(|_| "密文过长".to_string())?,
        pbData: encrypted.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut out)
            .map_err(|e| format!("CryptUnprotectData 失败: {e}"))?;
    }
    let plain = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    }
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let plain = "2024-test-密码-123";
        let enc = encrypt_secret(plain.as_bytes()).expect("encrypt");
        assert!(enc.starts_with(DPAPI_PREFIX));
        let dec = decrypt_secret(&enc).expect("decrypt");
        assert_eq!(String::from_utf8(dec).unwrap(), plain);
    }

    #[test]
    fn config_roundtrip_in_temp_dir() {
        let dir = std::env::temp_dir().join(format!(
            "yulink-store-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut cfg = AppConfig::default();
        cfg.set_account("20240001", "p@ss word", "2").unwrap();
        cfg.set_sms_code("123456", 1_700_000_000_000, 27.0).unwrap();
        cfg.preferences.autostart = false;
        cfg.save(&dir).unwrap();

        let loaded = AppConfig::load(&dir).unwrap();
        assert_eq!(loaded.preferences.autostart, false);
        assert_eq!(
            loaded.account_password().unwrap(),
            ("20240001".to_string(), "p@ss word".to_string())
        );
        assert_eq!(loaded.sms_code().unwrap(), "123456");

        let _ = fs::remove_dir_all(&dir);
    }
}

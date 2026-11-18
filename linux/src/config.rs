use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::network::Medium;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub portal_wireless: String,
    pub portal_wired: String,
    pub eportal_port: u16,
    pub username: String,
    pub password: String,
    pub sms_code: String,
    /// Unix 毫秒；0 表示尚未开始计时。
    pub sms_saved_at: i64,
    /// 小时；-1 表示永久。
    pub sms_hours: f64,
    pub isp: String,
    pub external_probe: String,
    pub check_interval_online_sec: u64,
    pub check_interval_offline_sec: u64,
    pub auto_relogin: bool,
    pub away_mode: bool,
    /// system / light / dark
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            portal_wireless: "http://172.26.255.2/".into(),
            portal_wired: "http://172.26.255.3/".into(),
            eportal_port: 801,
            username: String::new(),
            password: String::new(),
            sms_code: String::new(),
            sms_saved_at: 0,
            sms_hours: 27.0,
            isp: "2".into(),
            external_probe: "http://www.msftconnecttest.com/connecttest.txt".into(),
            check_interval_online_sec: 120,
            check_interval_offline_sec: 30,
            auto_relogin: true,
            away_mode: false,
            theme: "system".into(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let mut cfg: Self = serde_json::from_str(&raw)
            .with_context(|| format!("解析配置失败: {}", path.display()))?;
        cfg.normalize();
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        fs::write(path, raw).with_context(|| format!("写入配置失败: {}", path.display()))?;
        set_private_permissions(path)?;
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.portal_wireless = normalize_url(&self.portal_wireless);
        self.portal_wired = normalize_url(&self.portal_wired);
        self.username = self.username.trim().to_string();
        self.isp = self.isp.trim().to_string();
        if self.isp.is_empty() {
            self.isp = "2".into();
        }
        if self.sms_hours == 0.0 {
            self.sms_hours = 27.0;
        }
        self.theme = normalize_theme(&self.theme);
    }

    pub fn validate_portal_url(raw: &str) -> Result<Url> {
        let url = Url::parse(raw).with_context(|| format!("认证页 URL 无效: {raw}"))?;
        if url.scheme() != "http" {
            bail!("当前构建只支持 http 认证页（没有启用 TLS）: {raw}");
        }
        if url.host_str().unwrap_or_default().is_empty() {
            bail!("认证页 URL 缺少主机名: {raw}");
        }
        Ok(url)
    }

    /// 认证页候选顺序：有线优先 .3，无线优先 .2；未知介质默认先试有线。
    pub fn portal_candidates(&self, medium: Medium) -> Vec<String> {
        let wireless = normalize_url(&self.portal_wireless);
        let wired = normalize_url(&self.portal_wired);
        match medium {
            Medium::Wireless => vec![wireless, wired],
            Medium::Wired | Medium::Other => vec![wired, wireless],
        }
    }

    pub fn validate_login(&self) -> Result<()> {
        if self.username.is_empty() {
            bail!("配置缺少 username");
        }
        if self.password.is_empty() {
            bail!("配置缺少 password");
        }
        if !self.sms_valid_at(now_ms()) {
            bail!("动态密码为空或已过期，请先运行 set-sms");
        }
        Ok(())
    }

    pub fn sms_valid_at(&self, now: i64) -> bool {
        if self.sms_code.is_empty() {
            return false;
        }
        if self.sms_hours < 0.0 {
            return true;
        }
        if self.sms_hours == 0.0 || self.sms_saved_at <= 0 {
            return false;
        }
        let remain = self.sms_hours * 3_600_000.0 - (now - self.sms_saved_at) as f64;
        remain > 0.0
    }

    pub fn sms_remaining_text(&self, now: i64) -> String {
        if self.sms_code.is_empty() {
            return "未设置".into();
        }
        if self.sms_hours < 0.0 {
            return "永久有效".into();
        }
        if self.sms_saved_at <= 0 {
            return "未开始计时".into();
        }
        let remain_min =
            ((self.sms_hours * 3_600_000.0 - (now - self.sms_saved_at) as f64) / 60_000.0).ceil();
        if remain_min <= 0.0 {
            "已过期".into()
        } else if remain_min >= 1440.0 {
            format!("剩余约 {:.1} 天", remain_min / 1440.0)
        } else if remain_min >= 60.0 {
            format!("剩余约 {:.1} 小时", remain_min / 60.0)
        } else {
            format!("剩余约 {remain_min:.0} 分钟")
        }
    }

    pub fn set_sms(&mut self, code: &str, hours: f64, now: i64) {
        self.sms_code = code.trim().to_string();
        self.sms_saved_at = if self.sms_code.is_empty() { 0 } else { now };
        self.sms_hours = hours;
    }

    pub fn clear_sms(&mut self) {
        self.sms_code.clear();
        self.sms_saved_at = 0;
    }
}

pub fn normalize_theme(raw: &str) -> String {
    match raw.trim() {
        "light" => "light".into(),
        "dark" => "dark".into(),
        _ => "system".into(),
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn default_paths(data_dir: &Path) -> (PathBuf, PathBuf) {
    let config = data_dir.join("config.json");
    let logs = data_dir.join("logs");
    (config, logs)
}

fn normalize_url(raw: &str) -> String {
    let mut url = raw.trim().to_string();
    if !url.is_empty() && !url.ends_with('/') {
        url.push('/');
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sms_expiry_works() {
        let mut cfg = Config::default();
        cfg.set_sms("123456", 1.0, 1_000_000);
        assert!(cfg.sms_valid_at(1_000_000 + 3_599_000));
        assert!(!cfg.sms_valid_at(1_000_000 + 3_601_000));
        cfg.sms_hours = -1.0;
        assert!(cfg.sms_valid_at(i64::MAX / 2));
        cfg.clear_sms();
        assert!(!cfg.sms_valid_at(1_000_000));
    }

    #[test]
    fn portal_requires_http() {
        assert!(Config::validate_portal_url("http://172.26.255.3/").is_ok());
        assert!(Config::validate_portal_url("https://example.com/").is_err());
    }

    #[test]
    fn candidate_order_matches_medium() {
        let cfg = Config::default();
        assert_eq!(
            cfg.portal_candidates(Medium::Wired)[0],
            "http://172.26.255.3/"
        );
        assert_eq!(
            cfg.portal_candidates(Medium::Wireless)[0],
            "http://172.26.255.2/"
        );
    }
}

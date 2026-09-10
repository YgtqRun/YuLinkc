//! 更新检查：只检查版本、只做提示，不下载也不安装。
//!
//! 数据来源按顺序尝试，成功即返回：
//!
//! 1. Release 资产里的静态清单 `version.json`
//!    （`https://github.com/<owner>/<repo>/releases/latest/download/version.json`）：
//!    走 CDN 重定向、没有 API 限流，推荐每次发版都顺手传一份；
//! 2. GitHub API `repos/<owner>/<repo>/releases/latest`：兜底用。未认证时限 60 次
//!    每小时每 IP，校园网出口 IP 共享时容易被打满，所以只在没有清单时才走。
//!
//! `version.json` 只要有 `version` 即可，另外支持可选的 `notes` 与 `url`
//! （例如 `{"version":"1.0.2","notes":"修复 xxx","url":"https://.../tag/v1.0.2"}`）。
//! Tauri 官方 updater 生成的 `latest.json` 也能识别（同样有 `version` / `notes`）。
//!
//! 网络走 WinHTTP（系统自带 TLS 与代理设置），不额外引入 HTTP 依赖。

use std::sync::Mutex;

use serde_json::Value;
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

/// Release 资产里的静态清单文件名。
const MANIFEST_ASSET: &str = "version.json";
/// 响应体上限，避免异常数据撑爆内存。
const MAX_BODY_BYTES: usize = 64 * 1024;
/// 更新说明的展示上限（GitHub 的 release 正文可能很长）。
const MAX_NOTES_CHARS: usize = 300;

/// 远端发布信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
}

/// 一次检查的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 远端版本更新。
    Newer(Release),
    /// 已是最新（`latest` 为远端版本号，便于界面显示）。
    UpToDate { latest: String },
    /// 检查失败（网络不通、仓库地址未配置、被限流等），`message` 是原因。
    Failed(String),
}

/// 进程内缓存：同一次运行只自动查一次，用户手动点“检查更新”可强制重查。
static CACHE: Mutex<Option<Outcome>> = Mutex::new(None);

/// 检查更新；`force` 为 true 时忽略缓存重新请求。
pub fn check(local_version: &str, force: bool) -> Outcome {
    if !force {
        if let Ok(cache) = CACHE.lock() {
            if let Some(cached) = cache.as_ref() {
                return cached.clone();
            }
        }
    }
    let outcome = fetch(local_version);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some(outcome.clone());
    }
    outcome
}

fn fetch(local_version: &str) -> Outcome {
    let Some((owner, repo)) = repo_coords(env!("YULINK_REPO_URL")) else {
        return Outcome::Failed("未配置 GitHub 仓库地址，无法检查更新".into());
    };
    let base = format!("https://github.com/{owner}/{repo}");
    let manifest_url = format!("{base}/releases/latest/download/{MANIFEST_ASSET}");
    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");

    // 1) 优先读取 Release 资产里的静态清单。
    match http_get(&manifest_url) {
        Ok((200, body)) => match release_from_manifest(&body, &base) {
            Ok(release) => return judge(local_version, release),
            Err(e) => log::warn!("更新清单解析失败: {e}"),
        },
        Ok((status, _)) => log::info!("更新清单不可用（HTTP {status}），改用 GitHub API"),
        Err(e) => log::warn!("更新清单请求失败: {e}"),
    }

    // 2) 兜底 GitHub API。
    match http_get(&api_url) {
        Ok((200, body)) => match release_from_api(&body) {
            Ok(release) => judge(local_version, release),
            Err(e) => Outcome::Failed(format!("解析 GitHub 响应失败: {e}")),
        },
        Ok((status, _)) => Outcome::Failed(format!("GitHub 返回 HTTP {status}")),
        Err(e) => Outcome::Failed(e),
    }
}

fn judge(local_version: &str, release: Release) -> Outcome {
    let mut release = release;
    release.notes = truncate(&release.notes, MAX_NOTES_CHARS);
    if is_newer(&release.version, local_version) {
        Outcome::Newer(release)
    } else {
        Outcome::UpToDate {
            latest: release.version,
        }
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max_chars).collect();
    format!("{cut}…")
}

// ===================== 解析 =====================

fn release_from_manifest(body: &str, base: &str) -> Result<Release, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .ok_or("缺少 version 字段")?
        .to_string();
    let notes = value
        .get("notes")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{base}/releases/latest"));
    Ok(Release {
        version,
        notes,
        url,
    })
}

fn release_from_api(body: &str) -> Result<Release, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let version = value
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or("缺少 tag_name 字段")?
        .to_string();
    let notes = value
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = value
        .get("html_url")
        .and_then(Value::as_str)
        .ok_or("缺少 html_url 字段")?
        .to_string();
    Ok(Release {
        version,
        notes,
        url,
    })
}

/// 从 git 远程地址解析 `(owner, repo)`；非 GitHub 地址返回 `None`。
pub fn repo_coords(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim().trim_end_matches('/');
    if remote.is_empty() {
        return None;
    }
    // 兼容 https://github.com/o/r(.git)、git@github.com:o/r(.git)、github.com/o/r
    let path = remote.split_once("github.com")?.1;
    let path = path.trim_start_matches([':', '/']);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?;
    Some((owner.to_string(), repo.to_string()))
}

/// 解析 `v1.2.3`、`1.2.3-beta.1` → (数字段, 预发布标签)。
fn parse_version(raw: &str) -> Option<(Vec<u64>, Option<String>)> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']).trim();
    let core_and_pre = trimmed.split('+').next().unwrap_or(trimmed);
    let (core, pre) = match core_and_pre.split_once('-') {
        Some((core, pre)) => (core, Some(pre.to_string())),
        None => (core_and_pre, None),
    };
    let mut numbers = Vec::new();
    for part in core.split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        numbers.push(digits.parse().ok()?);
    }
    Some((numbers, pre))
}

/// 远端版本是否比本地新：按数字段比较（1.0.10 > 1.0.9），同号时正式版高于预发布版。
pub fn is_newer(remote: &str, local: &str) -> bool {
    let (Some((remote_nums, remote_pre)), Some((local_nums, local_pre))) =
        (parse_version(remote), parse_version(local))
    else {
        return false;
    };
    for index in 0..remote_nums.len().max(local_nums.len()) {
        let remote_part = remote_nums.get(index).copied().unwrap_or(0);
        let local_part = local_nums.get(index).copied().unwrap_or(0);
        if remote_part != local_part {
            return remote_part > local_part;
        }
    }
    remote_pre.is_none() && local_pre.is_some()
}

// ===================== WinHTTP =====================

/// 持有 WinHTTP 句柄，Drop 时关闭。
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("只支持 https 地址: {url}"))?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host.is_empty() {
        return Err(format!("地址缺少主机名: {url}"));
    }
    Ok((host.to_string(), path))
}

fn query_status(request: *mut core::ffi::c_void) -> u32 {
    let mut status: u32 = 0;
    let mut length = std::mem::size_of::<u32>() as u32;
    let mut index: u32 = 0;
    let result = unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut core::ffi::c_void),
            &mut length,
            &mut index,
        )
    };
    if result.is_ok() {
        status
    } else {
        0
    }
}

/// 发起一次 HTTPS GET，返回 (HTTP 状态码, 响应体)。
fn http_get(url: &str) -> Result<(u32, String), String> {
    let (host, path) = split_url(url)?;
    // GitHub 要求带 User-Agent，这里用 WinHttpOpen 的 agent 充当。
    let agent = wide("YuLink-UpdateCheck");
    unsafe {
        let session = WinHttpOpen(
            PCWSTR::from_raw(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return Err("初始化网络会话失败".into());
        }
        let session = Handle(session);
        let _ = WinHttpSetTimeouts(session.0, 5_000, 5_000, 10_000, 15_000);
        // 只放行 https → https 的重定向（GitHub 资产会跳到 objects.githubusercontent.com）
        let policy = WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP.to_le_bytes();
        let _ = WinHttpSetOption(
            Some(session.0 as *const _),
            WINHTTP_OPTION_REDIRECT_POLICY,
            Some(&policy),
        );

        let host_wide = wide(&host);
        let connect = WinHttpConnect(session.0, PCWSTR::from_raw(host_wide.as_ptr()), 443, 0);
        if connect.is_null() {
            return Err(format!("连接 {host} 失败"));
        }
        let connect = Handle(connect);

        let verb = wide("GET");
        let path_wide = wide(&path);
        let request = WinHttpOpenRequest(
            connect.0,
            PCWSTR::from_raw(verb.as_ptr()),
            PCWSTR::from_raw(path_wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if request.is_null() {
            return Err("创建请求失败".into());
        }
        let request = Handle(request);

        WinHttpSendRequest(request.0, None, None, 0, 0, 0)
            .map_err(|e| format!("发送请求失败: {e}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| format!("接收响应失败: {e}"))?;

        let status = query_status(request.0);
        let mut body = Vec::new();
        loop {
            let mut available: u32 = 0;
            WinHttpQueryDataAvailable(request.0, &mut available)
                .map_err(|e| format!("读取响应失败: {e}"))?;
            if available == 0 {
                break;
            }
            let want = available.min(8 * 1024) as usize;
            let mut buffer = vec![0u8; want];
            let mut read: u32 = 0;
            WinHttpReadData(
                request.0,
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                want as u32,
                &mut read,
            )
            .map_err(|e| format!("读取响应失败: {e}"))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..read as usize]);
            if body.len() >= MAX_BODY_BYTES {
                break;
            }
        }
        Ok((status, String::from_utf8_lossy(&body).into_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_semantically() {
        assert!(is_newer("1.0.10", "1.0.9"));
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(is_newer("1.1", "1.0.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.1"));
        assert!(!is_newer("v1.0.0", "1.0.0"));
        // 正式版高于同号预发布版，反之不算更新
        assert!(is_newer("1.0.1", "1.0.1-beta.1"));
        assert!(!is_newer("1.0.1-beta.1", "1.0.1"));
        // 无法解析的输入一律视为“没有更新”
        assert!(!is_newer("latest", "1.0.0"));
        assert!(!is_newer("1.0.0", ""));
    }

    #[test]
    fn parses_repo_coordinates() {
        for remote in [
            "https://github.com/YgtqRun/YuLinkc.git",
            "https://github.com/YgtqRun/YuLinkc",
            "git@github.com:YgtqRun/YuLinkc.git",
            "https://github.com/YgtqRun/YuLinkc/",
        ] {
            assert_eq!(
                repo_coords(remote),
                Some(("YgtqRun".to_string(), "YuLinkc".to_string())),
                "remote = {remote}"
            );
        }
        assert_eq!(repo_coords(""), None);
        assert_eq!(repo_coords("https://gitee.com/foo/bar.git"), None);
    }

    #[test]
    fn parses_manifest_and_api_payloads() {
        let manifest = release_from_manifest(
            r#"{"version":"1.2.3","notes":"修了 A","url":"https://example.com/tag"}"#,
            "https://github.com/o/r",
        )
        .unwrap();
        assert_eq!(manifest.version, "1.2.3");
        assert_eq!(manifest.notes, "修了 A");
        assert_eq!(manifest.url, "https://example.com/tag");

        // 缺 url 时回落到 releases 页面；Tauri 的 latest.json 也能识别
        let latest_json = release_from_manifest(
            r#"{"version":"1.2.3","notes":"x","pub_date":"2026-01-01","platforms":{}}"#,
            "https://github.com/o/r",
        )
        .unwrap();
        assert_eq!(latest_json.url, "https://github.com/o/r/releases/latest");

        let api = release_from_api(
            r#"{"tag_name":"v1.2.3","body":"正文","html_url":"https://github.com/o/r/releases/tag/v1.2.3"}"#,
        )
        .unwrap();
        assert_eq!(api.version, "v1.2.3");
        assert_eq!(api.notes, "正文");

        assert!(release_from_manifest("{}", "https://github.com/o/r").is_err());
        assert!(release_from_api("not json").is_err());
    }

    #[test]
    fn splits_https_urls() {
        assert_eq!(
            split_url("https://github.com/o/r/releases/latest/download/version.json").unwrap(),
            (
                "github.com".to_string(),
                "/o/r/releases/latest/download/version.json".to_string()
            )
        );
        assert_eq!(
            split_url("https://api.github.com").unwrap(),
            ("api.github.com".to_string(), "/".to_string())
        );
        assert!(split_url("http://github.com/o/r").is_err());
    }

    /// 真实网络冒烟：默认不跑，需要时 `cargo test -- --ignored --nocapture`。
    #[test]
    #[ignore = "需要能访问 github.com"]
    fn live_check_smoke() {
        println!("{:?}", check("1.0.1", true));
    }
}

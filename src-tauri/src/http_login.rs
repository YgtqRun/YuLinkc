//! Dr.COM 直连登录（HTTP 新流程）。
//!
//! 与旧的 WebView2 填表流程并存，由设置项 `loginMode` 选择：
//! - `http`：直接请求 `/drcom/login`、`checkPassStatus`、`chkstatus`
//! - `webview`：沿用隐藏 WebView2 + 注入脚本
//!
//! 这里只复用现有 `network::http_get_with_referer`，不引入新的 HTTP 依赖。

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::auth_session::{LoginOutcome, LoginOutcomeKind};
use crate::network;

const TIMEOUT: Duration = Duration::from_secs(15);
static SESSION_SEQ: AtomicU32 = AtomicU32::new(1);

/// 执行 HTTP 登录流程。业务失败返回 `Ok(LoginOutcome)`，网络/解析异常返回 `Err`。
pub fn run_http_login(
    portal_url: &str,
    account: &str,
    password: &str,
    sms_code: &str,
    isp: &str,
    detect_only: bool,
) -> Result<LoginOutcome, String> {
    let portal = PortalEndpoints::parse(portal_url)?;
    log::info!("HTTP 登录流程启动: portal={portal_url}");

    let status = request_jsonp("chkstatus", &portal.chkstatus_url(), &portal.referer)?;
    if result_ok(&status) {
        return Ok(outcome(LoginOutcomeKind::AlreadyOnline));
    }
    if detect_only {
        return Ok(outcome(LoginOutcomeKind::LoginFormPresent));
    }

    if isp == "2" {
        let sms = request_jsonp(
            "checkPassStatus",
            &portal.check_sms_url(account, sms_code),
            &portal.referer,
        )?;
        if !result_ok(&sms) {
            return Ok(outcome(LoginOutcomeKind::SmsCodeFailed(message_or(
                &sms,
                "动态密码校验失败",
            ))));
        }
        log::info!("HTTP 动态密码校验通过");
    }

    let login = request_jsonp(
        "login",
        &portal.login_url(account, password, isp),
        &portal.referer,
    )?;
    if !result_ok(&login) {
        return Ok(outcome(LoginOutcomeKind::CredentialsFailed(message_or(
            &login,
            "登录被门户拒绝",
        ))));
    }
    log::info!("HTTP 登录请求已被门户接受，等待状态刷新");

    std::thread::sleep(Duration::from_millis(1500));
    let status = request_jsonp("chkstatus", &portal.chkstatus_url(), &portal.referer)?;
    if result_ok(&status) {
        log::info!("HTTP 登录确认成功");
        Ok(outcome(LoginOutcomeKind::Ok))
    } else {
        Ok(outcome(LoginOutcomeKind::PageError(
            "门户已接受登录，但状态仍未在线".into(),
        )))
    }
}

fn outcome(kind: LoginOutcomeKind) -> LoginOutcome {
    LoginOutcome { kind }
}

struct PortalEndpoints {
    authority: String,
    referer: String,
    drcom: String,
    eportal: String,
}

impl PortalEndpoints {
    fn parse(portal_url: &str) -> Result<Self, String> {
        let trimmed = portal_url.trim().trim_end_matches('/');
        let rest = trimmed
            .strip_prefix("http://")
            .ok_or_else(|| "HTTP 登录只支持 http:// 认证页".to_string())?;
        let (authority, _) = rest.split_once('/').unwrap_or((rest, ""));
        if authority.is_empty() {
            return Err("认证页 URL 缺少主机名".into());
        }
        let host = authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .unwrap_or(authority);
        Ok(Self {
            authority: authority.to_string(),
            referer: format!("http://{authority}/"),
            drcom: format!("http://{authority}/drcom/"),
            eportal: format!("http://{host}:801/eportal/"),
        })
    }

    fn chkstatus_url(&self) -> String {
        format!(
            "{}chkstatus?callback={}&v={}",
            self.drcom,
            next_callback(),
            next_v()
        )
    }

    fn check_sms_url(&self, account: &str, sms_code: &str) -> String {
        format!(
            "{}?callback={}&c=Portal&a=checkPassStatus&username={}&pass={}&category=2&v={}",
            self.eportal,
            next_callback(),
            encode(account),
            encode(sms_code),
            next_v()
        )
    }

    fn login_url(&self, account: &str, password: &str, isp: &str) -> String {
        format!(
            "{}login?callback={}&DDDDD={}&upass={}&0MKKey=123456&R1=0&R3={}&R6=0&para=00&v6ip=&v={}",
            self.drcom,
            next_callback(),
            encode(account),
            encode(password),
            encode(isp),
            next_v()
        )
    }

    #[allow(dead_code)]
    fn authority(&self) -> &str {
        &self.authority
    }
}

fn request_jsonp(name: &str, url: &str, referer: &str) -> Result<Value, String> {
    let (status, body) = network::http_get_with_referer(url, TIMEOUT, Some(referer))
        .ok_or_else(|| format!("{name} 请求失败（认证页不可达）"))?;
    if !(200..300).contains(&status) {
        return Err(format!("{name} 返回 HTTP {status}"));
    }
    let value = parse_jsonp(&body).map_err(|e| format!("{name} 响应解析失败: {e}"))?;
    log::info!(
        "HTTP {name}: result={} msg={}",
        value_text(value.get("result")),
        value_text(value.get("msg"))
    );
    Ok(value)
}

fn parse_jsonp(body: &str) -> Result<Value, String> {
    let trimmed = body.trim();
    let json_text = if trimmed.starts_with('{') {
        trimmed
    } else {
        let start = trimmed
            .find('(')
            .ok_or_else(|| "缺少 JSONP 左括号".to_string())?;
        let end = trimmed
            .rfind(')')
            .ok_or_else(|| "缺少 JSONP 右括号".to_string())?;
        if end <= start {
            return Err("JSONP 括号顺序异常".into());
        }
        trimmed[start + 1..end].trim()
    };
    serde_json::from_str(json_text).map_err(|e| e.to_string())
}

fn result_ok(value: &Value) -> bool {
    match value.get("result") {
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("ok"),
        Some(Value::Bool(v)) => *v,
        _ => false,
    }
}

fn message_or(value: &Value, fallback: &str) -> String {
    value
        .get("msg")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Null) | None => "<none>".into(),
        Some(other) => other.to_string(),
    }
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn next_callback() -> String {
    format!("dr{}", 1000 + (next_v() % 9000))
}

fn next_v() -> u32 {
    let time_part = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed);
    1000 + ((time_part + seq) % 9000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonp_and_plain_json() {
        assert_eq!(
            parse_jsonp("  dr1006({\"result\":1})  ").unwrap(),
            serde_json::json!({"result": 1})
        );
        assert_eq!(
            parse_jsonp("{\"result\":0}").unwrap(),
            serde_json::json!({"result": 0})
        );
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode("Zjyc@050079"), "Zjyc%40050079");
        assert_eq!(encode("a b+c"), "a%20b%2Bc");
    }

    #[test]
    fn parses_portal_endpoints() {
        let p = PortalEndpoints::parse("http://172.26.255.3/").unwrap();
        assert_eq!(p.referer, "http://172.26.255.3/");
        assert_eq!(p.drcom, "http://172.26.255.3/drcom/");
        assert_eq!(p.eportal, "http://172.26.255.3:801/eportal/");
    }

    #[test]
    fn accepts_numeric_and_string_results() {
        assert!(result_ok(&serde_json::json!({"result": 1})));
        assert!(result_ok(&serde_json::json!({"result": "1"})));
        assert!(result_ok(&serde_json::json!({"result": "ok"})));
        assert!(!result_ok(&serde_json::json!({"result": 0})));
    }
}

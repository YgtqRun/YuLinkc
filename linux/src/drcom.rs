use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use url::Url;

use crate::config::Config;

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

static RANDOM_SEQ: AtomicU32 = AtomicU32::new(1);

#[derive(Debug, Clone)]
pub struct PortalReply {
    pub result: Value,
    pub msg: Option<String>,
}

impl PortalReply {
    pub fn ok(&self) -> bool {
        result_ok(&self.result)
    }

    pub fn msg_text(&self) -> String {
        self.msg.clone().unwrap_or_default()
    }

    pub fn result_text(&self) -> String {
        match &self.result {
            Value::Null => "<none>".into(),
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            other => other.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginOutcome {
    AlreadyOnline,
    LoggedIn,
}

pub struct DrcomClient {
    http: Client,
    portal: Url,
    drcom_base: Url,
    eportal_base: Url,
    username: String,
    password: String,
    sms_code: String,
    isp: String,
}

impl DrcomClient {
    pub fn new(cfg: &Config, portal_url: &str) -> Result<Self> {
        let portal = Config::validate_portal_url(portal_url)?;
        let host = portal
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("认证页 URL 缺少主机名"))?;
        let eportal_base = Url::parse(&format!(
            "{}://{}:{}/eportal/",
            portal.scheme(),
            host,
            cfg.eportal_port
        ))
        .context("构造 eportal URL 失败")?;
        let drcom_base = portal.join("drcom/").context("构造 drcom URL 失败")?;
        let http = Client::builder()
            .redirect(Policy::limited(5))
            .timeout(REQUEST_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .context("创建 HTTP 客户端失败")?;
        Ok(Self {
            http,
            portal,
            drcom_base,
            eportal_base,
            username: cfg.username.clone(),
            password: cfg.password.clone(),
            sms_code: cfg.sms_code.clone(),
            isp: cfg.isp.clone(),
        })
    }

    pub async fn warmup(&self) -> Result<StatusCode> {
        let resp = self
            .http
            .get(self.portal.clone())
            .header("Referer", self.portal.as_str())
            .send()
            .await
            .context("打开认证页失败")?;
        let status = resp.status();
        log::info!("WARMUP: HTTP {}", status.as_u16());
        Ok(status)
    }

    pub async fn chkstatus(&self) -> Result<PortalReply> {
        let url = self.drcom_base.join("chkstatus")?;
        self.get_jsonp("chkstatus", url, &[]).await
    }

    pub async fn logout(&self) -> Result<PortalReply> {
        let url = self.drcom_base.join("logout")?;
        match self.get_jsonp("logout(drcom)", url, &[]).await {
            Ok(reply) if reply.ok() => return Ok(reply),
            Ok(reply) => log::warn!(
                "drcom/logout 返回失败 result={}，尝试 eportal logout",
                reply.result_text()
            ),
            Err(e) => log::warn!("drcom/logout 请求失败: {e}，尝试 eportal logout"),
        }
        let query = vec![
            ("c".to_string(), "Portal".to_string()),
            ("a".to_string(), "logout".to_string()),
        ];
        self.get_jsonp("logout(eportal)", self.eportal_base.clone(), &query)
            .await
    }

    pub async fn check_sms(&self) -> Result<PortalReply> {
        if self.sms_code.is_empty() {
            bail!("动态密码为空");
        }
        let query = vec![
            ("c".to_string(), "Portal".to_string()),
            ("a".to_string(), "checkPassStatus".to_string()),
            ("username".to_string(), self.username.clone()),
            ("pass".to_string(), self.sms_code.clone()),
            ("category".to_string(), "2".to_string()),
        ];
        self.get_jsonp("checkPassStatus", self.eportal_base.clone(), &query)
            .await
    }

    pub async fn login(&self) -> Result<PortalReply> {
        let query = vec![
            ("DDDDD".to_string(), self.username.clone()),
            ("upass".to_string(), self.password.clone()),
            ("0MKKey".to_string(), "123456".to_string()),
            ("R1".to_string(), "0".to_string()),
            ("R3".to_string(), self.isp.clone()),
            ("R6".to_string(), "0".to_string()),
            ("para".to_string(), "00".to_string()),
            ("v6ip".to_string(), String::new()),
        ];
        let url = self.drcom_base.join("login")?;
        self.get_jsonp("login", url, &query).await
    }

    pub async fn login_flow(&self) -> Result<LoginOutcome> {
        let status = self.chkstatus().await?;
        if status.ok() {
            return Ok(LoginOutcome::AlreadyOnline);
        }
        if self.isp == "2" {
            let sms = self.check_sms().await?;
            if !sms.ok() {
                bail!("动态密码校验失败: {}", sms.msg_text());
            }
            log::info!("动态密码校验通过");
        }
        let login = self.login().await?;
        if !login.ok() {
            bail!("登录被门户拒绝: {}", login.msg_text());
        }
        log::info!("登录请求已被门户接受，等待状态刷新");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status = self.chkstatus().await?;
        if !status.ok() {
            bail!("登录返回成功，但 chkstatus 仍显示离线");
        }
        log::info!("登录确认成功");
        Ok(LoginOutcome::LoggedIn)
    }

    async fn get_jsonp(
        &self,
        name: &str,
        mut url: Url,
        query: &[(String, String)],
    ) -> Result<PortalReply> {
        let v = next_random();
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("callback", &format!("dr{v}"));
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
            pairs.append_pair("v", &v.to_string());
        }
        log::info!(
            "REQUEST {name}: GET {} ({} params)",
            url.path(),
            query.len()
        );
        let resp = self
            .http
            .get(url.clone())
            .header("Referer", self.portal.as_str())
            .header("Accept", "*/*")
            .send()
            .await
            .with_context(|| format!("{name} 请求失败"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .with_context(|| format!("{name} 读取响应失败"))?;
        if !status.is_success() {
            bail!("{name} 返回 HTTP {}: {}", status.as_u16(), snippet(&body));
        }
        let json_text = extract_jsonp(&body)
            .with_context(|| format!("{name} 响应不是 JSONP: {}", snippet(&body)))?;
        let raw: Value = serde_json::from_str(&json_text)
            .with_context(|| format!("{name} JSON 解析失败: {}", snippet(&json_text)))?;
        let result = raw.get("result").cloned().unwrap_or(Value::Null);
        let msg = raw.get("msg").and_then(Value::as_str).map(str::to_string);
        log::info!(
            "RESPONSE {name}: HTTP {} result={} msg={}",
            status.as_u16(),
            value_text(&result),
            msg.as_deref().unwrap_or("")
        );
        Ok(PortalReply { result, msg })
    }
}

fn next_random() -> u32 {
    let time_part = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    let seq = RANDOM_SEQ.fetch_add(1, Ordering::Relaxed);
    1000 + ((time_part + seq) % 9000)
}

fn extract_jsonp(body: &str) -> Result<String> {
    let trimmed = body.trim();
    if trimmed.starts_with('{') {
        return Ok(trimmed.to_string());
    }
    let open = trimmed
        .find('(')
        .ok_or_else(|| anyhow::anyhow!("缺少 JSONP 左括号"))?;
    let close = trimmed
        .rfind(')')
        .ok_or_else(|| anyhow::anyhow!("缺少 JSONP 右括号"))?;
    if close <= open {
        bail!("JSONP 括号顺序异常");
    }
    Ok(trimmed[open + 1..close].trim().to_string())
}

fn result_ok(value: &Value) -> bool {
    match value {
        Value::Number(n) => n.as_i64() == Some(1),
        Value::String(s) => s == "1" || s.eq_ignore_ascii_case("ok"),
        Value::Bool(b) => *b,
        _ => false,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Null => "<none>".into(),
        other => other.to_string(),
    }
}

fn snippet(text: &str) -> String {
    text.trim().chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonp_and_plain_json() {
        assert_eq!(
            extract_jsonp("  dr1006({\"result\":1})  ").unwrap(),
            "{\"result\":1}"
        );
        assert_eq!(extract_jsonp("{\"result\":0}").unwrap(), "{\"result\":0}");
    }

    #[test]
    fn accepts_numeric_and_string_results() {
        assert!(result_ok(&serde_json::json!(1)));
        assert!(result_ok(&serde_json::json!("1")));
        assert!(result_ok(&serde_json::json!("ok")));
        assert!(!result_ok(&serde_json::json!(0)));
        assert!(!result_ok(&serde_json::json!("0")));
    }
}

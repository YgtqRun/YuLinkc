use std::time::Duration;

use reqwest::Client;

use crate::config::Config;
use crate::network::{self, Medium};

const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36";
const FALLBACK_PROBES: [&str; 2] = [
    "http://connect.rom.miui.com/generate_204",
    "http://cp.cloudflare.com/generate_204",
];

pub fn client() -> reqwest::Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(5))
        .user_agent(USER_AGENT)
        .build()
}

pub async fn external_online(http: &Client, primary: &str) -> (bool, String) {
    let mut urls = vec![primary.to_string()];
    urls.extend(FALLBACK_PROBES.iter().map(|s| s.to_string()));
    let mut reasons = Vec::with_capacity(urls.len());
    for url in urls {
        match http.get(&url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.as_u16() == 204 {
                    return (true, format!("{url}: HTTP 204"));
                }
                let body = resp.text().await.unwrap_or_default();
                if status.is_success() && body.contains("Microsoft Connect Test") {
                    return (true, format!("{url}: HTTP 200 connect test"));
                }
                reasons.push(format!(
                    "{url}: HTTP {} ({})",
                    status.as_u16(),
                    snippet(&body)
                ));
            }
            Err(e) => reasons.push(format!("{url}: {e}")),
        }
    }
    (false, reasons.join("; "))
}

pub async fn portal_reachable(http: &Client, portal: &str) -> bool {
    match http.get(portal).send().await {
        Ok(resp) => {
            let code = resp.status().as_u16();
            (200..500).contains(&code)
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone)]
pub struct PortalChoice {
    pub url: String,
    pub reachable: bool,
    pub medium: Medium,
}

/// Pick the portal from the two configured URLs.
///
/// Ordering is based on the default-route interface: wired first for Ethernet,
/// wireless first for Wi-Fi. If the first URL is unreachable, try the other one.
pub async fn choose_portal(cfg: &Config, http: &Client, forced: Option<&str>) -> PortalChoice {
    let medium = network::detect_medium();
    if let Some(forced) = forced.filter(|url| !url.trim().is_empty()) {
        let reachable = portal_reachable(http, forced).await;
        log::info!(
            "使用手动指定认证页 {}（medium={}, reachable={}）",
            forced,
            medium.label(),
            reachable
        );
        return PortalChoice {
            url: forced.to_string(),
            reachable,
            medium,
        };
    }

    let candidates = cfg.portal_candidates(medium);
    for url in &candidates {
        if portal_reachable(http, url).await {
            log::info!("自动选择认证页 {}（medium={}）", url, medium.label());
            return PortalChoice {
                url: url.clone(),
                reachable: true,
                medium,
            };
        }
        log::warn!("认证页不可达: {url}");
    }
    let fallback = candidates.first().cloned().unwrap_or_default();
    log::warn!(
        "有线/无线认证页都不可达，暂用 {}（medium={}）",
        fallback,
        medium.label()
    );
    PortalChoice {
        url: fallback,
        reachable: false,
        medium,
    }
}

fn snippet(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}

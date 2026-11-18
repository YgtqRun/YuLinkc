use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::{now_ms, Config};
use crate::drcom::{DrcomClient, LoginOutcome};
use crate::probe;

const GATE_INTERVAL_SEC: u64 = 8;
const OFFLINE_GATE_FAILS: u32 = 2;

#[allow(unreachable_code)]
pub async fn run(cfg: Config, forced_portal: Option<String>) -> Result<()> {
    let probe_http = probe::client()?;
    let mut gate_fail = 0u32;
    let mut login_fail = 0u32;
    let mut first_connect_done = false;
    let mut next_full = Instant::now();
    let mut last_gate_online: Option<bool> = None;

    log::info!(
        "常驻模式启动：无线={} 有线={} user={} sms={}",
        cfg.portal_wireless,
        cfg.portal_wired,
        mask(&cfg.username),
        cfg.sms_remaining_text(now_ms())
    );
    log::warn!("当前配置为本地 JSON 明文存储；请不要把 data/ 目录交给他人");

    let _ = tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_ok() {
            log::info!("收到 Ctrl-C，退出");
            std::process::exit(0);
        }
    });

    loop {
        tokio::time::sleep(Duration::from_secs(GATE_INTERVAL_SEC)).await;

        if cfg.away_mode {
            if Instant::now() >= next_full {
                log::info!("离校模式：暂停自动认证");
                next_full = Instant::now() + Duration::from_secs(300);
            }
            continue;
        }

        let (online, reason) = probe::external_online(&probe_http, &cfg.external_probe).await;
        if online {
            if last_gate_online == Some(false) {
                log::info!("外网心跳恢复在线");
            }
            if last_gate_online != Some(true) {
                next_full = Instant::now();
            }
            last_gate_online = Some(true);
            gate_fail = 0;
        } else {
            gate_fail = gate_fail.saturating_add(1);
            if gate_fail <= OFFLINE_GATE_FAILS {
                log::warn!("外网心跳失败（{gate_fail}/{OFFLINE_GATE_FAILS}）: {reason}");
            }
            if gate_fail == OFFLINE_GATE_FAILS {
                next_full = Instant::now();
            }
            last_gate_online = Some(false);
        }

        if Instant::now() >= next_full {
            let wait = full_check(
                &cfg,
                &probe_http,
                forced_portal.as_deref(),
                &mut login_fail,
                &mut first_connect_done,
            )
            .await;
            next_full = Instant::now() + Duration::from_secs(wait);
        }
    }
    Ok(())
}

async fn full_check(
    cfg: &Config,
    probe_http: &reqwest::Client,
    forced_portal: Option<&str>,
    login_fail: &mut u32,
    first_connect_done: &mut bool,
) -> u64 {
    if cfg.away_mode {
        log::info!("离校模式：暂停自动认证");
        return 300;
    }
    if cfg.username.is_empty() || cfg.password.is_empty() {
        log::error!("配置缺少账号或密码");
        return 60;
    }

    let (online, reason) = probe::external_online(probe_http, &cfg.external_probe).await;
    if online {
        log::info!("外网在线：{reason}");
        *login_fail = 0;
        *first_connect_done = true;
        return cfg.check_interval_online_sec.max(10);
    }
    log::warn!("外网离线：{reason}");

    if !cfg.sms_valid_at(now_ms()) {
        log::warn!(
            "动态密码不可用（{}），暂停自动登录，请更新动态密码",
            cfg.sms_remaining_text(now_ms())
        );
        return 60;
    }

    let choice = probe::choose_portal(cfg, probe_http, forced_portal).await;
    if !choice.reachable {
        log::warn!("有线 / 无线认证页都不可达，等待网络恢复");
        return cfg.check_interval_offline_sec.max(10);
    }

    let client = match DrcomClient::new(cfg, &choice.url) {
        Ok(client) => client,
        Err(e) => {
            log::error!("创建门户客户端失败: {e}");
            return cfg.check_interval_offline_sec.max(10);
        }
    };

    if !cfg.auto_relogin && *first_connect_done {
        return detect_only(cfg, &client).await;
    }

    if let Err(e) = client.warmup().await {
        log::warn!("打开认证页失败（继续尝试登录）: {e}");
    }

    match client.login_flow().await {
        Ok(LoginOutcome::AlreadyOnline) => {
            log::info!("认证页显示已在线，不重复登录");
            *login_fail = 0;
            *first_connect_done = true;
            cfg.check_interval_online_sec.max(10)
        }
        Ok(LoginOutcome::LoggedIn) => {
            log::info!("自动登录成功");
            *login_fail = 0;
            *first_connect_done = true;
            cfg.check_interval_online_sec.max(10)
        }
        Err(e) => {
            *login_fail = login_fail.saturating_add(1);
            log::error!("自动登录失败（第 {} 次）: {e}", *login_fail);
            backoff_secs(*login_fail)
        }
    }
}

async fn detect_only(cfg: &Config, client: &DrcomClient) -> u64 {
    match client.chkstatus().await {
        Ok(reply) if reply.ok() => {
            log::info!("检测模式：门户仍在线");
            cfg.check_interval_online_sec.max(10)
        }
        Ok(reply) => {
            log::warn!(
                "检测模式：门户离线（result={}），自动登录已关闭",
                reply.result_text()
            );
            cfg.check_interval_offline_sec.max(10)
        }
        Err(e) => {
            log::warn!("检测模式：状态查询失败: {e}");
            cfg.check_interval_offline_sec.max(10)
        }
    }
}

fn backoff_secs(fail: u32) -> u64 {
    let exp = fail.saturating_sub(1).min(3);
    30u64 * 2u64.pow(exp)
}

fn mask(value: &str) -> String {
    if value.is_empty() {
        return "<missing>".into();
    }
    if value.chars().count() <= 4 {
        return "***".into();
    }
    let head: String = value.chars().take(4).collect();
    format!("{head}***")
}

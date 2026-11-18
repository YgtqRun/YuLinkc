mod config;
mod drcom;
mod logger;
mod network;
mod probe;
mod scheduler;
mod web;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use log::LevelFilter;

use crate::config::{now_ms, Config};
use crate::drcom::{DrcomClient, LoginOutcome};

const USAGE: &str = r#"Usage: yulink-linux [options] <command>

Commands:
  init [--force]        创建默认配置
  status               查询门户在线状态
  logout               注销当前会话
  login                校验动态密码并登录
  once                 探外网；离线则登录一次
  run                  常驻：心跳 + 自动重登
  serve                启动 Web 设置页和 API
  set-sms <code> [h]   保存动态密码并从现在开始计时
  clear-sms            清除动态密码

Options:
  --data-dir <dir>     数据目录，默认 ./data
  --config <file>      配置文件，默认 <data-dir>/config.json
  --portal <url>       临时覆盖认证页，例如 http://172.26.255.3/
  --eportal-port <n>   临时覆盖 eportal 端口，默认 801
  --www-dir <dir>      Web 静态文件目录，默认 ./www
  --bind <addr>        Web 监听地址，默认 0.0.0.0
  --port <n>           Web 端口，默认 8080
  --unix-socket <path> 用 Unix socket 提供统一网关服务
  -h, --help           显示帮助
  --version            显示版本
"#;

enum Command {
    Init,
    Status,
    Logout,
    Login,
    Once,
    Run,
    Serve,
    SetSms { code: String, hours: f64 },
    ClearSms,
}

struct Cli {
    data_dir: PathBuf,
    config_path: PathBuf,
    portal_override: Option<String>,
    eportal_port_override: Option<u16>,
    www_dir: PathBuf,
    bind: String,
    port: u16,
    unix_socket: Option<PathBuf>,
    force: bool,
    command: Command,
}

fn main() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("ERROR: 创建异步运行时失败: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = runtime.block_on(run()) {
        eprintln!("ERROR: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = parse_args()?;
    fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("创建数据目录失败: {}", cli.data_dir.display()))?;
    let (_, log_dir) = config::default_paths(&cli.data_dir);
    logger::FileLogger::init(&log_dir, LevelFilter::Info, true).map_err(|e| anyhow::anyhow!(e))?;
    log::info!("yulink-linux v{}", env!("CARGO_PKG_VERSION"));

    match &cli.command {
        Command::Init => {
            if cli.config_path.exists() && !cli.force {
                bail!(
                    "配置文件已存在: {}（加 --force 覆盖）",
                    cli.config_path.display()
                );
            }
            Config::default().save(&cli.config_path)?;
            println!("已创建配置: {}", cli.config_path.display());
            println!("请填写账号和密码，然后运行:");
            println!(
                "  {} set-sms <动态密码> 27",
                std::env::args()
                    .next()
                    .unwrap_or_else(|| "yulink-linux".into())
            );
            return Ok(());
        }
        Command::SetSms { code, hours } => {
            if *hours <= 0.0 && *hours != -1.0 {
                bail!("有效期必须大于 0 小时，或使用 -1 表示永久");
            }
            let mut cfg = Config::load(&cli.config_path)?;
            cfg.set_sms(code, *hours, now_ms());
            cfg.save(&cli.config_path)?;
            println!(
                "动态密码已保存：{}，{}",
                cfg.sms_remaining_text(now_ms()),
                if *hours < 0.0 {
                    "永久"
                } else {
                    "从现在开始计时"
                }
            );
            return Ok(());
        }
        Command::ClearSms => {
            let mut cfg = Config::load(&cli.config_path)?;
            cfg.clear_sms();
            cfg.save(&cli.config_path)?;
            println!("动态密码已清除");
            return Ok(());
        }
        _ => {}
    }

    let mut cfg = match Config::load(&cli.config_path) {
        Ok(cfg) => cfg,
        Err(_) if matches!(&cli.command, Command::Serve) => {
            let cfg = Config::default();
            cfg.save(&cli.config_path)?;
            cfg
        }
        Err(e) => return Err(e),
    };
    if let Some(port) = cli.eportal_port_override {
        cfg.eportal_port = port;
    }
    cfg.normalize();
    let forced_portal = cli.portal_override.clone();
    // Hand-edited config with a code but no timestamp: start the clock now.
    if !cfg.sms_code.is_empty() && cfg.sms_saved_at <= 0 {
        cfg.sms_saved_at = now_ms();
        cfg.save(&cli.config_path)?;
        log::warn!("配置里的动态密码没有计时，已从当前时间开始计算");
    }

    match cli.command {
        Command::Status => {
            let (client, choice) = selected_client(&cfg, forced_portal.as_deref()).await?;
            client.warmup().await?;
            let reply = client.chkstatus().await?;
            println!(
                "portal[{}] (medium={}): {}{}",
                choice.url,
                choice.medium.label(),
                if reply.ok() { "ONLINE" } else { "OFFLINE" },
                if reply.ok() {
                    String::new()
                } else {
                    format!(
                        " (result={}, msg={})",
                        reply.result_text(),
                        reply.msg_text()
                    )
                }
            );
        }
        Command::Logout => {
            let (client, _) = selected_client(&cfg, forced_portal.as_deref()).await?;
            let reply = client.logout().await?;
            if !reply.ok() {
                bail!(
                    "注销失败: result={}, msg={}",
                    reply.result_text(),
                    reply.msg_text()
                );
            }
            println!("logout: OK");
        }
        Command::Login => {
            cfg.validate_login()?;
            let (client, _) = selected_client(&cfg, forced_portal.as_deref()).await?;
            client.warmup().await?;
            match client.login_flow().await? {
                LoginOutcome::AlreadyOnline => println!("login: ALREADY_ONLINE"),
                LoginOutcome::LoggedIn => println!("login: OK"),
            }
        }
        Command::Once => {
            let probe_http = probe::client()?;
            let (online, reason) = probe::external_online(&probe_http, &cfg.external_probe).await;
            if online {
                println!("connected: external ({reason})");
                return Ok(());
            }
            let choice = probe::choose_portal(&cfg, &probe_http, forced_portal.as_deref()).await;
            if !choice.reachable {
                bail!("认证页不可达（有线 .3 / 无线 .2 都未响应）");
            }
            let client = DrcomClient::new(&cfg, &choice.url)?;
            let status = client.chkstatus().await?;
            if status.ok() {
                println!(
                    "connected: portal[{}] (external probe: {reason})",
                    choice.url
                );
                return Ok(());
            }
            cfg.validate_login()?;
            client.warmup().await?;
            match client.login_flow().await? {
                LoginOutcome::AlreadyOnline => println!("login: ALREADY_ONLINE"),
                LoginOutcome::LoggedIn => {
                    println!("login: OK");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let (online, reason) =
                        probe::external_online(&probe_http, &cfg.external_probe).await;
                    println!(
                        "external: {} ({reason})",
                        if online { "ONLINE" } else { "NOT_CONFIRMED" }
                    );
                }
            }
        }
        Command::Run => {
            if cfg.username.is_empty() || cfg.password.is_empty() {
                bail!("配置缺少账号或密码: {}", cli.config_path.display());
            }
            scheduler::run(cfg, forced_portal).await?;
        }
        Command::Serve => {
            let state = web::WebState {
                config: Arc::new(Mutex::new(cfg)),
                config_path: cli.config_path.clone(),
                forced_portal,
            };
            if let Some(socket) = cli.unix_socket.clone() {
                #[cfg(unix)]
                web::serve_unix(state, cli.www_dir.clone(), socket).await?;
                #[cfg(not(unix))]
                {
                    let _ = (state, socket);
                    bail!("--unix-socket 仅支持 Linux");
                }
            } else {
                web::serve(state, cli.www_dir.clone(), cli.bind.clone(), cli.port).await?;
            }
        }
        Command::Init | Command::SetSms { .. } | Command::ClearSms => unreachable!(),
    }
    Ok(())
}

async fn selected_client(
    cfg: &Config,
    forced_portal: Option<&str>,
) -> Result<(DrcomClient, probe::PortalChoice)> {
    let probe_http = probe::client()?;
    let choice = probe::choose_portal(cfg, &probe_http, forced_portal).await;
    if !choice.reachable {
        bail!("认证页不可达（有线 .3 / 无线 .2 都未响应）");
    }
    let client = DrcomClient::new(cfg, &choice.url)?;
    Ok((client, choice))
}

fn parse_args() -> Result<Cli> {
    let mut args = std::env::args().skip(1).peekable();
    let mut data_dir = PathBuf::from("./data");
    let mut config_path: Option<PathBuf> = None;
    let mut portal_override = None;
    let mut eportal_port_override = None;
    let mut www_dir = PathBuf::from("./www");
    let mut bind = "0.0.0.0".to_string();
    let mut port = 8080u16;
    let mut unix_socket = None;
    let mut command: Option<Command> = None;
    let mut force = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--version" => {
                println!("yulink-linux {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--data-dir" => {
                data_dir = PathBuf::from(next_value(&mut args, "--data-dir")?);
            }
            "--config" => {
                config_path = Some(PathBuf::from(next_value(&mut args, "--config")?));
            }
            "--portal" => {
                portal_override = Some(next_value(&mut args, "--portal")?);
            }
            "--eportal-port" => {
                let raw = next_value(&mut args, "--eportal-port")?;
                eportal_port_override = Some(
                    raw.parse::<u16>()
                        .with_context(|| format!("无效端口: {raw}"))?,
                );
            }
            "--www-dir" => {
                www_dir = PathBuf::from(next_value(&mut args, "--www-dir")?);
            }
            "--bind" => {
                bind = next_value(&mut args, "--bind")?;
            }
            "--port" => {
                let raw = next_value(&mut args, "--port")?;
                port = raw
                    .parse::<u16>()
                    .with_context(|| format!("无效端口: {raw}"))?;
            }
            "--unix-socket" => {
                unix_socket = Some(PathBuf::from(next_value(&mut args, "--unix-socket")?));
            }
            "--force" => force = true,
            "init" => command = Some(Command::Init),
            "status" => command = Some(Command::Status),
            "logout" => command = Some(Command::Logout),
            "login" => command = Some(Command::Login),
            "once" => command = Some(Command::Once),
            "run" => command = Some(Command::Run),
            "serve" => command = Some(Command::Serve),
            "clear-sms" => command = Some(Command::ClearSms),
            "set-sms" => {
                let code = next_value(&mut args, "set-sms <code>")?;
                let hours = next_optional_hours(&mut args)?;
                command = Some(Command::SetSms { code, hours });
            }
            other if other.starts_with('-') => {
                bail!("未知参数: {other}\n\n{USAGE}");
            }
            other => bail!("未知命令: {other}\n\n{USAGE}"),
        }
    }

    let config_path = config_path.unwrap_or_else(|| data_dir.join("config.json"));
    let command = match command {
        Some(cmd) => cmd,
        None => {
            println!("{USAGE}");
            std::process::exit(2);
        }
    };
    Ok(Cli {
        data_dir,
        config_path,
        portal_override,
        eportal_port_override,
        www_dir,
        bind,
        port,
        unix_socket,
        force,
        command,
    })
}

fn next_value<I>(args: &mut std::iter::Peekable<I>, name: &str) -> Result<String>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| anyhow::anyhow!("{name} 缺少参数值"))
}

fn next_optional_hours<I>(args: &mut std::iter::Peekable<I>) -> Result<f64>
where
    I: Iterator<Item = String>,
{
    let Some(next) = args.peek() else {
        return Ok(27.0);
    };
    if let Ok(hours) = next.parse::<f64>() {
        args.next();
        return Ok(hours);
    }
    Ok(27.0)
}

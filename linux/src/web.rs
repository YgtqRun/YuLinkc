use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::config::{normalize_theme, now_ms, Config};
use crate::drcom::{DrcomClient, LoginOutcome};
use crate::probe;

const GATEWAY_PREFIX: &str = "/app/yulink";

#[derive(Clone)]
pub struct WebState {
    pub config: Arc<Mutex<Config>>,
    pub config_path: PathBuf,
    pub forced_portal: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusView {
    kind: String,
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsView {
    account_configured: bool,
    username: String,
    password: String,
    isp: String,
    has_sms_code: bool,
    sms_code: String,
    sms_hours: f64,
    sms_saved_at: i64,
    sms_status_text: String,
    autostart: bool,
    startup_boost: bool,
    away_mode: bool,
    show_auth_window: bool,
    auto_relogin: bool,
    quit_after_first_connect: bool,
    theme: String,
    login_mode: String,
    background_url: Option<String>,
    portal_wireless: String,
    portal_wired: String,
    status: StatusView,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfoView {
    version: String,
    repo_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandResult {
    ok: bool,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountInput {
    username: String,
    password: String,
    isp: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SmsInput {
    code: String,
    hours: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSettingsRequest {
    account: Option<AccountInput>,
    sms: Option<SmsInput>,
    portal_wireless: Option<String>,
    portal_wired: Option<String>,
    away_mode: Option<bool>,
    auto_relogin: Option<bool>,
    theme: Option<String>,
    #[allow(dead_code)]
    login_mode: Option<String>,
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    cache_control: &'static str,
    location: Option<String>,
    body: Vec<u8>,
}

pub async fn serve(state: WebState, www_dir: PathBuf, bind: String, port: u16) -> Result<()> {
    let listener = TcpListener::bind((bind.as_str(), port))
        .await
        .with_context(|| format!("监听 {bind}:{port} 失败"))?;
    log::info!("Web 设置页: http://{bind}:{port}/");
    log::info!("静态目录: {}", www_dir.display());
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = state.clone();
        let www_dir = www_dir.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, state, www_dir).await {
                log::warn!("Web 请求失败（{peer}）: {e}");
            }
        });
    }
}

#[cfg(unix)]
pub async fn serve_unix(
    state: WebState,
    www_dir: PathBuf,
    socket_path: PathBuf,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let _ = fs::remove_file(&socket_path);
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 socket 目录失败: {}", parent.display()))?;
    }
    let listener = tokio::net::UnixListener::bind(&socket_path)
        .with_context(|| format!("监听 Unix socket 失败: {}", socket_path.display()))?;
    let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666));
    log::info!("Web 设置页 Unix socket: {}", socket_path.display());
    log::info!("静态目录: {}", www_dir.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let www_dir = www_dir.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, state, www_dir).await {
                log::warn!("Web gateway 请求失败: {e}");
            }
        });
    }
}

async fn handle_conn<S>(mut stream: S, state: WebState, www_dir: PathBuf) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = match read_request(&mut stream).await {
        Ok(request) => request,
        Err(e) => {
            let response = HttpResponse::text(400, &e);
            write_response(&mut stream, response).await?;
            return Ok(());
        }
    };
    let response = handle_request(request, &state, &www_dir).await;
    write_response(&mut stream, response).await?;
    Ok(())
}

async fn handle_request(request: Request, state: &WebState, www_dir: &Path) -> HttpResponse {
    let mut path = request.path.clone();
    if path == GATEWAY_PREFIX {
        return HttpResponse::redirect(format!("{GATEWAY_PREFIX}/"));
    }
    if let Some(rest) = path.strip_prefix(GATEWAY_PREFIX) {
        path = if rest.is_empty() {
            "/".to_string()
        } else if rest.starts_with('/') {
            rest.to_string()
        } else {
            format!("/{rest}")
        };
    }
    match (request.method.as_str(), path.as_str()) {
        ("GET", "/api/settings") => api_settings(state),
        ("POST", "/api/settings") => api_save_settings(state, &request.body),
        ("GET", "/api/status") => api_status(state).await,
        ("POST", "/api/login") => api_login(state).await,
        ("POST", "/api/logout") => api_logout(state).await,
        ("POST", "/api/sms/clear") => api_clear_sms(state),
        ("POST", "/api/clear-all") => api_clear_all(state),
        ("GET", "/api/app-info") => api_app_info(),
        ("GET", "/api/background") => api_background(state),
        ("POST", "/api/background") => api_background_upload(state, &request.body),
        ("POST", "/api/background/clear") => api_background_clear(state),
        ("GET", _) => static_response(www_dir, &path),
        ("HEAD", _) => static_response(www_dir, &path),
        _ if path.starts_with("/api/") => {
            HttpResponse::json(405, &serde_json::json!({"error": "method not allowed"}))
        }
        _ => HttpResponse::text(405, "method not allowed"),
    }
}

fn api_settings(state: &WebState) -> HttpResponse {
    match snapshot(state) {
        Ok(cfg) => HttpResponse::json(200, &build_view(state, &cfg)),
        Err(e) => HttpResponse::json(500, &serde_json::json!({"error": e})),
    }
}

fn api_save_settings(state: &WebState, body: &[u8]) -> HttpResponse {
    let req: SaveSettingsRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(e) => {
            return HttpResponse::json(
                400,
                &serde_json::json!({"error": format!("请求 JSON 无效: {e}")}),
            )
        }
    };
    let mut cfg = match snapshot(state) {
        Ok(cfg) => cfg,
        Err(e) => return HttpResponse::json(500, &serde_json::json!({"error": e})),
    };

    if let Some(account) = req.account {
        let username = account.username.trim();
        if username.is_empty() {
            return HttpResponse::json(400, &serde_json::json!({"error": "账号不能为空"}));
        }
        if account.password.is_empty() && cfg.password.is_empty() {
            return HttpResponse::json(400, &serde_json::json!({"error": "密码不能为空"}));
        }
        cfg.username = username.to_string();
        if !account.password.is_empty() {
            cfg.password = account.password;
        }
        cfg.isp = if account.isp.trim().is_empty() {
            "2".into()
        } else {
            account.isp.trim().to_string()
        };
    }

    if let Some(sms) = req.sms {
        let code = sms.code.trim();
        if !code.is_empty() {
            if !(sms.hours > 0.0 || sms.hours == -1.0) {
                return HttpResponse::json(
                    400,
                    &serde_json::json!({"error": "有效期需大于 0 小时，或选择永久"}),
                );
            }
            cfg.set_sms(code, sms.hours, now_ms());
        }
    }

    if let Some(url) = req.portal_wireless {
        let url = url.trim();
        if Config::validate_portal_url(url).is_err() {
            return HttpResponse::json(400, &serde_json::json!({"error": "无线认证网址无效"}));
        }
        cfg.portal_wireless = url.to_string();
    }
    if let Some(url) = req.portal_wired {
        let url = url.trim();
        if Config::validate_portal_url(url).is_err() {
            return HttpResponse::json(400, &serde_json::json!({"error": "有线认证网址无效"}));
        }
        cfg.portal_wired = url.to_string();
    }
    if let Some(away) = req.away_mode {
        cfg.away_mode = away;
    }
    if let Some(auto) = req.auto_relogin {
        cfg.auto_relogin = auto;
    }
    if let Some(theme) = req.theme {
        cfg.theme = normalize_theme(&theme);
    }
    cfg.normalize();

    if let Err(e) = persist(state, &cfg) {
        return HttpResponse::json(500, &serde_json::json!({"error": e}));
    }
    HttpResponse::json(200, &build_view(state, &cfg))
}

async fn api_status(state: &WebState) -> HttpResponse {
    let cfg = match snapshot(state) {
        Ok(cfg) => cfg,
        Err(e) => return HttpResponse::json(500, &serde_json::json!({"error": e})),
    };
    if cfg.username.is_empty() {
        return HttpResponse::json(
            200,
            &StatusView {
                kind: "unconfigured".into(),
                text: "未配置账号".into(),
            },
        );
    }
    let probe_http = match probe::client() {
        Ok(client) => client,
        Err(e) => {
            return HttpResponse::json(
                200,
                &StatusView {
                    kind: "failed".into(),
                    text: format!("网络探测失败: {e}"),
                },
            )
        }
    };
    let choice = probe::choose_portal(&cfg, &probe_http, state.forced_portal.as_deref()).await;
    if !choice.reachable {
        return HttpResponse::json(
            200,
            &StatusView {
                kind: "network-down".into(),
                text: "认证页不可达（有线 .3 / 无线 .2）".into(),
            },
        );
    }
    let client = match DrcomClient::new(&cfg, &choice.url) {
        Ok(client) => client,
        Err(e) => {
            return HttpResponse::json(
                200,
                &StatusView {
                    kind: "failed".into(),
                    text: e.to_string(),
                },
            )
        }
    };
    match client.chkstatus().await {
        Ok(reply) if reply.ok() => HttpResponse::json(
            200,
            &StatusView {
                kind: "connected".into(),
                text: "已连接".into(),
            },
        ),
        Ok(reply) => HttpResponse::json(
            200,
            &StatusView {
                kind: "network-down".into(),
                text: format!("门户离线（result={}）", reply.result_text()),
            },
        ),
        Err(e) => HttpResponse::json(
            200,
            &StatusView {
                kind: "failed".into(),
                text: format!("状态查询失败: {e}"),
            },
        ),
    }
}

async fn api_login(state: &WebState) -> HttpResponse {
    let cfg = match snapshot(state) {
        Ok(cfg) => cfg,
        Err(e) => return HttpResponse::json(500, &serde_json::json!({"error": e})),
    };
    if let Err(e) = cfg.validate_login() {
        return HttpResponse::json(200, &command(false, e.to_string()));
    }
    let probe_http = match probe::client() {
        Ok(client) => client,
        Err(e) => return HttpResponse::json(200, &command(false, e.to_string())),
    };
    let choice = probe::choose_portal(&cfg, &probe_http, state.forced_portal.as_deref()).await;
    if !choice.reachable {
        return HttpResponse::json(200, &command(false, "认证页不可达".into()));
    }
    let client = match DrcomClient::new(&cfg, &choice.url) {
        Ok(client) => client,
        Err(e) => return HttpResponse::json(200, &command(false, e.to_string())),
    };
    if let Err(e) = client.warmup().await {
        log::warn!("打开认证页失败（继续登录）: {e}");
    }
    match client.login_flow().await {
        Ok(LoginOutcome::AlreadyOnline) => {
            HttpResponse::json(200, &command(true, "已在线，无需重复登录".into()))
        }
        Ok(LoginOutcome::LoggedIn) => HttpResponse::json(200, &command(true, "登录成功".into())),
        Err(e) => HttpResponse::json(200, &command(false, e.to_string())),
    }
}

async fn api_logout(state: &WebState) -> HttpResponse {
    let cfg = match snapshot(state) {
        Ok(cfg) => cfg,
        Err(e) => return HttpResponse::json(500, &serde_json::json!({"error": e})),
    };
    let probe_http = match probe::client() {
        Ok(client) => client,
        Err(e) => return HttpResponse::json(200, &command(false, e.to_string())),
    };
    let choice = probe::choose_portal(&cfg, &probe_http, state.forced_portal.as_deref()).await;
    if !choice.reachable {
        return HttpResponse::json(200, &command(false, "认证页不可达".into()));
    }
    let client = match DrcomClient::new(&cfg, &choice.url) {
        Ok(client) => client,
        Err(e) => return HttpResponse::json(200, &command(false, e.to_string())),
    };
    match client.logout().await {
        Ok(reply) if reply.ok() => HttpResponse::json(200, &command(true, "注销成功".into())),
        Ok(reply) => HttpResponse::json(
            200,
            &command(false, format!("注销失败: {}", reply.msg_text())),
        ),
        Err(e) => HttpResponse::json(200, &command(false, e.to_string())),
    }
}

fn api_clear_sms(state: &WebState) -> HttpResponse {
    let mut cfg = match snapshot(state) {
        Ok(cfg) => cfg,
        Err(e) => return HttpResponse::json(500, &serde_json::json!({"error": e})),
    };
    cfg.clear_sms();
    if let Err(e) = persist(state, &cfg) {
        return HttpResponse::json(500, &serde_json::json!({"error": e}));
    }
    HttpResponse::json(200, &build_view(state, &cfg))
}

fn api_clear_all(state: &WebState) -> HttpResponse {
    let mut cfg = match snapshot(state) {
        Ok(cfg) => cfg,
        Err(e) => return HttpResponse::json(500, &serde_json::json!({"error": e})),
    };
    cfg.username.clear();
    cfg.password.clear();
    cfg.clear_sms();
    if let Err(e) = persist(state, &cfg) {
        return HttpResponse::json(500, &serde_json::json!({"error": e}));
    }
    HttpResponse::json(200, &build_view(state, &cfg))
}

fn api_app_info() -> HttpResponse {
    HttpResponse::json(
        200,
        &AppInfoView {
            version: env!("CARGO_PKG_VERSION").into(),
            repo_url: "https://github.com/YgtqRun/YuLinkc".into(),
        },
    )
}

fn background_path(state: &WebState) -> PathBuf {
    state
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("background.img")
}

fn background_url(state: &WebState) -> Option<String> {
    let meta = fs::metadata(background_path(state)).ok()?;
    let version = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Some(format!("api/background?v={version}"))
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("image/png")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

fn api_background(state: &WebState) -> HttpResponse {
    let body = match fs::read(background_path(state)) {
        Ok(body) => body,
        Err(_) => return HttpResponse::text(404, "background not set"),
    };
    let content_type = image_mime(&body).unwrap_or("application/octet-stream");
    HttpResponse {
        status: 200,
        content_type,
        cache_control: "no-cache",
        location: None,
        body,
    }
}

fn api_background_upload(state: &WebState, body: &[u8]) -> HttpResponse {
    if body.is_empty() {
        return HttpResponse::json(400, &serde_json::json!({"error": "背景图片为空"}));
    }
    if body.len() > 5 * 1024 * 1024 {
        return HttpResponse::json(400, &serde_json::json!({"error": "背景图片不能超过 5MB"}));
    }
    if image_mime(body).is_none() {
        return HttpResponse::json(
            400,
            &serde_json::json!({"error": "只支持 WebP / JPEG / PNG / GIF"}),
        );
    }
    let path = background_path(state);
    let Some(parent) = path.parent() else {
        return HttpResponse::json(500, &serde_json::json!({"error": "背景目录无效"}));
    };
    if let Err(e) = fs::create_dir_all(parent) {
        return HttpResponse::json(500, &serde_json::json!({"error": format!("创建目录失败: {e}")}));
    }
    let tmp = path.with_extension("tmp");
    if let Err(e) = fs::write(&tmp, body) {
        return HttpResponse::json(500, &serde_json::json!({"error": format!("写入失败: {e}")}));
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return HttpResponse::json(500, &serde_json::json!({"error": format!("保存失败: {e}")}));
    }
    log::info!("Web 背景图片已更新（{} bytes）", body.len());
    HttpResponse::json(
        200,
        &serde_json::json!({
            "ok": true,
            "url": background_url(state).unwrap_or_default()
        }),
    )
}

fn api_background_clear(state: &WebState) -> HttpResponse {
    let path = background_path(state);
    match fs::remove_file(&path) {
        Ok(()) => log::info!("Web 背景图片已清除"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return HttpResponse::json(500, &serde_json::json!({"error": format!("清除失败: {e}")}))
        }
    }
    HttpResponse::json(200, &serde_json::json!({"ok": true}))
}

fn build_view(state: &WebState, cfg: &Config) -> SettingsView {
    let now = now_ms();
    let account_configured = !cfg.username.is_empty();
    let kind = if !account_configured {
        "unconfigured"
    } else if !cfg.sms_valid_at(now) {
        "needs-sms"
    } else {
        "ready"
    };
    let text = match kind {
        "unconfigured" => "未配置账号",
        "needs-sms" => "动态密码缺失或已过期",
        _ => "已就绪，可自动登录",
    };
    SettingsView {
        account_configured,
        username: cfg.username.clone(),
        password: cfg.password.clone(),
        isp: cfg.isp.clone(),
        has_sms_code: !cfg.sms_code.is_empty(),
        sms_code: cfg.sms_code.clone(),
        sms_hours: cfg.sms_hours,
        sms_saved_at: cfg.sms_saved_at,
        sms_status_text: cfg.sms_remaining_text(now),
        autostart: false,
        startup_boost: false,
        away_mode: cfg.away_mode,
        show_auth_window: false,
        auto_relogin: cfg.auto_relogin,
        quit_after_first_connect: false,
        theme: cfg.theme.clone(),
        login_mode: "http".into(),
        background_url: background_url(state),
        portal_wireless: cfg.portal_wireless.clone(),
        portal_wired: cfg.portal_wired.clone(),
        status: StatusView {
            kind: kind.into(),
            text: text.into(),
        },
    }
}

fn command(ok: bool, message: String) -> CommandResult {
    CommandResult { ok, message }
}

fn snapshot(state: &WebState) -> Result<Config, String> {
    state
        .config
        .lock()
        .map(|cfg| cfg.clone())
        .map_err(|_| "配置锁不可用".into())
}

fn persist(state: &WebState, cfg: &Config) -> Result<(), String> {
    cfg.save(&state.config_path).map_err(|e| e.to_string())?;
    *state
        .config
        .lock()
        .map_err(|_| "配置锁不可用".to_string())? = cfg.clone();
    Ok(())
}

fn static_response(www_dir: &Path, request_path: &str) -> HttpResponse {
    let rel = request_path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.split('/').any(|part| part == ".." || part.is_empty()) {
        return HttpResponse::text(404, "not found");
    }
    let file = www_dir.join(rel);
    let body = match std::fs::read(&file) {
        Ok(body) => body,
        Err(_) => return HttpResponse::text(404, "not found"),
    };
    let cache = if rel.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    HttpResponse {
        status: 200,
        content_type: mime_for(rel),
        cache_control: cache,
        location: None,
        body,
    }
}

fn mime_for(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

async fn read_request<S>(stream: &mut S) -> Result<Request, String>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("读取请求失败: {e}"))?;
        if n == 0 {
            return Err("连接在请求完成前关闭".into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_bytes(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 32 * 1024 {
            return Err("请求头过大".into());
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.lines();
    let first = lines.next().ok_or_else(|| "缺少请求行".to_string())?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or_else(|| "缺少请求方法".to_string())?;
    let target = parts.next().ok_or_else(|| "缺少请求路径".to_string())?;
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > 5 * 1024 * 1024 {
        return Err("请求体过大".into());
    }

    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("读取请求体失败: {e}"))?;
        if n == 0 {
            return Err("请求体不完整".into());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Ok(Request {
        method: method.to_string(),
        path: target.split('?').next().unwrap_or("/").to_string(),
        body,
    })
}

async fn write_response<S>(stream: &mut S, response: HttpResponse) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        308 => "Permanent Redirect",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\n",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        response.cache_control
    );
    if let Some(location) = &response.location {
        head.push_str(&format!("Location: {location}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await?;
    Ok(())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

impl HttpResponse {
    fn json<T: Serialize>(status: u16, value: &T) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            cache_control: "no-store",
            location: None,
            body: serde_json::to_vec(value).unwrap_or_else(|_| b"{\"error\":\"json\"}".to_vec()),
        }
    }

    fn text(status: u16, text: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            cache_control: "no-store",
            location: None,
            body: text.as_bytes().to_vec(),
        }
    }

    fn redirect(location: String) -> Self {
        Self {
            status: 308,
            content_type: "text/plain; charset=utf-8",
            cache_control: "no-store",
            location: Some(location),
            body: Vec::new(),
        }
    }
}

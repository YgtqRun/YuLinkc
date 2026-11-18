//! 登录调度器：轻量网络心跳 + 完整巡检（探活、断线自动重登、即时登录触发）。
//!
//! 与旧版“每 N 秒做一次完整巡检”的区别：
//! - 高频“闸门”只做廉价外网探活（默认 8s），网络 up/down 翻转时立刻把完整巡检提前；
//! - 外网探活采用“主地址 + 备用 204 地址”，任一通过即在线，降低单一地址被校园网
//!   劫持/阻断造成的“明明联网却判离线”；
//! - 连续多次闸门失败才认定离线（迟滞），打开设置等“状态查询”不会因单次误判触发登录；
//! - 登录失败按 `loginMaxRetries` 指数退避；若认证页已受理但外网始终未恢复，
//!   则暂停自动重登，等外网真正恢复（或用户手动登录）再继续，避免反复打门户；
//! - 页面判定成功只作为候选：Rust 外网探测确认通过才算“已连接”。
//! - 用户点“立即登录”永远优先：排队请求合并成一次强制登录，正在跑的“仅检测”
//!   巡检也会立刻让位，避免手动登录被“断网自动重试已关闭”的检测轮挤掉/拖住。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::auth_session::{run_login, LoginOutcomeKind};
use crate::auth_window::AuthWindowVisibility;
use crate::http_login;
use crate::network;
use crate::state::RuntimeState;
use crate::store::ConfigState;

const EXTERNAL_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const GATE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PORTAL_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 轻量闸门探活间隔：已连接时也能在约 2 个周期内感知掉线/恢复。
const GATE_INTERVAL_SEC: u64 = 8;
/// 连续多少次闸门失败才认定离线（迟滞阈值）。
const OFFLINE_GATE_FAILS: u32 = 2;
/// 未达离线阈值时完整巡检的休眠时长；闸门在网络翻转时会提前唤醒。
const GATE_HOLD_SEC: u64 = 300;
/// 主探活地址之外的备用地址：返回 204 即视为在线（劫持页不会回 204）。
const FALLBACK_PROBE_URLS: [&str; 2] = [
    "http://connect.rom.miui.com/generate_204",
    "http://cp.cloudflare.com/generate_204",
];

pub enum SchedMessage {
    LoginNow,
    CheckNow,
}

impl SchedMessage {
    /// 是否为“用户显式要求登录”：必须优先于普通巡检执行。
    fn is_login(&self) -> bool {
        matches!(self, SchedMessage::LoginNow)
    }
}

/// 供托盘/命令持有的调度器句柄。
#[derive(Clone)]
pub struct Scheduler {
    tx: Sender<SchedMessage>,
    /// “立即登录”的即时标记：只用于让正在跑的“仅检测”巡检尽快让位，
    /// 真正的请求仍然以通道里的消息为准。
    login_requested: Arc<AtomicBool>,
}

impl Scheduler {
    pub fn request_login(&self) {
        self.login_requested.store(true, Ordering::SeqCst);
        let _ = self.tx.send(SchedMessage::LoginNow);
    }

    /// 唤醒一次完整巡检（配置/网址/模式变更或界面查询状态后立即生效）。
    /// 注意：巡检不会绕过“连续离线阈值”，单次探活误判不会触发登录。
    pub fn check_now(&self) {
        let _ = self.tx.send(SchedMessage::CheckNow);
    }
}

/// 调度器跨轮次状态。
#[derive(Default)]
struct SchedCtx {
    /// 闸门连续失败次数（完整巡检与闸门共享同一计数）。
    external_fail: u32,
    /// 连续登录失败次数（用于指数退避；外网恢复或登录成功后清零）。
    login_fail: u32,
    /// 本次进程启动后是否已达成首次连接（外网确认或认证页显示已登录）。
    /// “断网自动重试”关闭时，首次连接成功后即不再自动登录。
    first_connect_done: bool,
    /// 上一次登录“认证页已受理（ok/navigating）但外网未确认”：为 true 时
    /// 不再自动重登，直到外网探测恢复或用户手动触发。
    page_ok_pending: bool,
}

pub fn spawn(app: &AppHandle) -> Scheduler {
    let (tx, rx) = mpsc::channel::<SchedMessage>();
    let login_requested = Arc::new(AtomicBool::new(false));
    let login_flag = login_requested.clone();
    let app2 = app.clone();
    thread::spawn(move || {
        let mut ctx = SchedCtx::default();
        // 启动即做一次闸门探活 + 一次完整巡检，保持旧版的“启动立刻检查”行为。
        let mut next_gate = Instant::now();
        let mut next_full = Instant::now();
        let mut force_login = false;
        let mut last_gate_online: Option<bool> = None;

        loop {
            // 0) 先一次性消化队列里排队的请求。
            //    界面“立即登录”会先保存凭据（顺带发一次 CheckNow）再发 LoginNow；
            //    若逐个处理，就会先跑一轮普通巡检——关闭“断网自动重试”后普通巡检
            //    是“仅检测”，会把用户手动点的登录挤到后面甚至覆盖掉状态提示。
            //    这里合并成一次巡检，并且只要队列里有 LoginNow 就必须是强制登录。
            match drain_requests(&rx, &mut force_login) {
                Drain::Closed => break,
                Drain::Awake => next_full = Instant::now(),
                Drain::Idle => {}
            }

            // 1) 轻量闸门：多端点探外网，感知 up/down 翻转并唤醒完整巡检。
            if Instant::now() >= next_gate {
                next_gate += Duration::from_secs(GATE_INTERVAL_SEC);
                if let Some((online, reasons)) = gate_probe(&app2) {
                    let changed = last_gate_online.map(|o| o != online).unwrap_or(true);
                    last_gate_online = Some(online);
                    if online {
                        if ctx.external_fail >= OFFLINE_GATE_FAILS {
                            log::info!("外网心跳探活恢复在线");
                        }
                        ctx.external_fail = 0;
                        ctx.page_ok_pending = false;
                        if changed {
                            next_full = Instant::now();
                        }
                    } else {
                        ctx.external_fail = ctx.external_fail.saturating_add(1);
                        // 只在前几次失败时写日志，避免离线期间每 8 秒刷屏。
                        if ctx.external_fail <= OFFLINE_GATE_FAILS {
                            log::warn!(
                                "外网心跳探活失败（连续 {}/{} 次）：{}",
                                ctx.external_fail,
                                OFFLINE_GATE_FAILS,
                                reasons.join("；")
                            );
                        }
                        // 只在“刚达到阈值”这一次唤醒完整巡检；之后由完整巡检返回的
                        // 退避/巡检间隔决定节奏，否则会每 8 秒覆盖一次退避，造成反复登录。
                        if ctx.external_fail == OFFLINE_GATE_FAILS {
                            next_full = Instant::now();
                        }
                    }
                }
            }

            // 2) 完整巡检（仅在到期时执行）。
            if Instant::now() >= next_full {
                if force_login {
                    // 手动登录马上开始：清掉即时标记，之后的新一轮“仅检测”无需再让位。
                    login_flag.store(false, Ordering::SeqCst);
                }
                let wait_secs = full_check(&app2, force_login, &mut ctx, &login_flag);
                force_login = false;
                next_full = Instant::now() + Duration::from_secs(wait_secs);
            }

            // 3) 等待下一个闸门或完整巡检；消息到达则提前唤醒。
            let next_event = next_gate.min(next_full);
            let now = Instant::now();
            if now >= next_event {
                continue;
            }
            match rx.recv_timeout(next_event - now) {
                // 先记下请求意图，队列里剩余的请求交给下一轮开头的统一消化。
                Ok(msg) => {
                    if msg.is_login() {
                        force_login = true;
                    }
                    next_full = Instant::now();
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    });
    Scheduler {
        tx,
        login_requested,
    }
}

/// 一轮“消化队列请求”的结果。
enum Drain {
    /// 队列为空，本轮没有新请求。
    Idle,
    /// 取到了至少一个请求：应立刻做一次完整巡检。
    Awake,
    /// 发送端已关闭（程序退出）。
    Closed,
}

/// 取走队列里所有待处理请求；`LoginNow` 会把本轮标记为强制登录。
fn drain_requests(rx: &Receiver<SchedMessage>, force_login: &mut bool) -> Drain {
    let mut awake = false;
    loop {
        match rx.try_recv() {
            Ok(msg) => {
                awake = true;
                if msg.is_login() {
                    *force_login = true;
                }
            }
            Err(TryRecvError::Empty) => {
                return if awake { Drain::Awake } else { Drain::Idle };
            }
            Err(TryRecvError::Disconnected) => return Drain::Closed,
        }
    }
}

/// 闸门：多端点廉价外网探活。配置读取失败时返回 None（本轮跳过）。
fn gate_probe(app: &AppHandle) -> Option<(bool, Vec<String>)> {
    let state = app.state::<ConfigState>();
    let cfg = state.0.lock().ok()?;
    let url = cfg.preferences.external_probe_url.clone();
    Some(probe_external_any(&url, GATE_PROBE_TIMEOUT))
}

/// 外网多端点探活：主地址 + 备用 204 地址，任一通过即在线。
fn probe_external_any(primary: &str, per_url_timeout: Duration) -> (bool, Vec<String>) {
    let mut urls = vec![primary.to_string()];
    urls.extend(FALLBACK_PROBE_URLS.iter().map(|s| s.to_string()));
    let mut reasons = Vec::with_capacity(urls.len());
    for url in urls {
        let (ok, reason) = network::external_probe(&url, per_url_timeout);
        if ok {
            reasons.push(format!("{url}: ok（{reason}）"));
            return (true, reasons);
        }
        reasons.push(format!("{url}: {reason}"));
    }
    (false, reasons)
}

/// HTTP 新流程的精简巡检：
/// 外网探活 → 选认证页 → checkPassStatus → login → chkstatus。
///
/// 不再使用 WebView2 的页面受理候选、page_ok_pending 暂停、external_restored
/// 多轮确认和仅检测让位逻辑；HTTP 接口返回成功且 chkstatus 在线即视为成功。
fn http_full_check(
    app: &AppHandle,
    force_login: bool,
    ctx: &mut SchedCtx,
    cfg: &crate::store::AppConfig,
) -> u64 {
    let state = app.state::<RuntimeState>();
    let prefs = &cfg.preferences;
    ctx.page_ok_pending = false;

    if prefs.away_mode {
        state.set(app, "away-mode", "离校模式：自动认证已暂停".into());
        return 300;
    }
    if cfg.account.is_none() {
        state.set(app, "unconfigured", "未配置账号，点击托盘打开设置".into());
        return 60;
    }

    if !force_login {
        let (online, reasons) =
            probe_external_any(&prefs.external_probe_url, EXTERNAL_PROBE_TIMEOUT);
        if online {
            ctx.external_fail = 0;
            ctx.login_fail = 0;
            ctx.first_connect_done = true;
            state.set(app, "connected", "已连接".into());
            maybe_quit_after_connect(app, prefs.quit_after_first_connect);
            return prefs.check_interval_online_sec.max(10);
        }
        log::info!("HTTP 巡检外网探活失败：{}", reasons.join("；"));
        if ctx.external_fail < OFFLINE_GATE_FAILS && !mock_portal_mode() {
            log::info!(
                "外网失败未达连续阈值（{}/{}），暂不自动登录",
                ctx.external_fail,
                OFFLINE_GATE_FAILS
            );
            return GATE_HOLD_SEC;
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if !cfg.sms_valid_at(now) {
        state.set(app, "needs-sms", "动态密码缺失或已过期".into());
        return 60;
    }

    let medium = network::detect_medium().unwrap_or(network::Medium::Other);
    let candidates = portal_candidates(medium, prefs);
    let portal = if force_login {
        log::info!(
            "手动触发 HTTP 登录（介质: {}, 候选认证页: {}）",
            medium_display(medium),
            candidates.join(", ")
        );
        candidates.first().cloned()
    } else {
        candidates
            .iter()
            .find(|url| network::http_ok(url, PORTAL_PROBE_TIMEOUT))
            .cloned()
    };
    let Some(portal) = portal else {
        state.set(
            app,
            "network-down",
            format!(
                "未检测到校园网认证页（已尝试 {}，请确认已连接校园网无线/有线）",
                candidates.join(" / ")
            ),
        );
        return prefs.check_interval_offline_sec.max(10);
    };

    let Some((account, password)) = cfg.account_password() else {
        state.set(app, "unconfigured", "账号配置读取失败".into());
        return 60;
    };
    let Some(sms_code) = cfg.sms_code() else {
        state.set(app, "needs-sms", "动态密码缺失或已过期".into());
        return 60;
    };
    let isp = cfg
        .account
        .as_ref()
        .map(|a| a.isp.clone())
        .unwrap_or_else(|| "2".into());
    let detect_only = !force_login && !prefs.auto_relogin && ctx.first_connect_done;

    state.set(
        app,
        "logging-in",
        if detect_only {
            "正在检测认证页状态（HTTP）".into()
        } else {
            "正在自动登录（HTTP）".into()
        },
    );

    match http_login::run_http_login(&portal, &account, &password, &sms_code, &isp, detect_only) {
        Ok(o) if o.kind == LoginOutcomeKind::AlreadyOnline => {
            log::info!("HTTP 状态查询：已在线");
            ctx.login_fail = 0;
            if http_external_confirmed(app, &prefs.external_probe_url) {
                ctx.external_fail = 0;
                ctx.first_connect_done = true;
                state.set(app, "connected", "已连接".into());
                maybe_quit_after_connect(app, prefs.quit_after_first_connect);
                prefs.check_interval_online_sec.max(10)
            } else {
                state.set(app, "failed", "认证页显示已在线，但外网确认未通过".into());
                prefs.check_interval_offline_sec.max(10)
            }
        }
        Ok(o) if o.is_ok() => {
            log::info!("HTTP 登录成功");
            ctx.login_fail = 0;
            if http_external_confirmed(app, &prefs.external_probe_url) {
                ctx.external_fail = 0;
                ctx.first_connect_done = true;
                state.set(app, "connected", "已连接".into());
                maybe_quit_after_connect(app, prefs.quit_after_first_connect);
                prefs.check_interval_online_sec.max(10)
            } else {
                state.set(app, "failed", "HTTP 登录成功，但外网确认未通过".into());
                prefs.check_interval_offline_sec.max(10)
            }
        }
        Ok(o) if o.kind == LoginOutcomeKind::LoginFormPresent => {
            log::info!("{}（自动登录已关闭，仅检测）", o.message());
            state.set(
                app,
                "network-down",
                "网络已断开（自动登录已关闭，仅保留检测）".into(),
            );
            prefs.check_interval_offline_sec.max(10)
        }
        Ok(o) => {
            ctx.login_fail = ctx.login_fail.saturating_add(1);
            state.set(
                app,
                "failed",
                format!(
                    "{}（第 {} 次失败，按退避重试）",
                    o.message(),
                    ctx.login_fail
                ),
            );
            backoff_secs(ctx.login_fail, prefs.login_max_retries)
        }
        Err(e) => {
            ctx.login_fail = ctx.login_fail.saturating_add(1);
            state.set(
                app,
                "failed",
                format!(
                    "HTTP 登录异常：{e}（第 {} 次失败，按退避重试）",
                    ctx.login_fail
                ),
            );
            backoff_secs(ctx.login_fail, prefs.login_max_retries)
        }
    }
}

/// HTTP 流程的一次性外网确认（主地址 + 备用 204，任一通过即可）。
fn http_external_confirmed(app: &AppHandle, probe_url: &str) -> bool {
    let state = app.state::<RuntimeState>();
    state.set(app, "logging-in", "正在确认外网（HTTP）".into());
    let (online, reasons) = probe_external_any(probe_url, EXTERNAL_PROBE_TIMEOUT);
    if online {
        log::info!("HTTP 外网确认通过：{}", reasons.join("；"));
    } else {
        log::warn!("HTTP 外网确认失败：{}", reasons.join("；"));
    }
    online
}

/// 执行一轮完整巡检，返回下一次完整巡检的等待秒数。
fn full_check(
    app: &AppHandle,
    force_login: bool,
    ctx: &mut SchedCtx,
    login_requested: &AtomicBool,
) -> u64 {
    let cfg = match app.state::<ConfigState>().0.lock() {
        Ok(g) => g.clone(),
        Err(_) => return 30,
    };
    // HTTP 新流程不套 WebView2 的页面受理/多轮外网确认/让位逻辑，
    // 单独走精简路径；旧流程保持不变。
    if cfg.preferences.login_mode == "http" {
        return http_full_check(app, force_login, ctx, &cfg);
    }
    let state = app.state::<RuntimeState>();
    let prefs = &cfg.preferences;

    // 离校模式：暂停一切自动认证（含手动"立即登录"）。
    if prefs.away_mode {
        state.set(app, "away-mode", "离校模式：自动认证已暂停".into());
        return 300;
    }

    if cfg.account.is_none() {
        state.set(app, "unconfigured", "未配置账号，点击托盘打开设置".into());
        return 60;
    }

    // 最终外网闸门：手动“立即登录”不经过这里。
    if !force_login {
        let (online, reasons) =
            probe_external_any(&prefs.external_probe_url, EXTERNAL_PROBE_TIMEOUT);
        if online {
            ctx.external_fail = 0;
            ctx.login_fail = 0;
            ctx.page_ok_pending = false;
            ctx.first_connect_done = true;
            state.set(app, "connected", "已连接".into());
            maybe_quit_after_connect(app, prefs.quit_after_first_connect);
            return prefs.check_interval_online_sec.max(10);
        }
        log::info!("完整巡检外网探活失败：{}", reasons.join("；"));
        if ctx.external_fail < OFFLINE_GATE_FAILS && !mock_portal_mode() {
            // 尚未连续失败到阈值：不改状态、不登录，交给闸门在翻转时唤醒。
            log::info!(
                "外网失败未达连续阈值（{}/{}），暂不自动登录",
                ctx.external_fail,
                OFFLINE_GATE_FAILS
            );
            return GATE_HOLD_SEC;
        }
    }

    // 认证页上次已受理但外网未确认：不再自动重登（重打门户没有意义），
    // 等外网探测恢复（闸门会唤醒）或用户手动点“立即登录”。
    if !force_login && ctx.page_ok_pending {
        state.set(
            app,
            "failed",
            "上次登录认证页已受理但外网未确认；已暂停自动重登，等待网络恢复后继续".into(),
        );
        return GATE_HOLD_SEC;
    }

    // 选择认证页：先按介质排序，再按可达性挑第一个能响应的。
    let medium = network::detect_medium().unwrap_or(network::Medium::Other);
    let candidates = portal_candidates(medium, prefs);
    if force_login {
        log::info!(
            "手动触发登录（介质: {}, 候选认证页: {}）",
            medium_display(medium),
            candidates.join(", ")
        );
    }
    // 手动“立即登录”不应被探活挡住：即使探活失败也直接加载介质对应的认证页，
    // 让 WebView 自己尝试（认证窗口加载失败会由登录会话超时/刷新逻辑兜底）。
    let portal = if force_login {
        candidates.clone().into_iter().next()
    } else {
        candidates
            .iter()
            .find(|url| network::http_ok(url, PORTAL_PROBE_TIMEOUT))
            .cloned()
    };
    let Some(portal) = portal else {
        state.set(
            app,
            "network-down",
            format!(
                "未检测到校园网认证页（已尝试 {}，请确认已连接校园网无线/有线）",
                candidates.join(" / ")
            ),
        );
        return prefs.check_interval_offline_sec.max(10);
    };

    // 认证页“是否需要登录”由 WebView 内的注入脚本判定（无需 Rust 猜 HTML）：
    // - 出现登录表单 → 走填表登录；
    // - 无表单但出现“注销”按钮 → 已登录，上报 online，不重复登录；
    // - 两者都没有 → 超时失败，按退避重试。

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if !cfg.sms_valid_at(now) {
        state.set(app, "needs-sms", "动态密码缺失或已过期".into());
        return 60;
    }

    let Some((account, password)) = cfg.account_password() else {
        state.set(app, "unconfigured", "账号配置读取失败".into());
        return 60;
    };
    let Some(sms_code) = cfg.sms_code() else {
        state.set(app, "needs-sms", "动态密码缺失或已过期".into());
        return 60;
    };

    // “断网自动重试”关闭且本次启动已完成首次连接：仍然访问认证页做状态检测
    // （注销按钮=在线 / 登录表单=需要登录），但绝不执行填表登录。
    let detect_only = !force_login && !prefs.auto_relogin && ctx.first_connect_done;

    state.set(
        app,
        "logging-in",
        if detect_only {
            "正在检测认证页状态".into()
        } else {
            "正在自动登录".into()
        },
    );
    log::info!("登录方式：WebView2 旧流程");
    let outcome = run_login(
        app,
        &portal,
        &account,
        &password,
        &sms_code,
        &cfg.selectors,
        AuthWindowVisibility::from_config(prefs.show_auth_window),
        detect_only,
        // 只有“仅检测”巡检需要让位给用户手动登录；真正的登录不打断。
        if detect_only {
            Some(login_requested)
        } else {
            None
        },
    );

    match outcome {
        // 用户点“立即登录”打断了这轮“仅检测”：不写状态、不写退避，
        // 立刻回到队列，把同一个登录请求当作强制登录跑掉。
        Ok(o) if o.kind == LoginOutcomeKind::Preempted => {
            login_requested.store(false, Ordering::SeqCst);
            ctx.page_ok_pending = false;
            log::info!("仅检测巡检让位给手动登录，立即执行强制登录");
            return 0;
        }
        // 认证页没有登录表单但检测到注销按钮：会话已在线，不重复登录。
        Ok(o) if o.kind == LoginOutcomeKind::AlreadyOnline => {
            log::info!("{}，不重复登录", o.message());
            ctx.page_ok_pending = false;
            ctx.login_fail = 0;
            ctx.first_connect_done = true;
            // 已确认在线后再测一次外网：只影响状态文案，不阻止“已登录”结论。
            let confirmed = mock_portal_mode() || external_restored(app, &prefs.external_probe_url);
            if confirmed {
                ctx.external_fail = 0;
                state.set(app, "connected", "已连接".into());
                maybe_quit_after_connect(app, prefs.quit_after_first_connect);
                return prefs.check_interval_online_sec.max(10);
            }
            state.set(app, "connected", "已连接（认证页显示已登录）".into());
            // 已在线时不再频繁访问认证页；会话掉线时认证页会重新出现登录表单，
            // 下一轮完整巡检即可自动登录（网络事件/手动登录可提前唤醒）。
            maybe_quit_after_connect(app, prefs.quit_after_first_connect);
            return prefs.check_interval_online_sec.max(10);
        }
        // 检测模式（断网自动重试已关闭）下发现登录表单：提示断开，但不执行登录。
        Ok(o) if o.kind == LoginOutcomeKind::LoginFormPresent => {
            log::info!("{}（自动登录已关闭，仅检测）", o.message());
            state.set(
                app,
                "network-down",
                "网络已断开（自动登录已关闭，仅保留检测）".into(),
            );
            return prefs.check_interval_offline_sec.max(10);
        }
        Ok(o) if o.is_ok() => {
            log::info!("认证页确认登录成功（{}）", o.message());
            // 最终判定归 Rust：外网探测通过才是“已连接”。
            let confirmed = mock_portal_mode() || external_restored(app, &prefs.external_probe_url);
            if confirmed {
                ctx.external_fail = 0;
                ctx.login_fail = 0;
                ctx.page_ok_pending = false;
                ctx.first_connect_done = true;
                state.set(app, "connected", "已连接".into());
                maybe_quit_after_connect(app, prefs.quit_after_first_connect);
                return prefs.check_interval_online_sec.max(10);
            }
            // 门户说已受理但外网没通：暂停自动重登，等外网恢复再继续，
            // 避免“已在线”状态下仍反复打认证页。
            ctx.page_ok_pending = true;
            ctx.login_fail = ctx.login_fail.saturating_add(1);
            state.set(
                app,
                "failed",
                "认证页已判定成功，但外网探测未通过；暂停自动重登，等待网络恢复后继续".into(),
            );
            return GATE_HOLD_SEC;
        }
        Ok(o) => {
            if detect_only {
                state.set(
                    app,
                    "network-down",
                    format!("网络未就绪（检测模式）：{}", o.message()),
                );
                return prefs.check_interval_offline_sec.max(10);
            }
            // 失败不清除动态密码，有效期内可重试；按退避间隔重试。
            ctx.page_ok_pending = false;
            ctx.login_fail = ctx.login_fail.saturating_add(1);
            state.set(
                app,
                "failed",
                format!(
                    "{}（动态密码保留，第 {} 次失败，按退避重试）",
                    o.message(),
                    ctx.login_fail
                ),
            );
        }
        Err(e) => {
            if detect_only {
                state.set(app, "network-down", format!("网络未就绪（检测模式）：{e}"));
                return prefs.check_interval_offline_sec.max(10);
            }
            ctx.page_ok_pending = false;
            ctx.login_fail = ctx.login_fail.saturating_add(1);
            state.set(
                app,
                "failed",
                format!(
                    "登录会话异常：{e}（第 {} 次失败，按退避重试）",
                    ctx.login_fail
                ),
            );
        }
    }
    backoff_secs(ctx.login_fail, cfg.preferences.login_max_retries)
}

/// 登录成功后重试外网探测（最多 3 次，多端点任一通过即可）。
fn external_restored(app: &AppHandle, probe_url: &str) -> bool {
    let state = app.state::<RuntimeState>();
    let per_url_timeout = Duration::from_secs(2);
    for i in 1..=3 {
        state.set(app, "logging-in", format!("正在确认外网恢复（{i}/3）"));
        if probe_external_any(probe_url, per_url_timeout).0 {
            return true;
        }
        thread::sleep(Duration::from_secs(2));
    }
    false
}

/// “开机后连接成功就关闭软件”：首次连接成功后延时退出，让状态/托盘先落一次地。
fn maybe_quit_after_connect(app: &AppHandle, enabled: bool) {
    if !enabled {
        return;
    }
    log::info!("已连接，按设置“连接成功后退出”关闭 YuLink");
    let app2 = app.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(600));
        app2.exit(0);
    });
}

fn mock_portal_mode() -> bool {
    std::env::var("YULINK_MOCK_PORTAL")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// 登录失败退避：30s、60s、120s……指数增长，指数受 loginMaxRetries 封顶（默认最大 240s）。
fn backoff_secs(login_fail: u32, max_retries: u32) -> u64 {
    let steps = max_retries.max(1);
    let exp = (login_fail.saturating_sub(1)).min(steps);
    30u64 * 2u64.pow(exp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_requests_empty_is_idle() {
        let (_tx, rx) = mpsc::channel::<SchedMessage>();
        let mut force_login = false;
        assert!(matches!(drain_requests(&rx, &mut force_login), Drain::Idle));
        assert!(!force_login);
    }

    /// 关键回归：界面“立即登录”会先发 CheckNow（保存凭据）再发 LoginNow，
    /// 两者必须合并成一次强制登录，否则关闭“断网自动重试”后会先跑一轮“仅检测”。
    #[test]
    fn drain_requests_merges_check_then_login_into_forced_login() {
        let (tx, rx) = mpsc::channel::<SchedMessage>();
        tx.send(SchedMessage::CheckNow).unwrap();
        tx.send(SchedMessage::LoginNow).unwrap();

        let mut force_login = false;
        assert!(matches!(
            drain_requests(&rx, &mut force_login),
            Drain::Awake
        ));
        assert!(force_login, "队列里有 LoginNow 时必须按强制登录执行");
        // 消息已被一次性取空，不会留下一轮多余的普通巡检
        let mut again = false;
        assert!(matches!(drain_requests(&rx, &mut again), Drain::Idle));
        assert!(!again);
    }

    #[test]
    fn drain_requests_check_only_keeps_normal_check() {
        let (tx, rx) = mpsc::channel::<SchedMessage>();
        tx.send(SchedMessage::CheckNow).unwrap();

        let mut force_login = false;
        assert!(matches!(
            drain_requests(&rx, &mut force_login),
            Drain::Awake
        ));
        assert!(!force_login);
    }

    #[test]
    fn drain_requests_reports_closed_channel() {
        let (tx, rx) = mpsc::channel::<SchedMessage>();
        drop(tx);
        let mut force_login = false;
        assert!(matches!(
            drain_requests(&rx, &mut force_login),
            Drain::Closed
        ));
    }
}

fn portal_candidates(medium: network::Medium, prefs: &crate::store::Preferences) -> Vec<String> {
    let wireless = prefs.portal_wireless.clone();
    let wired = prefs.portal_wired.clone();
    match medium {
        network::Medium::Wireless => vec![wireless, wired],
        network::Medium::Wired => vec![wired, wireless],
        network::Medium::Other => vec![wireless, wired],
    }
}

fn medium_display(medium: network::Medium) -> &'static str {
    match medium {
        network::Medium::Wireless => "无线",
        network::Medium::Wired => "有线",
        network::Medium::Other => "其他",
    }
}

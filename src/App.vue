<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";

interface StatusView {
  kind: string;
  text: string;
}

interface SettingsView {
  accountConfigured: boolean;
  username: string;
  password: string;
  isp: string;
  hasSmsCode: boolean;
  smsCode: string;
  smsHours: number;
  smsSavedAt: number;
  smsStatusText: string;
  autostart: boolean;
  startupBoost: boolean;
  awayMode: boolean;
  showAuthWindow: boolean;
  autoRelogin: boolean;
  quitAfterFirstConnect: boolean;
  portalWireless: string;
  portalWired: string;
  status: StatusView;
}

interface AppInfo {
  version: string;
  repoUrl: string;
}

interface UpdateCheck {
  current: string;
  latest: string | null;
  hasUpdate: boolean;
  url: string | null;
  notes: string | null;
  error: string | null;
}

interface AccountInput {
  username: string;
  password: string;
  isp: string;
}

interface SmsInput {
  code: string;
  hours: number;
}

interface SaveRequest {
  account?: AccountInput;
  sms?: SmsInput;
  portalWireless?: string;
  portalWired?: string;
  awayMode?: boolean;
  startupBoost?: boolean;
  showAuthWindow?: boolean;
  autoRelogin?: boolean;
  quitAfterFirstConnect?: boolean;
}

interface CommandResult {
  ok: boolean;
  message: string;
}

const PRESET_HOURS = [1, 3, 9, 18, 27];
const DEFAULT_HOURS = 27;
const PERMANENT = -1;

const page = ref<"home" | "settings">("home");
const username = ref("");
const password = ref("");
const showPassword = ref(false);
const smsCode = ref("");
const smsStatusText = ref("未设置");
const savedSmsStatusText = ref("未设置");
const expireChoice = ref(String(DEFAULT_HOURS));
const customHours = ref("");
const autostart = ref(false);
const startupBoost = ref(false);
const awayMode = ref(false);
const showAuthWindow = ref(false);
const autoRelogin = ref(true);
const quitAfterFirstConnect = ref(false);
const portalWireless = ref("");
const portalWired = ref("");
const appInfo = ref<AppInfo>({ version: "", repoUrl: "" });
const updateResult = ref<UpdateCheck | null>(null);
const updateChecking = ref(false);
const statusView = ref<StatusView>({ kind: "checking", text: "读取中…" });
const busy = ref(false);
const toast = ref<{ text: string; type: string } | null>(null);
const clearArmed = ref(false);
const shown = ref(false);
const entering = ref(false);
const closing = ref(false);

let toastTimer: number | undefined;
let clearTimer: number | undefined;
let unlistenStatus: UnlistenFn | undefined;
let enterTimer: number | undefined;
let exitTimer: number | undefined;
let updateAutoChecked = false;

const appWindow = getCurrentWindow();

const repoDisplay = computed(() => {
  const url = appInfo.value.repoUrl;
  if (!url) return "本地仓库（未配置远程）";
  return url.replace(/^https?:\/\//i, "").replace(/\.git$/, "");
});

/** Rust 在窗口显示前调用：先把卡片放到右侧外并隐藏，避免首帧闪现 */
function prepareShow() {
  shown.value = false;
  entering.value = false;
  closing.value = false;
}

/** Rust 在窗口稳定显示后调用：从右侧滑入 */
function playEnter() {
  if (closing.value) return;
  shown.value = true;
  entering.value = true;
  if (enterTimer) window.clearTimeout(enterTimer);
  enterTimer = window.setTimeout(() => {
    entering.value = false;
  }, 240);
}

/** 收起前先向右滑出，动画结束后再隐藏窗口 */
async function playExit() {
  if (closing.value) return;
  closing.value = true;
  if (exitTimer) window.clearTimeout(exitTimer);
  exitTimer = window.setTimeout(async () => {
    shown.value = false;
    closing.value = false;
    await appWindow.hide().catch(() => {
      // 极少数情况下 hide 被拒绝时忽略（Rust 侧也会兜底隐藏）
    });
  }, 220);
}

const statusClass = computed(() => {
  const kind = statusView.value.kind;
  if (kind === "ready" || kind === "connected") return "ok";
  if (kind === "needs-sms") return "warn";
  if (kind === "failed") return "error";
  return "muted";
});

function showToast(text: string, type = "info") {
  toast.value = { text, type };
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    toast.value = null;
  }, 2800);
}

function applyView(v: SettingsView) {
  username.value = v.username;
  password.value = v.password;
  smsCode.value = v.smsCode;
  smsStatusText.value = v.smsStatusText;
  savedSmsStatusText.value = v.smsStatusText;
  autostart.value = v.autostart;
  startupBoost.value = v.startupBoost;
  awayMode.value = v.awayMode;
  showAuthWindow.value = v.showAuthWindow;
  autoRelogin.value = v.autoRelogin;
  quitAfterFirstConnect.value = v.quitAfterFirstConnect;
  portalWireless.value = v.portalWireless;
  portalWired.value = v.portalWired;
  statusView.value = v.status;

  const h = v.smsHours;
  if (h < 0) {
    expireChoice.value = "permanent";
  } else if (PRESET_HOURS.includes(h)) {
    expireChoice.value = String(h);
  } else {
    expireChoice.value = "custom";
    customHours.value = h > 0 ? String(h) : "";
  }
}

async function getSettings() {
  try {
    applyView(await invoke<SettingsView>("get_settings"));
  } catch (e) {
    showToast(String(e), "error");
  }
}

async function initRuntimeStatus() {
  try {
    statusView.value = await invoke<StatusView>("get_runtime_status");
  } catch {
    // 等待调度器事件
  }
  maybeAutoCheckUpdate(statusView.value.kind);
  try {
    unlistenStatus = await listen<StatusView>("yulink://status", (e) => {
      statusView.value = e.payload;
      maybeAutoCheckUpdate(e.payload.kind);
      if (e.payload.kind === "needs-sms") {
        showToast(e.payload.text || "动态密码缺失或已过期", "error");
        getSettings();
      } else if (e.payload.kind === "failed") {
        showToast(e.payload.text || "登录失败", "error");
      } else if (e.payload.kind === "connected") {
        showToast(e.payload.text || "已连接", "success");
      }
    });
  } catch {
    // 事件通道不可用时保留静态状态
  }
}

/** 联网成功后再自动查一次更新：没有网络时查也是白查，只查一次。 */
function maybeAutoCheckUpdate(kind: string) {
  if (updateAutoChecked || kind !== "connected") return;
  updateAutoChecked = true;
  checkUpdate(false);
}

async function checkUpdate(force: boolean) {
  if (updateChecking.value) return;
  updateChecking.value = true;
  try {
    updateResult.value = await invoke<UpdateCheck>("check_update", { force });
  } catch (e) {
    updateResult.value = {
      current: appInfo.value.version,
      latest: null,
      hasUpdate: false,
      url: null,
      notes: null,
      error: String(e),
    };
  } finally {
    updateChecking.value = false;
  }
}

const updateText = computed(() => {
  if (updateChecking.value) return "检查中…";
  const result = updateResult.value;
  if (!result) return "点击检查";
  if (result.hasUpdate) return `发现新版本 ${result.latest} ↗`;
  if (result.error) return "检查失败，点击重试";
  return "已是最新";
});

const updateClickable = computed(
  () => !!updateResult.value?.hasUpdate || !updateChecking.value
);

const updateHint = computed(() => updateResult.value?.notes ?? "");

/** 有新版本就打开发布页，否则触发一次强制检查。 */
function onUpdateRowClick() {
  const result = updateResult.value;
  if (result?.hasUpdate && result.url) {
    openUrl(result.url).catch(() => {});
    return;
  }
  checkUpdate(true);
}

function choiceHours(): number | null {
  const v = expireChoice.value;
  if (v === "permanent") return PERMANENT;
  if (v === "custom") {
    const n = parseFloat(customHours.value);
    return Number.isFinite(n) && n > 0 ? n : null;
  }
  return Number(v);
}

function collectCredentialRequest(): SaveRequest | { error: string } {
  const req: SaveRequest = {};
  const acc = username.value.trim();
  const pwd = password.value;
  if (acc || pwd) {
    if (!acc || !pwd) {
      return { error: "账号与密码需同时填写" };
    }
    req.account = { username: acc, password: pwd, isp: "2" };
  }
  const code = smsCode.value.trim();
  if (code) {
    const hours = choiceHours();
    if (hours === null) {
      return { error: "有效期请输入大于 0 的小时数" };
    }
    req.sms = { code, hours };
  }
  return req;
}

async function saveCredentials() {
  const req = collectCredentialRequest();
  if ("error" in req) {
    showToast(req.error, "error");
    return false;
  }
  busy.value = true;
  try {
    applyView(await invoke<SettingsView>("save_settings", { req }));
    showToast("凭据已保存", "success");
    return true;
  } catch (e) {
    showToast(String(e), "error");
    return false;
  } finally {
    busy.value = false;
  }
}

async function saveSettings() {
  busy.value = true;
  const req: SaveRequest = {
    portalWireless: portalWireless.value.trim(),
    portalWired: portalWired.value.trim(),
    awayMode: awayMode.value,
    showAuthWindow: showAuthWindow.value,
    autoRelogin: autoRelogin.value,
    quitAfterFirstConnect: quitAfterFirstConnect.value,
  };
  try {
    applyView(await invoke<SettingsView>("save_settings", { req }));
    showToast("设置已保存", "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

async function toggleAwayMode() {
  const target = !awayMode.value;
  awayMode.value = target;
  busy.value = true;
  try {
    const view = await invoke<SettingsView>("save_settings", {
      req: { awayMode: target },
    });
    applyView(view);
    showToast(
      target ? "已开启离校模式：暂停自动认证" : "已关闭离校模式",
      target ? "info" : "success"
    );
  } catch (e) {
    awayMode.value = !target;
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

async function toggleAutostart() {
  const target = !autostart.value;
  autostart.value = target;
  try {
    const res = await invoke<CommandResult>("set_autostart", { enabled: target });
    showToast(res.message, "success");
  } catch (e) {
    autostart.value = !target;
    showToast(String(e), "error");
  }
}

async function toggleStartupBoost() {
  const target = !startupBoost.value;
  startupBoost.value = target;
  try {
    const view = await invoke<SettingsView>("save_settings", {
      req: { startupBoost: target },
    });
    applyView(view);
    showToast(
      target
        ? "已取消 Windows 对启动项的延迟，登录后会更早拉起"
        : "已恢复 Windows 默认的启动项延迟",
      "info"
    );
  } catch (e) {
    startupBoost.value = !target;
    showToast(String(e), "error");
  }
}

async function toggleShowAuthWindow() {
  const target = !showAuthWindow.value;
  showAuthWindow.value = target;
  try {
    const view = await invoke<SettingsView>("save_settings", {
      req: { showAuthWindow: target },
    });
    applyView(view);
    showToast(target ? "登录时显示认证窗口" : "认证窗口已隐藏", "info");
  } catch (e) {
    showAuthWindow.value = !target;
    showToast(String(e), "error");
  }
}

async function toggleAutoRelogin() {
  if (quitAfterFirstConnect.value) {
    showToast("“连接成功后退出”开启时，自动重试不可用", "info");
    return;
  }
  const target = !autoRelogin.value;
  autoRelogin.value = target;
  try {
    const view = await invoke<SettingsView>("save_settings", {
      req: { autoRelogin: target },
    });
    applyView(view);
    showToast(
      target
        ? "掉线后将一直自动重试登录"
        : "首次连接成功后不再自动登录，仅保留心跳检测",
      "info"
    );
  } catch (e) {
    autoRelogin.value = !target;
    showToast(String(e), "error");
  }
}

async function toggleQuitAfterFirstConnect() {
  const target = !quitAfterFirstConnect.value;
  quitAfterFirstConnect.value = target;
  try {
    const view = await invoke<SettingsView>("save_settings", {
      req: { quitAfterFirstConnect: target },
    });
    applyView(view);
    showToast(
      target
        ? "首次连接成功后程序将自动退出"
        : "已取消“连接成功后退出”",
      target ? "info" : "success"
    );
  } catch (e) {
    quitAfterFirstConnect.value = !target;
    showToast(String(e), "error");
  }
}

async function loginNow() {
  if (awayMode.value) {
    showToast("离校模式已开启，不会执行验证流程", "error");
    return;
  }
  const ok = await saveCredentials();
  if (!ok) return;
  busy.value = true;
  try {
    const res = await invoke<CommandResult>("login_now");
    showToast(res.message, res.ok ? "info" : "error");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

async function clearSms() {
  busy.value = true;
  try {
    applyView(await invoke<SettingsView>("clear_sms_code"));
    showToast("动态密码已清除", "info");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

async function clearAll() {
  if (!clearArmed.value) {
    clearArmed.value = true;
    if (clearTimer) window.clearTimeout(clearTimer);
    clearTimer = window.setTimeout(() => {
      clearArmed.value = false;
    }, 3000);
    return;
  }
  clearArmed.value = false;
  if (clearTimer) window.clearTimeout(clearTimer);
  busy.value = true;
  try {
    applyView(await invoke<SettingsView>("clear_all"));
    showToast("凭据已全部清空", "success");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

function onSmsInput() {
  const code = smsCode.value.trim();
  if (code) {
    smsStatusText.value = "已输入新动态密码，保存后开始计时";
  } else {
    smsStatusText.value = savedSmsStatusText.value || "未设置";
  }
}

function onEscape(e: KeyboardEvent) {
  if (e.key === "Escape") {
    if (page.value === "settings") {
      page.value = "home";
    } else {
      playExit();
    }
  }
}

async function loadAppInfo() {
  try {
    appInfo.value = await invoke<AppInfo>("get_app_info");
  } catch {
    // 信息通道不可用时保持占位显示
  }
}

function openRepo() {
  const url = appInfo.value.repoUrl;
  if (/^https?:\/\//i.test(url)) {
    openUrl(url).catch(() => {});
  }
}

/** 双击头部是 Windows 最大化惯例；悬浮窗不应放大，兜底还原 */
function preventMaximize() {
  appWindow.unmaximize().catch(() => { });
}

onMounted(() => {
  getSettings();
  initRuntimeStatus();
  loadAppInfo();
  window.addEventListener("keydown", onEscape);
  // 供 Rust 侧在窗口稳定显示后调用：__yulinkEnter / __yulinkExit
  (window as unknown as Record<string, unknown>).__yulinkPrepare = prepareShow;
  (window as unknown as Record<string, unknown>).__yulinkEnter = playEnter;
  (window as unknown as Record<string, unknown>).__yulinkExit = playExit;
});

onUnmounted(() => {
  unlistenStatus?.();
  window.removeEventListener("keydown", onEscape);
  if (toastTimer) window.clearTimeout(toastTimer);
  if (clearTimer) window.clearTimeout(clearTimer);
  if (enterTimer) window.clearTimeout(enterTimer);
  if (exitTimer) window.clearTimeout(exitTimer);
});
</script>

<template>
  <div class="page">
    <div class="flyout-card" :class="{
      'flyout-hidden': !shown && !closing,
      'flyout-in': entering,
      'flyout-out': closing,
    }">
      <header class="head" data-tauri-drag-region @dblclick.prevent="preventMaximize">
        <template v-if="page === 'home'">
          <img class="brand-logo" src="/yulink.png" alt="" draggable="false" />
          <div class="title-box" data-tauri-drag-region>
            <h1>御连 YuLink</h1>
            <p>校园网自动认证</p>
          </div>
          <button class="icon-btn" type="button" title="设置" @click="page = 'settings'">
            <svg viewBox="0 0 24 24" width="15" height="15">
              <circle cx="12" cy="12" r="3" />
              <path
                d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1 1.55V21a2 2 0 1 1-4 0v-.09a1.7 1.7 0 0 0-1-1.55 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.55-1H3a2 2 0 1 1 0-4h.09a1.7 1.7 0 0 0 1.55-1 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34h.09a1.7 1.7 0 0 0 1-1.55V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1 1.55h.09a1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87v.09a1.7 1.7 0 0 0 1.55 1H21a2 2 0 1 1 0 4h-.09a1.7 1.7 0 0 0-1.55 1z" />
            </svg>
          </button>
        </template>
        <template v-else>
          <div class="title-box" data-tauri-drag-region>
            <h1>设置</h1>
            <p>认证网址 / 运行模式 / 自启</p>
          </div>
          <button class="icon-btn" type="button" title="返回" @click="page = 'home'">
            <svg viewBox="0 0 24 24" width="15" height="15">
              <path d="M15 18l-6-6 6-6" />
            </svg>
          </button>
        </template>
      </header>

      <section class="status" :class="statusClass">
        <span class="dot" aria-hidden="true"></span>
        <div>
          <strong>{{ statusView.text }}</strong>
          <small>动态密码 {{ smsStatusText }}</small>
        </div>
        <button v-if="page === 'home'" class="login-chip" type="button" :disabled="busy || awayMode" @click="loginNow">
          <svg viewBox="0 0 24 24" width="12" height="12">
            <path d="M8 5.5v13l10-6.5z" />
          </svg>
          {{ busy ? "处理中" : awayMode ? "离校模式" : "立即登录" }}
        </button>
      </section>

      <main v-if="page === 'home'" class="body">
        <section class="group">
          <div class="group-title">账号</div>
          <div class="field-card">
            <label class="row">
              <span>校园网账号</span>
              <input v-model="username" autocomplete="off" spellcheck="false" placeholder="请输入账号" />
            </label>
            <label class="row">
              <span>密码</span>
              <span class="pwd-wrap">
                <input v-model="password" :type="showPassword ? 'text' : 'password'" autocomplete="new-password"
                  placeholder="请输入密码" />
                <button class="eye" type="button" @click="showPassword = !showPassword">
                  {{ showPassword ? "隐藏" : "显示" }}
                </button>
              </span>
            </label>
          </div>
        </section>

        <section class="group">
          <div class="group-title">
            <span>动态密码</span>
            <button class="text-btn" type="button" @click="clearSms">清除</button>
          </div>
          <div class="field-card">
            <input v-model="smsCode" class="sms-input" maxlength="8" autocomplete="off" spellcheck="false"
              placeholder="电信短信下发的动态密码" @input="onSmsInput" />
            <div class="expire-row">
              <span>有效期</span>
              <span class="select">
                <select v-model="expireChoice">
                  <option v-for="h in PRESET_HOURS" :key="h" :value="String(h)">{{ h }} 小时</option>
                  <option value="permanent">永久</option>
                  <option value="custom">自定义…</option>
                </select>
                <svg viewBox="0 0 24 24" width="12" height="12">
                  <path d="m6 9 6 6 6-6" />
                </svg>
              </span>
            </div>
            <div v-if="expireChoice === 'custom'" class="custom-row">
              <input v-model="customHours" type="number" min="0.1" step="0.5" placeholder="小时数" />
              <span>小时</span>
            </div>
            <p class="hint">保存后开始计时；有效期内可重复使用，登录失败不会自动清除</p>
          </div>
        </section>
      </main>

      <main v-else class="body">
        <section class="group">
          <div class="group-title">认证网址</div>
          <div class="field-card">
            <label class="row url-row">
              <span>无线认证</span>
              <input v-model="portalWireless" spellcheck="false" placeholder="http://172.26.255.2/" />
            </label>
            <label class="row url-row">
              <span>有线认证</span>
              <input v-model="portalWired" spellcheck="false" placeholder="http://172.26.255.3/" />
            </label>
          </div>
        </section>

        <section class="group">
          <div class="group-title">运行</div>
          <div class="switch-card" @click="toggleAwayMode">
            <div>
              <strong>离校模式</strong>
              <small>开启后不会自动启动，也不会执行验证流程</small>
            </div>
            <span class="switch" :class="{ on: awayMode }"><i></i></span>
          </div>
          <div class="switch-card" @click="toggleAutostart">
            <div>
              <strong>开机自启动</strong>
              <small>登录 Windows 后自动运行</small>
            </div>
            <span class="switch" :class="{ on: autostart }"><i></i></span>
          </div>
          <div class="switch-card" @click="toggleStartupBoost">
            <div>
              <strong>开机启动加速</strong>
              <small>
                取消 Windows 给登录后启动项加的延迟，登录后更早拉起（对本机当前用户的所有自启项生效）
              </small>
            </div>
            <span class="switch" :class="{ on: startupBoost }"><i></i></span>
          </div>
          <div class="switch-card" @click="toggleShowAuthWindow">
            <div>
              <strong>显示认证窗口</strong>
              <small>登录时显示门户页面；关闭后后台自动完成</small>
            </div>
            <span class="switch" :class="{ on: showAuthWindow }"><i></i></span>
          </div>
          <div
            class="switch-card"
            :class="{ disabled: quitAfterFirstConnect }"
            @click="toggleAutoRelogin"
          >
            <div>
              <strong>断网自动重试</strong>
              <small v-if="!quitAfterFirstConnect">
                关闭后首次连接成功便不再自动登录，仅保留心跳检测
              </small>
              <small v-else>已由“连接成功后退出”接管，无需自动重试</small>
            </div>
            <span class="switch" :class="{ on: autoRelogin }"><i></i></span>
          </div>
          <div class="switch-card" @click="toggleQuitAfterFirstConnect">
            <div>
              <strong>连接成功后退出</strong>
              <small>开机后首次连接成功即关闭程序，不再维护后续事务</small>
            </div>
            <span class="switch" :class="{ on: quitAfterFirstConnect }"><i></i></span>
          </div>
        </section>

        <section class="group">
          <div class="group-title">数据</div>
          <div class="danger-card">
            <div>
              <strong>清除已保存的账号、密码与动态密码</strong>
            </div>
            <button class="danger-btn" type="button" :disabled="busy" @click="clearAll">
              {{ clearArmed ? "再点一次确认" : "清空数据" }}
            </button>
          </div>
        </section>

        <section class="group">
          <div class="group-title">关于</div>
          <div class="field-card">
            <div class="row">
              <span>名称</span>
              <span class="about-value">
                <img class="about-logo" src="/yulink.png" alt="YuLink" draggable="false" />
                御连 YuLink
              </span>
            </div>
            <div class="row">
              <span>版本</span>
              <span class="about-value">v{{ appInfo.version || "未知" }}</span>
            </div>
            <div
              class="row about-link"
              :class="{ clickable: /^https?:/i.test(appInfo.repoUrl) }"
              @click="openRepo"
            >
              <span>仓库</span>
              <span class="about-value">
                <template v-if="/^https?:/i.test(appInfo.repoUrl)">
                  {{ repoDisplay }} ↗
                </template>
                <template v-else>本地仓库（未配置远程）</template>
              </span>
            </div>
            <div
              class="row about-link"
              :class="{ clickable: updateClickable }"
              @click="onUpdateRowClick"
            >
              <span>更新</span>
              <span class="about-value" :title="updateHint">{{ updateText }}</span>
            </div>
          </div>
        </section>
      </main>

      <footer class="foot">
        <span class="spacer"></span>
        <button v-if="page === 'settings'" class="primary" type="button" :disabled="busy" @click="saveSettings">
          {{ busy ? "保存中…" : "保存设置" }}
        </button>
        <button v-else class="primary" type="button" :disabled="busy" @click="saveCredentials">
          {{ busy ? "保存中…" : "保存凭据" }}
        </button>
      </footer>
    </div>

    <Transition name="toast">
      <div v-if="toast" class="toast" :class="toast.type">{{ toast.text }}</div>
    </Transition>
  </div>
</template>

<style>
* {
  box-sizing: border-box;
  -webkit-user-select: none;
  user-select: none;
}

html,
body,
#app {
  height: 100%;
  margin: 0;
  background: transparent;
  overflow: hidden;
}

body {
  font-family: "Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei", system-ui, sans-serif;
  color: #1b1b1b;
  --space-1: 4px;
  --space-2: 8px;
  --space-3: 12px;
  --space-4: 16px;
  --space-5: 20px;
  --space-6: 24px;
  --radius-sm: 8px;
  --radius-md: 12px;
  --hairline: 1px solid rgba(0, 0, 0, 0.07);
}

.page {
  height: 100vh;
  padding: var(--space-3);
}

.flyout-card {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: #f7f8fb;
  border: var(--hairline);
  border-radius: var(--radius-md);
  box-shadow: 0 3px 12px rgba(0, 0, 0, 0.14);
  overflow: hidden;
}

.flyout-in {
  animation: flyout-in 0.22s cubic-bezier(0.16, 0.84, 0.32, 1);
}

.flyout-out {
  animation: flyout-out 0.18s cubic-bezier(0.7, 0, 0.84, 0) forwards;
}

.flyout-hidden {
  opacity: 0;
  transform: translateX(46px);
}

@keyframes flyout-in {
  from {
    transform: translateX(46px);
    opacity: 0;
  }

  to {
    transform: translateX(0);
    opacity: 1;
  }
}

@keyframes flyout-out {
  from {
    transform: translateX(0);
    opacity: 1;
  }

  to {
    transform: translateX(60px);
    opacity: 0;
  }
}

.head {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-4) var(--space-5) var(--space-2);
  -webkit-app-region: drag;
}

.brand-icon {
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  background: linear-gradient(145deg, #3b82f6, #1d5fd6);
  color: #fff;
  box-shadow: 0 3px 8px rgba(29, 95, 214, 0.3);
}

.brand-logo {
  width: 32px;
  height: 32px;
  flex: 0 0 auto;
  border-radius: 8px;
  object-fit: contain;
  background: #ffffff;
  border: 1px solid rgba(0, 0, 0, 0.06);
}

.brand-icon svg,
.login-chip svg {
  fill: currentColor;
  stroke: currentColor;
  stroke-width: 1.5;
  stroke-linejoin: round;
}

.title-box {
  flex: 1;
  min-width: 0;
  -webkit-app-region: drag;
}

.title-box h1 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
}

.title-box p {
  margin: 1px 0 0;
  font-size: 10.5px;
  color: #7a7f89;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.icon-btn {
  width: 30px;
  height: 30px;
  flex: 0 0 auto;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: #5b6068;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
}

.icon-btn svg {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.7;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.icon-btn:hover {
  background: rgba(0, 0, 0, 0.07);
}

.status {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin: 0;
  padding: var(--space-3) var(--space-6);
}

.status .dot {
  width: 9px;
  height: 9px;
  border-radius: 50%;
  background: #9aa0a8;
  flex: 0 0 auto;
}

.status div {
  flex: 1;
  min-width: 0;
}

.status strong,
.status small {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.status strong {
  font-size: 13px;
  font-weight: 600;
}

.status small {
  margin-top: 1px;
  font-size: 11px;
  color: #7a7f89;
}

.status.ok .dot {
  background: #1fa155;
  box-shadow: 0 0 0 4px rgba(31, 161, 85, 0.15);
}

.status.warn .dot {
  background: #d08b00;
  box-shadow: 0 0 0 4px rgba(208, 139, 0, 0.15);
}

.status.error .dot {
  background: #d83b3b;
  box-shadow: 0 0 0 4px rgba(216, 59, 59, 0.15);
}

.login-chip {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: none;
  border-radius: var(--radius-sm);
  padding: var(--space-2) var(--space-3);
  background: #1d64d8;
  color: #fff;
  font: 600 12px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  cursor: pointer;
}

.login-chip:hover:not(:disabled) {
  background: #1756c0;
}

.login-chip:disabled {
  opacity: 0.55;
  cursor: default;
}

.body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: var(--space-2) var(--space-6) var(--space-6);
  scrollbar-width: thin;
  scrollbar-color: rgba(0, 0, 0, 0.22) transparent;
}

.group {
  margin-bottom: var(--space-5);
}

.group:first-child {
  margin-top: var(--space-1);
}

.group-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
  font-weight: 600;
  color: #71767e;
  margin: var(--space-2) 0 var(--space-1);
}

.text-btn {
  border: none;
  background: none;
  color: #1d64d8;
  font-size: 11px;
  cursor: pointer;
  text-transform: none;
  padding: 2px 4px;
  border-radius: 5px;
}

.text-btn:hover {
  background: rgba(29, 100, 216, 0.08);
}

.field-card,
.switch-card {
  background: transparent;
  border: none;
  border-radius: 0;
  padding: 0;
}

.switch-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  padding: var(--space-2) 0;
  cursor: pointer;
  border-bottom: var(--hairline);
}

.switch-card.disabled {
  opacity: 0.45;
  pointer-events: none;
}

.switch-card strong,
.switch-card small,
.danger-card strong {
  display: block;
}

.switch-card strong,
.danger-card strong {
  font-size: 13px;
}

.switch-card small {
  margin-top: 1px;
  font-size: 11px;
  color: #7a7f89;
}

.row {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  min-height: 44px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.05);
}

.row:last-child {
  border-bottom: none;
}

.row>span:first-child {
  flex: 0 0 74px;
  font-size: 12px;
  color: #565b64;
}

.row .about-value {
  flex: 1;
  min-width: 0;
  display: inline-flex;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: #1b1b1b;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row .about-value .about-logo {
  width: 18px;
  height: 18px;
  border-radius: 4px;
  flex: none;
  user-select: none;
}

.row.about-link {
  cursor: default;
}

.row.about-link.clickable {
  cursor: pointer;
}

.row.about-link.clickable .about-value {
  color: #1d64d8;
}

.row input,
.sms-input,
.custom-row input {
  flex: 1;
  min-width: 0;
  border: none;
  background: transparent;
  font: 13px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  color: #1b1b1b;
  outline: none;
  text-align: left;
}

.url-row input {
  font-size: 12px;
  color: #333;
}

.pwd-wrap {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 4px;
}

.pwd-wrap input {
  text-align: left;
}

.eye {
  border: none;
  background: transparent;
  color: #1d64d8;
  font-size: 11px;
  cursor: pointer;
  padding: 3px 5px;
  border-radius: 5px;
}

.eye:hover {
  background: rgba(29, 100, 216, 0.08);
}

.sms-input {
  width: 100%;
  text-align: left;
  padding: 9px 0 4px;
}

.expire-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 34px;
  font-size: 12px;
  color: #565b64;
}

.select {
  position: relative;
  display: inline-flex;
  align-items: center;
}

.select select {
  appearance: none;
  border: 1px solid rgba(0, 0, 0, 0.08);
  background: #f1f2f6;
  border-radius: 7px;
  padding: 6px 26px 6px 10px;
  font: 12px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  color: #1b1b1b;
  cursor: pointer;
  outline: none;
}

.select select:focus {
  border-color: #1d64d8;
}

.select svg {
  position: absolute;
  right: 8px;
  pointer-events: none;
  fill: none;
  stroke: #71767e;
  stroke-width: 2;
  stroke-linecap: round;
  stroke-linejoin: round;
}

.custom-row {
  display: flex;
  align-items: center;
  gap: 6px;
  padding-bottom: 8px;
  font-size: 11px;
  color: #71767e;
}

.custom-row input {
  flex: 0 0 90px;
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 7px;
  background: #f1f2f6;
  padding: 5px 8px;
  text-align: left;
}

.hint {
  margin: 0 0 9px;
  font-size: 10.5px;
  color: #8a8f98;
  line-height: 1.5;
}

.switch {
  position: relative;
  width: 40px;
  height: 20px;
  border-radius: 999px;
  background: #c9ccd2;
  transition: background 0.15s;
  flex: 0 0 auto;
}

.switch i {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
  transition: left 0.15s;
}

.switch.on {
  background: #1d64d8;
}

.switch.on i {
  left: 22px;
}

.danger-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--space-3);
  background: transparent;
  border: none;
  border-radius: 0;
  padding: var(--space-3) 0;
}

.danger-btn {
  flex: 0 0 auto;
  border: 1px solid rgba(176, 50, 47, 0.35);
  background: transparent;
  color: #b0322f;
  border-radius: 8px;
  padding: 6px 10px;
  font: 600 11px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  cursor: pointer;
}

.danger-btn:hover:not(:disabled) {
  background: rgba(176, 50, 47, 0.07);
}

.danger-btn:disabled {
  opacity: 0.55;
}

.foot {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-3) var(--space-6) var(--space-5);
}

.foot button {
  border: none;
  border-radius: 8px;
  padding: 7px 14px;
  font: 600 12px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  cursor: pointer;
}

.foot button:disabled {
  opacity: 0.55;
}

.spacer {
  flex: 1;
}

.ghost {
  background: rgba(0, 0, 0, 0.07);
  color: #1b1b1b;
}

.ghost:hover {
  background: rgba(0, 0, 0, 0.11);
}

.primary {
  background: #1d64d8;
  color: #fff;
}

.primary:hover {
  background: #1756c0;
}

.toast {
  position: fixed;
  left: 50%;
  bottom: 58px;
  transform: translateX(-50%);
  max-width: 320px;
  background: #1c1f24;
  color: #fff;
  font-size: 12px;
  padding: 8px 14px;
  border-radius: 8px;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.2);
  z-index: 99;
  pointer-events: none;
}

.toast.success {
  background: #147a3e;
}

.toast.error {
  background: #b0322f;
}

.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.18s, transform 0.18s;
}

.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, 6px);
}

@media (prefers-color-scheme: dark) {
  body {
    color: #e9eaee;
  }

  .flyout-card {
    background: #202124;
    border-color: rgba(255, 255, 255, 0.1);
  }

  .status,
  .field-card,
  .switch-card,
  .danger-card {
    background: transparent;
    border-color: rgba(255, 255, 255, 0.07);
  }

  .row>span:first-child,
  .group-title,
  .expire-row,
  .hint,
  .title-box p,
  .status small,
  .switch-card small {
    color: #a8adb8;
  }

  .row input,
  .sms-input,
  .custom-row input {
    color: #e9eaee;
  }

  .select select,
  .custom-row input {
    background: rgba(255, 255, 255, 0.09);
    color: #e9eaee;
  }

  .ghost {
    background: rgba(255, 255, 255, 0.1);
    color: #e9eaee;
  }

  .icon-btn:hover,
  .eye:hover,
  .text-btn:hover {
    background: rgba(255, 255, 255, 0.08);
  }
}
</style>

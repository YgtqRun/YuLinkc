<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

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
  status: StatusView;
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
}

interface CommandResult {
  ok: boolean;
  message: string;
}

const PRESET_HOURS = [1, 3, 9, 18, 27];
const DEFAULT_HOURS = 27;
const PERMANENT = -1;

const username = ref("");
const password = ref("");
const showPassword = ref(false);
const smsCode = ref("");
const smsStatusText = ref("未设置");
const savedSmsStatusText = ref("未设置");
const expireChoice = ref(String(DEFAULT_HOURS));
const customHours = ref("");
const autostart = ref(false);
const statusView = ref<StatusView>({ kind: "checking", text: "读取中…" });
const busy = ref(false);
const toast = ref<{ text: string; type: string } | null>(null);
const clearArmed = ref(false);

let toastTimer: number | undefined;
let clearTimer: number | undefined;
let unlistenStatus: UnlistenFn | undefined;

const appWindow = getCurrentWindow();

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
  try {
    unlistenStatus = await listen<StatusView>("yulink://status", (e) => {
      statusView.value = e.payload;
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

function choiceHours(): number | null {
  const v = expireChoice.value;
  if (v === "permanent") return PERMANENT;
  if (v === "custom") {
    const n = parseFloat(customHours.value);
    return Number.isFinite(n) && n > 0 ? n : null;
  }
  return Number(v);
}

function collectRequest(): SaveRequest | { error: string } {
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

async function save() {
  const req = collectRequest();
  if ("error" in req) {
    showToast(req.error, "error");
    return false;
  }
  busy.value = true;
  try {
    applyView(await invoke<SettingsView>("save_settings", { req }));
    showToast("已保存", "success");
    return true;
  } catch (e) {
    showToast(String(e), "error");
    return false;
  } finally {
    busy.value = false;
  }
}

async function loginNow() {
  const ok = await save();
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
    appWindow.hide();
  }
}

function hideFlyout() {
  appWindow.hide();
}

onMounted(() => {
  getSettings();
  initRuntimeStatus();
  window.addEventListener("keydown", onEscape);
});

onUnmounted(() => {
  unlistenStatus?.();
  window.removeEventListener("keydown", onEscape);
  if (toastTimer) window.clearTimeout(toastTimer);
  if (clearTimer) window.clearTimeout(clearTimer);
});
</script>

<template>
  <div class="page">
    <div class="flyout-card">
      <header class="head" data-tauri-drag-region>
        <div class="brand-icon" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="18" height="18">
            <path d="M8.5 3h7M12 3v4" />
            <path d="M7 7h10l-1.2 12a2 2 0 0 1-2 1.8h-3.6a2 2 0 0 1-2-1.8L7 7z" />
            <path d="M10 11.5v4M14 11.5v4" />
          </svg>
        </div>
        <div class="title-box" data-tauri-drag-region>
          <h1>御连 YuLink</h1>
          <p>校园网自动认证</p>
        </div>
        <button class="icon-btn" type="button" title="收起" @click="hideFlyout">
          <svg viewBox="0 0 24 24" width="14" height="14">
            <path d="M18 6 6 18M6 6l12 12" />
          </svg>
        </button>
      </header>

      <section class="status" :class="statusClass">
        <span class="dot" aria-hidden="true"></span>
        <div>
          <strong>{{ statusView.text }}</strong>
          <small>动态密码 {{ smsStatusText }}</small>
        </div>
        <button class="login-chip" type="button" :disabled="busy" @click="loginNow">
          <svg viewBox="0 0 24 24" width="12" height="12">
            <path d="M8 5.5v13l10-6.5z" />
          </svg>
          {{ busy ? "处理中" : "立即登录" }}
        </button>
      </section>

      <main class="body">
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
                <input
                  v-model="password"
                  :type="showPassword ? 'text' : 'password'"
                  autocomplete="new-password"
                  placeholder="请输入密码"
                />
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
            <input
              v-model="smsCode"
              class="sms-input"
              maxlength="8"
              autocomplete="off"
              spellcheck="false"
              placeholder="电信短信下发的动态密码"
              @input="onSmsInput"
            />
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

        <section class="switch-card" @click="toggleAutostart">
          <div>
            <strong>开机自启</strong>
            <small>登录 Windows 后自动完成认证</small>
          </div>
          <span class="switch" :class="{ on: autostart }"><i></i></span>
        </section>
      </main>

      <footer class="foot">
        <button class="danger" type="button" :disabled="busy" @click="clearAll">
          {{ clearArmed ? "再点一次确认清空" : "清空凭据" }}
        </button>
        <span class="spacer"></span>
        <button class="ghost" type="button" :disabled="busy" @click="save">保存</button>
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
}
.page {
  height: 100vh;
  padding: 12px;
}
.flyout-card {
  position: relative;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: linear-gradient(180deg, rgba(250, 250, 252, 0.96), rgba(242, 243, 246, 0.96));
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 12px;
  box-shadow: 0 3px 12px rgba(0, 0, 0, 0.12);
  overflow: hidden;
}
.head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 14px 16px 10px;
  -webkit-app-region: drag;
}
.brand-icon {
  width: 36px;
  height: 36px;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 10px;
  background: linear-gradient(145deg, #3b82f6, #1d5fd6);
  color: #fff;
  box-shadow: 0 3px 8px rgba(29, 95, 214, 0.35);
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
  -webkit-app-region: drag;
}
.title-box h1 {
  margin: 0;
  font-size: 15px;
  font-weight: 600;
  letter-spacing: 0.2px;
}
.title-box p {
  margin: 1px 0 0;
  font-size: 11px;
  color: #7a7f89;
}
.icon-btn {
  width: 30px;
  height: 30px;
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
  stroke-width: 1.8;
  stroke-linecap: round;
}
.icon-btn:hover {
  background: rgba(0, 0, 0, 0.06);
}
.status {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 2px 16px 10px;
  padding: 10px 12px;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.72);
  border: 1px solid rgba(0, 0, 0, 0.05);
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
  box-shadow: 0 0 0 4px rgba(31, 161, 85, 0.14);
}
.status.warn .dot {
  background: #d08b00;
  box-shadow: 0 0 0 4px rgba(208, 139, 0, 0.14);
}
.status.error .dot {
  background: #d83b3b;
  box-shadow: 0 0 0 4px rgba(216, 59, 59, 0.14);
}
.login-chip {
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  border: none;
  border-radius: 8px;
  padding: 8px 12px;
  background: #1d64d8;
  color: #fff;
  font: 600 12px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  cursor: pointer;
  box-shadow: 0 2px 6px rgba(29, 100, 216, 0.28);
}
.login-chip:hover:not(:disabled) {
  background: #1756c0;
}
.login-chip:disabled {
  opacity: 0.65;
}
.body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 2px 16px 8px;
  scrollbar-width: thin;
}
.group {
  margin-bottom: 12px;
}
.group-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 11px;
  font-weight: 600;
  color: #71767e;
  margin: 2px 4px 6px;
  text-transform: uppercase;
  letter-spacing: 0.4px;
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
  background: rgba(255, 255, 255, 0.78);
  border: 1px solid rgba(0, 0, 0, 0.05);
  border-radius: 10px;
  padding: 4px 12px;
}
.row {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 44px;
  border-bottom: 1px solid rgba(0, 0, 0, 0.045);
}
.row:last-child {
  border-bottom: none;
}
.row > span:first-child {
  flex: 0 0 72px;
  font-size: 12px;
  color: #565b64;
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
  text-align: right;
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
  padding: 11px 0 5px;
}
.expire-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 38px;
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
  background: #f3f4f7;
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
  background: #f3f4f7;
  padding: 5px 8px;
  text-align: left;
}
.hint {
  margin: 0 0 9px;
  font-size: 10.5px;
  color: #8a8f98;
  line-height: 1.5;
}
.switch-card {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 12px;
  cursor: pointer;
  margin-bottom: 6px;
}
.switch-card strong,
.switch-card small {
  display: block;
}
.switch-card strong {
  font-size: 13px;
}
.switch-card small {
  margin-top: 1px;
  font-size: 11px;
  color: #7a7f89;
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
.foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 16px 14px;
}
.foot button {
  border: none;
  border-radius: 8px;
  padding: 8px 14px;
  font: 600 12px "Segoe UI Variable Text", "Segoe UI", sans-serif;
  cursor: pointer;
}
.foot button:disabled {
  opacity: 0.55;
}
.danger {
  background: transparent;
  color: #b0322f;
  padding-left: 6px !important;
  padding-right: 6px !important;
}
.danger:hover {
  background: rgba(176, 50, 47, 0.07);
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
.toast {
  position: fixed;
  left: 50%;
  bottom: 66px;
  transform: translateX(-50%);
  max-width: 320px;
  background: rgba(28, 31, 36, 0.92);
  color: #fff;
  font-size: 12px;
  padding: 8px 14px;
  border-radius: 8px;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.2);
  z-index: 99;
  pointer-events: none;
}
.toast.success {
  background: rgba(20, 122, 62, 0.95);
}
.toast.error {
  background: rgba(176, 50, 47, 0.95);
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
    background: linear-gradient(180deg, rgba(40, 42, 48, 0.97), rgba(31, 33, 38, 0.97));
    border-color: rgba(255, 255, 255, 0.07);
  }
  .field-card,
  .switch-card,
  .status {
    background: rgba(255, 255, 255, 0.06);
    border-color: rgba(255, 255, 255, 0.05);
  }
  .row > span:first-child,
  .group-title,
  .expire-row,
  .hint,
  .title-box p,
  .status small,
  .switch-card small {
    color: #a8adb8;
  }
  .select select,
  .custom-row input {
    background: rgba(255, 255, 255, 0.08);
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

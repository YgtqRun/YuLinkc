<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

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

const EXPIRE_OPTIONS = [
  ...PRESET_HOURS.map((h) => ({ value: String(h), label: `${h} 小时` })),
  { value: "permanent", label: "永久" },
  { value: "custom", label: "自定义…" },
];

const username = ref("");
const password = ref("");
const showPassword = ref(false);
const smsCode = ref("");
const smsStatusText = ref("未设置");
const savedSmsStatusText = ref("未设置");
const smsEdited = ref(false);
const expireChoice = ref(String(DEFAULT_HOURS));
const customHours = ref("");
const expireMenuOpen = ref(false);
const autostart = ref(false);
const statusView = ref<StatusView>({ kind: "unconfigured", text: "读取中…" });
const busy = ref(false);
const toast = ref<{ text: string; type: string } | null>(null);
const clearArmed = ref(false);
let toastTimer: number | undefined;
let clearTimer: number | undefined;

const expireLabel = computed(() => {
  const v = expireChoice.value;
  if (v === "permanent") return "永久";
  if (v === "custom") {
    const n = parseFloat(customHours.value);
    return Number.isFinite(n) && n > 0 ? `${n} 小时` : "自定义…";
  }
  return EXPIRE_OPTIONS.find((o) => o.value === v)?.label ?? "27 小时";
});

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
  smsEdited.value = false;
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
    const v = await invoke<SettingsView>("get_settings");
    applyView(v);
  } catch (e) {
    showToast(String(e), "error");
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
      return { error: "自定义有效期请输入大于 0 的小时数" };
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
    const v = await invoke<SettingsView>("save_settings", { req });
    applyView(v);
    showToast("设置已保存", "success");
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
    showToast(res.message, res.ok ? "success" : "error");
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    busy.value = false;
  }
}

async function clearSms() {
  busy.value = true;
  try {
    const v = await invoke<SettingsView>("clear_sms_code");
    applyView(v);
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
    const v = await invoke<SettingsView>("clear_all");
    applyView(v);
    showToast("账号、密码、动态密码等数据已全部清空", "success");
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
  smsEdited.value = true;
  const code = smsCode.value.trim();
  if (code) {
    smsStatusText.value = "已输入新动态密码，保存后开始计时";
  } else {
    smsStatusText.value = savedSmsStatusText.value || "未设置";
    smsEdited.value = false;
  }
}

function chooseExpire(value: string) {
  expireChoice.value = value;
  expireMenuOpen.value = false;
  if (value === "custom") {
    requestAnimationFrame(() => {
      document.getElementById("custom-hours")?.focus();
    });
  }
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    expireMenuOpen.value = false;
  }
}

onMounted(() => {
  getSettings();
  window.addEventListener("keydown", onKeydown);
});

onUnmounted(() => {
  window.removeEventListener("keydown", onKeydown);
  if (toastTimer) window.clearTimeout(toastTimer);
  if (clearTimer) window.clearTimeout(clearTimer);
});
</script>

<template>
  <div class="page">
    <div class="card">
      <header class="head">
        <div>
          <h1>御连 YuLink</h1>
          <p class="sub">校园网自动认证常驻助手</p>
        </div>
        <span class="status" :class="statusClass">
          <i class="dot" aria-hidden="true"></i>
          {{ statusView.text }}
        </span>
      </header>

      <section class="body">
        <div class="field">
          <label for="acc">校园网账号</label>
          <input
            id="acc"
            v-model="username"
            class="input"
            autocomplete="off"
            spellcheck="false"
            placeholder="请输入账号"
            :disabled="busy"
          />
        </div>

        <div class="field">
          <label for="pwd">密码</label>
          <div class="append">
            <input
              id="pwd"
              v-model="password"
              class="input"
              :type="showPassword ? 'text' : 'password'"
              autocomplete="new-password"
              placeholder="请输入密码"
              :disabled="busy"
            />
            <button class="eye" type="button" @click="showPassword = !showPassword">
              {{ showPassword ? "隐藏" : "显示" }}
            </button>
          </div>
        </div>

        <div class="sep">动态密码</div>

        <div class="field">
          <div class="label-row">
            <label for="sms">动态密码</label>
            <button class="link" type="button" @click="clearSms">清除动态密码</button>
          </div>
          <input
            id="sms"
            v-model="smsCode"
            class="input"
            maxlength="8"
            autocomplete="off"
            spellcheck="false"
            placeholder="请输入电信下发的动态密码"
            :disabled="busy"
            @input="onSmsInput"
          />
          <p class="hint" :class="{ edited: smsEdited }">{{ smsStatusText }}</p>
        </div>

        <div class="field">
          <label>动态密码有效期</label>
          <div class="select-wrap">
            <button
              class="select-trigger"
              type="button"
              :aria-expanded="expireMenuOpen"
              @click="expireMenuOpen = !expireMenuOpen"
            >
              <span>{{ expireLabel }}</span>
              <svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true">
                <path d="m6 9 6 6 6-6" />
              </svg>
            </button>
            <div v-if="expireMenuOpen" class="select-menu" role="listbox">
              <button
                v-for="opt in EXPIRE_OPTIONS"
                :key="opt.value"
                class="option"
                :class="{ selected: expireChoice === opt.value }"
                type="button"
                role="option"
                :aria-selected="expireChoice === opt.value"
                @click="chooseExpire(opt.value)"
              >
                {{ opt.label }}
              </button>
            </div>
          </div>
          <div v-if="expireChoice === 'custom'" class="custom-row">
            <input
              id="custom-hours"
              v-model="customHours"
              class="input"
              type="number"
              min="0.1"
              step="0.5"
              placeholder="小时数，如 6"
            />
            <span class="unit">小时</span>
          </div>
          <p class="hint">保存动态密码后开始计时；有效期内可重复使用，登录失败不会自动清除</p>
        </div>

        <div class="field switch-row">
          <div>
            <label>开机自启</label>
            <p class="hint">用户登录 Windows 后自动启动并完成认证</p>
          </div>
          <button
            class="switch"
            :class="{ on: autostart }"
            type="button"
            role="switch"
            :aria-checked="autostart"
            :disabled="busy"
            @click="toggleAutostart"
          >
            <i></i>
          </button>
        </div>
      </section>

      <footer class="foot">
        <button
          class="btn danger"
          type="button"
          :disabled="busy"
          @click="clearAll"
        >
          {{ clearArmed ? "再点一次确认清空" : "清空数据" }}
        </button>
        <span class="flex"></span>
        <button class="btn ghost" type="button" :disabled="busy" @click="save">
          保存
        </button>
        <button class="btn primary" type="button" :disabled="busy" @click="loginNow">
          {{ busy ? "处理中…" : "立即登录" }}
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
}
html,
body,
#app {
  height: 100%;
  margin: 0;
}
body {
  font-family: "Microsoft YaHei", "PingFang SC", system-ui, -apple-system, sans-serif;
  background: linear-gradient(160deg, #eef3fb 0%, #e7edf7 100%);
  color: #1f2937;
}
.page {
  min-height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 18px;
}
.card {
  width: min(92vw, 440px);
  max-height: calc(100vh - 36px);
  display: flex;
  flex-direction: column;
  background: #fff;
  border-radius: 16px;
  box-shadow: 0 18px 50px rgba(15, 23, 42, 0.16), 0 2px 8px rgba(15, 23, 42, 0.06);
  overflow: hidden;
}
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 18px 20px 14px;
  border-bottom: 1px solid #eef0f3;
}
h1 {
  margin: 0;
  font-size: 19px;
  letter-spacing: 0.2px;
}
.sub {
  margin: 3px 0 0;
  font-size: 12px;
  color: #8a94a6;
}
.status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  padding: 5px 10px;
  border-radius: 999px;
  white-space: nowrap;
}
.status .dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #9ca3af;
  box-shadow: 0 0 0 2px #fff;
}
.status.ok {
  background: #e8f8ef;
  color: #178f4a;
}
.status.ok .dot {
  background: #16a34a;
}
.status.warn {
  background: #fff6e5;
  color: #ad6800;
}
.status.warn .dot {
  background: #f59e0b;
}
.status.error {
  background: #fff1f0;
  color: #cf1322;
}
.status.error .dot {
  background: #dc2626;
}
.status.muted {
  background: #f1f3f6;
  color: #5b6472;
}
.body {
  flex: 1;
  overflow-y: auto;
  padding: 2px 20px 8px;
}
.field {
  margin: 14px 0;
}
label {
  display: block;
  font-size: 12px;
  color: #667085;
  margin-bottom: 6px;
}
.label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.label-row label {
  margin-bottom: 0;
}
.input {
  width: 100%;
  padding: 10px 12px;
  border: 1px solid #d7dce4;
  border-radius: 9px;
  font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
  color: #111827;
  background: #fff;
  outline: none;
  transition: border-color 0.15s, box-shadow 0.15s;
}
.input:focus {
  border-color: #1677ff;
  box-shadow: 0 0 0 3px rgba(22, 119, 255, 0.14);
}
.input:disabled {
  background: #f7f8fa;
  color: #9aa3b2;
}
.append {
  position: relative;
}
.append .input {
  padding-right: 58px;
}
.eye {
  position: absolute;
  right: 7px;
  top: 50%;
  transform: translateY(-50%);
  border: none;
  background: transparent;
  color: #1677ff;
  font-size: 12px;
  cursor: pointer;
  padding: 5px 7px;
  border-radius: 6px;
}
.eye:hover {
  background: #f0f7ff;
}
.link {
  border: none;
  background: none;
  color: #1677ff;
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.link:hover {
  color: #0958d9;
  text-decoration: underline;
}
.sep {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 18px 0 2px;
  font-size: 12px;
  color: #98a1b0;
}
.sep::before,
.sep::after {
  content: "";
  flex: 1;
  height: 1px;
  background: #f0f1f3;
}
.hint {
  margin: 6px 2px 0;
  font-size: 12px;
  color: #9ca3af;
  line-height: 1.55;
}
.hint.edited {
  color: #1677ff;
}
.select-wrap {
  position: relative;
}
.select-trigger {
  width: 100%;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 12px;
  border: 1px solid #d7dce4;
  border-radius: 9px;
  background: #fff;
  color: #111827;
  cursor: pointer;
  font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
  text-align: left;
}
.select-trigger[aria-expanded="true"] {
  border-color: #1677ff;
  box-shadow: 0 0 0 3px rgba(22, 119, 255, 0.14);
}
.select-trigger svg {
  flex: 0 0 auto;
  fill: none;
  stroke: currentColor;
  stroke-width: 2.2;
  stroke-linecap: round;
  stroke-linejoin: round;
  color: #98a1b0;
  transition: transform 0.15s;
}
.select-trigger[aria-expanded="true"] svg {
  transform: rotate(180deg);
}
.select-menu {
  position: absolute;
  z-index: 20;
  left: 0;
  right: 0;
  top: calc(100% + 4px);
  display: flex;
  flex-direction: column;
  padding: 6px;
  background: #fff;
  border: 1px solid #e5e7eb;
  border-radius: 10px;
  box-shadow: 0 14px 36px rgba(15, 23, 42, 0.2);
  max-height: 260px;
  overflow-y: auto;
}
.option {
  width: 100%;
  text-align: left;
  padding: 9px 10px;
  border: none;
  border-radius: 7px;
  background: transparent;
  font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
  color: #1f2937;
  cursor: pointer;
}
.option:hover {
  background: #f3f4f6;
}
.option.selected {
  color: #1677ff;
  font-weight: 600;
  background: #eff6ff;
}
.custom-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 8px;
}
.unit {
  font-size: 13px;
  color: #6b7280;
}
.switch-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding-top: 4px;
}
.switch-row .hint {
  margin: 4px 2px 0 0;
}
.switch {
  flex: 0 0 auto;
  width: 44px;
  height: 24px;
  border: none;
  border-radius: 999px;
  background: #d1d5db;
  cursor: pointer;
  padding: 0;
  position: relative;
  transition: background 0.18s;
}
.switch i {
  position: absolute;
  top: 3px;
  left: 3px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.22);
  transition: left 0.18s;
}
.switch.on {
  background: #1677ff;
}
.switch.on i {
  left: 23px;
}
.switch:disabled {
  opacity: 0.6;
}
.foot {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 12px 20px 16px;
  border-top: 1px solid #f0f1f3;
}
.btn {
  border: 1px solid transparent;
  border-radius: 9px;
  padding: 9px 15px;
  font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
  cursor: pointer;
  transition: background 0.15s, filter 0.15s, opacity 0.15s;
}
.btn:disabled {
  opacity: 0.6;
  cursor: default;
}
.btn.primary {
  background: #1677ff;
  color: #fff;
}
.btn.primary:hover:not(:disabled) {
  background: #0958d9;
}
.btn.ghost {
  background: #f3f4f6;
  color: #374151;
}
.btn.ghost:hover:not(:disabled) {
  background: #e5e7eb;
}
.btn.danger {
  background: transparent;
  color: #dc2626;
  border-color: #fecaca;
  padding-left: 10px;
  padding-right: 10px;
}
.btn.danger:hover:not(:disabled) {
  background: #fef2f2;
}
.flex {
  flex: 1;
}
.toast {
  position: fixed;
  left: 50%;
  top: 18px;
  transform: translateX(-50%);
  z-index: 100;
  max-width: 82vw;
  background: #1f2937;
  color: #fff;
  font-size: 13px;
  padding: 9px 18px;
  border-radius: 999px;
  box-shadow: 0 6px 20px rgba(0, 0, 0, 0.2);
}
.toast.success {
  background: #16a34a;
}
.toast.error {
  background: #dc2626;
}
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.18s, transform 0.18s;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translate(-50%, -8px);
}
@media (prefers-reduced-motion: reduce) {
  .select-trigger svg,
  .switch,
  .switch i {
    transition: none;
  }
}
</style>

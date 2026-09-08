// ==UserScript==
// @name         校园网自动登录助手（稳定修复版）
// @namespace    campus-login-helper
// @version      2.0.0
// @description  自动填充账号密码与验证码；内置设置面板，可查看/修改账号、密码、验证码及其有效期；验证码有效期内可重复使用
// @match        *://172.26.255.2/*
// @match        *://172.26.255.3/*
// @grant        GM_getValue
// @grant        GM_setValue
// ==/UserScript==

(function () {
    'use strict';

    // ===== 常量 =====
    const PERMANENT = -1;                       // 永久有效的标记
    const PRESET_HOURS = [1, 3, 9, 18, 27];     // 下拉预设时长（小时）
    const DEFAULT_HOURS = 27;                   // 默认时长（小时）
    const EXPIRE_OPTIONS = PRESET_HOURS
        .map((h) => ({ value: String(h), label: h + " 小时" }))
        .concat([
            { value: "permanent", label: "永久" },
            { value: "custom", label: "自定义…" }
        ]);

    // ===== 数据存取 =====
    const getAccount = () => GM_getValue("campus_account", null);
    const setAccount = (v) => GM_setValue("campus_account", v);

    const getCaptcha = () => GM_getValue("campus_captcha", null);
    const setCaptcha = (v) => GM_setValue("campus_captcha", v);

    const getExpireHours = () => GM_getValue("campus_captcha_hours", DEFAULT_HOURS);
    const setExpireHours = (v) => GM_setValue("campus_captcha_hours", v);

    // ===== 验证码有效期判断 =====
    function captchaHoursOf(cap) {
        if (!cap) return null;
        if (cap.hours === PERMANENT || cap.hours === "permanent") return PERMANENT;
        if (typeof cap.hours === "number" && cap.hours > 0) return cap.hours;
        return getExpireHours(); // 兼容旧版本未保存时长的数据
    }

    function captchaValid() {
        const cap = getCaptcha();
        if (!cap || !cap.code) return false;
        const hours = captchaHoursOf(cap);
        if (hours === PERMANENT) return true;
        if (typeof hours !== "number" || !(hours > 0)) return false;
        return Date.now() - cap.time < hours * 3600 * 1000;
    }

    function captchaStatusText() {
        const cap = getCaptcha();
        if (!cap || !cap.code) return "未设置";
        const hours = captchaHoursOf(cap);
        if (hours === PERMANENT) return "永久有效";
        if (typeof hours !== "number" || !(hours > 0)) return "配置异常";
        const remainMs = hours * 3600 * 1000 - (Date.now() - (cap.time || 0));
        if (remainMs <= 0) return "已过期";
        const remainH = remainMs / 3600 / 1000;
        if (remainH >= 24) return "有效，剩余约 " + (remainH / 24).toFixed(1) + " 天";
        if (remainH >= 1) return "有效，剩余约 " + remainH.toFixed(1) + " 小时";
        const remainM = Math.max(1, Math.round(remainMs / 60000));
        return "有效，剩余约 " + remainM + " 分钟";
    }

    // ===== 界面样式 =====
    const UI_CSS = `
.cuh-overlay{
    position: fixed; top: 0; left: 0; right: 0; bottom: 0;
    z-index: 2147483646; display: flex; align-items: center; justify-content: center;
    background: rgba(15,23,42,.55);
    font-family: "Microsoft YaHei","PingFang SC",system-ui,-apple-system,sans-serif;
}
.cuh-dialog{
    display: flex;
    flex-direction: column;
    width: min(92vw, 420px);
    max-height: 88vh;
    overflow: hidden;
    background: #ffffff;
    color: #1f2937;
    border-radius: 14px;
    box-shadow: 0 18px 50px rgba(0,0,0,.35);
    animation: cuh-pop .16s ease-out;
}
.cuh-dialog > .cuh-head,
.cuh-dialog > .cuh-foot{
    flex: 0 0 auto;
}
@keyframes cuh-pop{
    from{ opacity: 0; transform: scale(.96) translateY(8px); }
    to{ opacity: 1; transform: scale(1) translateY(0); }
}
.cuh-head{
    display: flex; align-items: center; justify-content: space-between;
    padding: 14px 18px 10px; border-bottom: 1px solid #eef0f3;
}
.cuh-title{ font-size: 16px; font-weight: 600; }
.cuh-icon-btn{
    border: none; background: transparent; color: #6b7280;
    font-size: 20px; line-height: 1; padding: 2px 6px; cursor: pointer; border-radius: 6px;
}
.cuh-icon-btn:hover{ background: #f3f4f6; color: #111827; }
.cuh-notice{
    margin: 12px 18px 0; padding: 8px 12px; border-radius: 8px;
    background: #fff7e6; color: #ad6800; font-size: 13px; line-height: 1.5;
}
.cuh-body{
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 18px 6px;
}
.cuh-field{ margin: 12px 0; }
.cuh-label{ display: block; font-size: 12px; color: #6b7280; margin-bottom: 6px; }
.cuh-input{
    box-sizing: border-box; width: 100%; padding: 9px 12px;
    border: 1px solid #d1d5db; border-radius: 8px;
    font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
    color: #111827; background: #fff; outline: none;
}
.cuh-input:focus{ border-color: #1677ff; box-shadow: 0 0 0 3px rgba(22,119,255,.15); }
.cuh-select-wrap{ position: relative; }
.cuh-select-trigger{
    box-sizing: border-box; width: 100%;
    display: flex; align-items: center; justify-content: space-between; gap: 10px;
    padding: 9px 12px; border: 1px solid #d1d5db; border-radius: 8px;
    background: #ffffff; color: #111827; cursor: pointer; outline: none;
    font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif; text-align: left;
}
.cuh-select-trigger:focus-visible,
.cuh-select-trigger[aria-expanded="true"]{
    border-color: #1677ff;
    box-shadow: 0 0 0 3px rgba(22,119,255,.15);
}
.cuh-select-chev{
    width: 14px; height: 14px; flex: 0 0 auto; color: #6b7280;
    transition: transform .15s ease-out;
}
.cuh-select-trigger[aria-expanded="true"] .cuh-select-chev{ transform: rotate(180deg); }
.cuh-select-menu{
    position: fixed; z-index: 30;
    display: flex; flex-direction: column; padding: 6px;
    background: #ffffff; border: 1px solid #e5e7eb; border-radius: 10px;
    box-shadow: 0 14px 36px rgba(15,23,42,.20);
    max-height: min(320px, calc(100vh - 24px));
    overflow-y: auto;
}
.cuh-expire-option{
    box-sizing: border-box; width: 100%;
    display: flex; align-items: center; justify-content: space-between; gap: 10px;
    padding: 8px 10px; border: none; border-radius: 7px; background: transparent;
    font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
    color: #1f2937; cursor: pointer; text-align: left;
}
.cuh-expire-option::after{
    content: ""; width: 6px; height: 6px; border-radius: 50%;
    background: transparent; flex: 0 0 auto;
}
.cuh-expire-option:hover{ background: #f3f4f6; }
.cuh-expire-option:focus-visible{
    outline: 2px solid #1677ff; outline-offset: -2px; background: #f0f7ff;
}
.cuh-expire-option[aria-selected="true"]{
    color: #1677ff; font-weight: 600; background: #eff6ff;
}
.cuh-expire-option[aria-selected="true"]::after{ background: #1677ff; }
@media (prefers-reduced-motion: reduce){
    .cuh-select-chev{ transition: none; }
}
.cuh-append{ position: relative; }
.cuh-append .cuh-input{ padding-right: 58px; }
.cuh-eye{
    position: absolute; right: 6px; top: 50%; transform: translateY(-50%);
    border: none; background: transparent; color: #1677ff;
    font-size: 12px; cursor: pointer; padding: 4px 6px;
}
.cuh-custom{ display: flex; align-items: center; gap: 8px; margin-top: 8px; }
.cuh-custom .cuh-input{ flex: 1; }
.cuh-unit{ font-size: 13px; color: #6b7280; }
.cuh-hint{ font-size: 12px; color: #9ca3af; margin-top: 6px; line-height: 1.6; }
.cuh-link{
    border: none; background: none; color: #1677ff; font-size: 12px;
    cursor: pointer; padding: 0; margin-left: 8px;
}
.cuh-link:hover{ color: #0958d9; text-decoration: underline; }
.cuh-sep{
    display: flex; align-items: center; gap: 10px; margin: 16px 0 2px;
    font-size: 12px; color: #9ca3af;
}
.cuh-sep::before,.cuh-sep::after{
    content: ""; flex: 1; height: 1px; background: #f0f1f3;
}
.cuh-foot{
    display: flex; align-items: center; gap: 8px;
    padding: 12px 18px 16px; border-top: 1px solid #f0f1f3;
}
.cuh-btn{
    border: 1px solid transparent; border-radius: 8px;
    padding: 8px 14px; font: 14px/1.4 "Microsoft YaHei", system-ui, sans-serif;
    cursor: pointer; transition: background .15s, filter .15s;
}
.cuh-btn:active{ transform: translateY(1px); }
.cuh-btn-primary{ background: #1677ff; color: #fff; }
.cuh-btn-primary:hover{ background: #0958d9; }
.cuh-btn-ghost{ background: #f3f4f6; color: #374151; }
.cuh-btn-ghost:hover{ background: #e5e7eb; }
.cuh-btn-danger{ background: transparent; color: #dc2626; border-color: #fecaca; }
.cuh-btn-danger:hover{ background: #fef2f2; }
.cuh-flex{ flex: 1; }
.cuh-toast{
    position: fixed; left: 50%; top: 24px; transform: translate(-50%, -16px);
    z-index: 2147483647; max-width: 82vw; box-sizing: border-box;
    background: #1677ff; color: #fff; font: 13px/1.5 "Microsoft YaHei", system-ui, sans-serif;
    padding: 9px 18px; border-radius: 999px; box-shadow: 0 6px 20px rgba(0,0,0,.25);
    opacity: 0; pointer-events: none; transition: opacity .18s, transform .18s;
    word-break: break-all;
}
.cuh-toast.show{ opacity: 1; transform: translate(-50%, 0); }
.cuh-loading{
    position: fixed; top: 20px; left: 50%; transform: translateX(-50%);
    z-index: 2147483647; display: flex; align-items: center; gap: 10px;
    max-width: 90vw; box-sizing: border-box;
    background: rgba(17,24,39,.92); color: #ffffff;
    font: 13px/1.4 "Microsoft YaHei", system-ui, sans-serif;
    padding: 10px 18px; border-radius: 999px;
    box-shadow: 0 8px 24px rgba(0,0,0,.28);
}
.cuh-loading-spinner{
    width: 14px; height: 14px; flex: 0 0 auto; border-radius: 50%;
    border: 2px solid rgba(255,255,255,.35); border-top-color: #ffffff;
    animation: cuh-spin .7s linear infinite;
}
@keyframes cuh-spin{
    to{ transform: rotate(360deg); }
}
@media (prefers-reduced-motion: reduce){
    .cuh-loading-spinner{ animation-duration: 1.8s; }
}
.cuh-banner{
    position: fixed; left: 50%; bottom: 110px; transform: translateX(-50%);
    z-index: 2147483645; width: min(92vw, 560px); box-sizing: border-box;
    background: #fff1f0; border: 1px solid #ffccc7; border-radius: 12px;
    box-shadow: 0 10px 30px rgba(0,0,0,.18);
    font-family: "Microsoft YaHei","PingFang SC",system-ui,sans-serif;
    padding: 12px 14px; display: none;
}
.cuh-banner-top{ display: flex; align-items: flex-start; gap: 10px; }
.cuh-banner-msg{
    flex: 1; color: #cf1322; font-size: 13px; line-height: 1.6;
    max-height: 120px; overflow: auto; white-space: pre-wrap; word-break: break-all;
}
.cuh-banner-note{ margin-top: 6px; color: #fa8c16; font-size: 12px; }
.cuh-banner-actions{ display: flex; gap: 8px; margin-top: 10px; }
`;

    // 悬浮球样式独立注入：不依赖设置弹窗的 UI，保证任何时候都能正常显示
    const FAB_CSS = `
#cuh-fab{
    -webkit-tap-highlight-color: transparent;
    transition: transform .16s cubic-bezier(.2,.8,.2,1), box-shadow .16s ease-out;
}
#cuh-fab:hover{
    transform: translateY(-1px);
    box-shadow: 0 12px 28px rgba(15,23,42,.20), 0 2px 6px rgba(15,23,42,.10);
}
#cuh-fab:active{
    transform: translateY(0) scale(.97);
}
#cuh-fab:focus-visible{
    outline: 2px solid #1677ff;
    outline-offset: 3px;
}
.cuh-fab-ic{
    width: 28px; height: 28px; flex: 0 0 auto;
    display: flex; align-items: center; justify-content: center;
    background: #1677ff; color: #fff; border-radius: 50%;
    box-shadow: 0 2px 6px rgba(22,119,255,.32);
}
.cuh-fab-label{
    white-space: nowrap;
    color: #111827;
    font-weight: 600;
}
.cuh-fab-dot{
    width: 8px; height: 8px; flex: 0 0 auto; border-radius: 50%;
    background: #9ca3af;
    box-shadow: 0 0 0 2px #ffffff;
}
.cuh-fab-dot[data-state="ready"]{ background: #16a34a; }
.cuh-fab-dot[data-state="warn"]{ background: #f59e0b; }
@media (prefers-reduced-motion: reduce){
    #cuh-fab{ transition: none; }
}
`;

    // ===== 界面引用与基础操作 =====
    const ui = {};
    let toastHideTimer = null;
    let modalOpen = false;
    let modalResolve = null;
    let modalPromise = null;
    let modalOpts = {};
    let loginInProgress = false;
    let expireChoice = String(DEFAULT_HOURS);

    function $(id) { return document.getElementById(id); }

    function buildDialogHTML() {
        return `
            <div class="cuh-dialog" role="dialog" aria-modal="true" aria-label="校园网自动登录设置"
                 style="display:flex; flex-direction:column; background-color:#ffffff;">
                <div class="cuh-head">
                    <div class="cuh-title" id="cuh-title">校园网自动登录设置</div>
                    <button type="button" class="cuh-icon-btn" id="cuh-close" title="关闭">&times;</button>
                </div>
                <div class="cuh-notice" id="cuh-notice" style="display:none"></div>
                <div class="cuh-body">
                    <div class="cuh-field">
                        <label class="cuh-label" for="cuh-acc">校园网账号</label>
                        <input id="cuh-acc" class="cuh-input" autocomplete="off" spellcheck="false" placeholder="请输入账号">
                    </div>
                    <div class="cuh-field">
                        <label class="cuh-label" for="cuh-pwd">密码</label>
                        <div class="cuh-append">
                            <input id="cuh-pwd" class="cuh-input" type="password" autocomplete="new-password" placeholder="请输入密码">
                            <button type="button" class="cuh-eye" id="cuh-eye">显示</button>
                        </div>
                    </div>
                    <div class="cuh-sep">验证码</div>
                    <div class="cuh-field">
                        <label class="cuh-label" for="cuh-cap">
                            验证码
                            <button type="button" class="cuh-link" id="cuh-cap-clear">清除验证码</button>
                        </label>
                        <input id="cuh-cap" class="cuh-input" autocomplete="off" spellcheck="false" placeholder="请输入当前页面验证码">
                        <div class="cuh-hint" id="cuh-cap-status"></div>
                    </div>
                    <div class="cuh-field">
                        <label class="cuh-label" for="cuh-expire-trigger">验证码有效期</label>
                        <div class="cuh-select-wrap" id="cuh-expire-wrap">
                            <button type="button" class="cuh-select-trigger" id="cuh-expire-trigger"
                                    aria-haspopup="listbox" aria-expanded="false">
                                <span id="cuh-expire-label">27 小时</span>
                                <svg class="cuh-select-chev" viewBox="0 0 24 24" width="14" height="14"
                                     fill="none" stroke="currentColor" stroke-width="2.2"
                                     stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                                    <path d="m6 9 6 6 6-6"></path>
                                </svg>
                            </button>
                            <div class="cuh-select-menu" id="cuh-expire-menu" role="listbox"
                                 aria-label="验证码有效期" style="display:none">
                                ${EXPIRE_OPTIONS.map((o) =>
                                    `<button type="button" class="cuh-expire-option" role="option"
                                             aria-selected="false" data-value="${o.value}">${o.label}</button>`
                                ).join('')}
                            </div>
                        </div>
                        <div class="cuh-custom" id="cuh-custom-row" style="display:none">
                            <input id="cuh-expire-custom" class="cuh-input" type="number" min="0.1" step="0.5" placeholder="小时数，如 6">
                            <span class="cuh-unit">小时</span>
                        </div>
                        <div class="cuh-hint">验证码保存后开始计时；有效期内可重复使用，登录失败不会自动清除</div>
                    </div>
                </div>
                <div class="cuh-foot">
                    <button type="button" class="cuh-btn cuh-btn-danger" id="cuh-clear">清空数据</button>
                    <span class="cuh-flex"></span>
                    <button type="button" class="cuh-btn cuh-btn-ghost" id="cuh-cancel">关闭</button>
                    <button type="button" class="cuh-btn cuh-btn-primary" id="cuh-save">保存</button>
                </div>
            </div>`;
    }

    function buildBannerHTML() {
        return `
            <div class="cuh-banner-top">
                <button type="button" class="cuh-icon-btn" id="cuh-banner-close" title="关闭">&times;</button>
                <div style="flex:1">
                    <div class="cuh-banner-msg" id="cuh-banner-msg"></div>
                    <div class="cuh-banner-note" id="cuh-banner-note"></div>
                </div>
            </div>
            <div class="cuh-banner-actions">
                <button type="button" class="cuh-btn cuh-btn-primary" id="cuh-banner-retry">重试登录</button>
                <button type="button" class="cuh-btn cuh-btn-ghost" id="cuh-banner-settings">修改设置</button>
            </div>`;
    }

    function ensureUi() {
        if (ui.ready) return;
        ui.ready = true;

        const style = document.createElement("style");
        style.textContent = UI_CSS;
        (document.head || document.documentElement).appendChild(style);

        const overlay = document.createElement("div");
        overlay.className = "cuh-overlay";
        overlay.id = "cuh-overlay";
        overlay.style.display = "none";
        overlay.innerHTML = buildDialogHTML();
        document.documentElement.appendChild(overlay);

        const toast = document.createElement("div");
        toast.id = "cuh-toast";
        toast.className = "cuh-toast";
        document.documentElement.appendChild(toast);

        const loading = document.createElement("div");
        loading.id = "cuh-loading";
        loading.className = "cuh-loading";
        loading.style.display = "none";
        loading.innerHTML =
            '<span class="cuh-loading-spinner" aria-hidden="true"></span>' +
            '<span id="cuh-loading-text">正在填入验证码…</span>';
        document.documentElement.appendChild(loading);

        const banner = document.createElement("div");
        banner.className = "cuh-banner";
        banner.id = "cuh-banner";
        banner.innerHTML = buildBannerHTML();
        document.documentElement.appendChild(banner);

        ui.overlay = $("cuh-overlay");
        ui.title = $("cuh-title");
        ui.notice = $("cuh-notice");
        ui.closeBtn = $("cuh-close");
        ui.accInput = $("cuh-acc");
        ui.pwdInput = $("cuh-pwd");
        ui.eyeBtn = $("cuh-eye");
        ui.capInput = $("cuh-cap");
        ui.capStatus = $("cuh-cap-status");
        ui.capClearBtn = $("cuh-cap-clear");
        ui.expireWrap = $("cuh-expire-wrap");
        ui.expireTrigger = $("cuh-expire-trigger");
        ui.expireLabel = $("cuh-expire-label");
        ui.expireMenu = $("cuh-expire-menu");
        ui.expireOptions = Array.from(ui.expireMenu.querySelectorAll(".cuh-expire-option"));
        ui.expireCustom = $("cuh-expire-custom");
        ui.customRow = $("cuh-custom-row");
        ui.clearBtn = $("cuh-clear");
        ui.cancelBtn = $("cuh-cancel");
        ui.saveBtn = $("cuh-save");
        ui.toast = $("cuh-toast");
        ui.loading = $("cuh-loading");
        ui.loadingText = $("cuh-loading-text");
        ui.banner = $("cuh-banner");
        ui.bannerMsg = $("cuh-banner-msg");
        ui.bannerNote = $("cuh-banner-note");
        ui.bannerRetry = $("cuh-banner-retry");
        ui.bannerSettings = $("cuh-banner-settings");
        ui.bannerClose = $("cuh-banner-close");

        bindUiEvents();
    }

    // ===== 轻提示 =====
    function showToast(text, type, duration) {
        ensureUi();
        const t = ui.toast;
        t.textContent = text;
        t.style.background = type === "success" ? "#16a34a"
            : type === "error" ? "#dc2626"
            : "#1677ff";
        t.className = "cuh-toast show";
        clearTimeout(toastHideTimer);
        toastHideTimer = setTimeout(() => {
            t.className = "cuh-toast";
        }, duration || 2600);
    }

    // ===== 页面加载/自动填入提示 =====
    function showPageLoading(text) {
        ensureUi();
        ui.loadingText.textContent = text || "正在填入验证码…";
        ui.loading.style.display = "flex";
    }

    function hidePageLoading() {
        if (ui.loading) ui.loading.style.display = "none";
    }

    // ===== 登录失败提示条 =====
    function showFailureBanner(text) {
        ensureUi();
        ui.bannerMsg.textContent = text || "未知错误";
        ui.bannerNote.textContent = "验证码已保留，有效期内可继续复用；可在“修改设置”中修正账号密码后自动重试";
        ui.banner.style.display = "block";
    }

    function hideFailureBanner() {
        ensureUi();
        ui.banner.style.display = "none";
    }

    // ===== 设置弹窗 =====
    function updateCaptchaStatus() {
        ui.capStatus.textContent = "当前验证码：" + captchaStatusText();
    }

    function expireLabelFor(value) {
        const opt = EXPIRE_OPTIONS.find((o) => o.value === value);
        return opt ? opt.label : "自定义…";
    }

    function syncExpireSelection() {
        if (!ui.expireOptions) return;
        ui.expireOptions.forEach((opt) => {
            opt.setAttribute("aria-selected", String(opt.dataset.value === expireChoice));
        });
    }

    function applyExpireChoice(value, opts) {
        expireChoice = value;
        if (!ui.ready) return;
        ui.expireLabel.textContent = expireLabelFor(value);
        if (value === "custom") {
            ui.customRow.style.display = "flex";
            if (opts && opts.focus) {
                setTimeout(() => { if (modalOpen) ui.expireCustom.focus(); }, 0);
            }
        } else {
            ui.customRow.style.display = "none";
        }
        syncExpireSelection();
    }

    function openExpireMenu() {
        if (!ui.expireMenu) return;
        ui.expireMenu.style.display = "block";
        ui.expireTrigger.setAttribute("aria-expanded", "true");

        const rect = ui.expireTrigger.getBoundingClientRect();
        const menuH = ui.expireMenu.offsetHeight;
        let top = rect.bottom + 6;
        if (top + menuH > window.innerHeight - 12) {
            top = Math.max(12, rect.top - menuH - 6);
        }
        ui.expireMenu.style.left = rect.left + "px";
        ui.expireMenu.style.top = top + "px";
        ui.expireMenu.style.width = rect.width + "px";
        syncExpireSelection();
    }

    function closeExpireMenu() {
        if (!ui.expireMenu) return;
        ui.expireMenu.style.display = "none";
        if (ui.expireTrigger) ui.expireTrigger.setAttribute("aria-expanded", "false");
    }

    function toggleExpireMenu() {
        if (ui.expireMenu && ui.expireMenu.style.display === "block") {
            closeExpireMenu();
        } else {
            openExpireMenu();
        }
    }

    function chooseExpireOption(value) {
        applyExpireChoice(value, { focus: value === "custom" });
        closeExpireMenu();
        if (value !== "custom") ui.expireTrigger.focus();
    }

    function focusExpireOptionAt(index) {
        const opts = ui.expireOptions || [];
        if (opts[index]) opts[index].focus();
    }

    function resetExpireControls() {
        const hours = getExpireHours();
        let value;
        if (hours === PERMANENT) {
            value = "permanent";
        } else if (typeof hours === "number" && hours > 0 && PRESET_HOURS.indexOf(hours) >= 0) {
            value = String(hours);
        } else if (typeof hours === "number" && hours > 0) {
            value = "custom";
            ui.expireCustom.value = hours;
        } else {
            value = String(DEFAULT_HOURS);
        }
        expireChoice = value;
        if (ui.ready) {
            ui.expireLabel.textContent = expireLabelFor(value);
            ui.customRow.style.display = value === "custom" ? "flex" : "none";
            closeExpireMenu();
            syncExpireSelection();
        }
    }

    function readExpireHoursFromForm() {
        const v = expireChoice;
        if (v === "permanent") return PERMANENT;
        if (v === "custom") {
            const n = parseFloat(ui.expireCustom.value);
            return (Number.isFinite(n) && n > 0) ? n : null;
        }
        const n = parseFloat(v);
        return (Number.isFinite(n) && n > 0) ? n : DEFAULT_HOURS;
    }

    function resetClearBtn() {
        ui.clearBtn.dataset.armed = "0";
        ui.clearBtn.textContent = "清空数据";
    }

    function openSettings(opts) {
        ensureUi();
        if (modalOpen) return modalPromise;

        opts = opts || {};
        modalOpts = opts;
        modalOpen = true;

        ui.title.textContent = opts.title || "校园网自动登录设置";
        if (opts.notice) {
            ui.notice.style.display = "block";
            ui.notice.textContent = opts.notice;
        } else {
            ui.notice.style.display = "none";
        }

        const accData = getAccount();
        ui.accInput.value = accData ? accData.acc : "";
        ui.pwdInput.value = accData ? accData.pwd : "";

        const cap = getCaptcha();
        ui.capInput.value = opts.blankCaptcha ? "" : (cap && cap.code) || "";
        ui.pwdInput.type = "password";
        ui.eyeBtn.textContent = "显示";
        updateCaptchaStatus();
        resetExpireControls();
        resetClearBtn();

        ui.overlay.style.display = "flex";
        const target = opts.focus === "cap" ? ui.capInput
            : opts.focus === "pwd" ? ui.pwdInput
            : ui.accInput;
        setTimeout(() => {
            if (modalOpen) target.focus();
        }, 80);

        modalPromise = new Promise((resolve) => { modalResolve = resolve; });
        return modalPromise;
    }

    function closeSettings(saved) {
        if (!modalOpen) return;
        modalOpen = false;
        closeExpireMenu();
        ui.overlay.style.display = "none";
        const resolve = modalResolve;
        modalResolve = null;
        modalPromise = null;
        if (resolve) resolve(saved);
    }

    function onSaveClick() {
        const acc = ui.accInput.value.trim();
        const pwd = ui.pwdInput.value;
        const code = ui.capInput.value.trim();
        const hours = readExpireHoursFromForm();

        if (hours === null) {
            showToast("自定义时长请输入大于 0 的小时数", "error");
            ui.expireCustom.focus();
            return;
        }

        if (modalOpts.requireAccount) {
            if (!acc || !pwd) {
                showToast("请填写校园网账号和密码", "error");
                (acc ? ui.pwdInput : ui.accInput).focus();
                return;
            }
        } else if (acc || pwd) {
            if (!acc || !pwd) {
                showToast("账号与密码需同时填写，留空则不修改", "error");
                return;
            }
        }

        const savedParts = [];
        if (acc && pwd) {
            setAccount({ acc, pwd });
            savedParts.push("账号密码");
        }
        if (code) {
            setCaptcha({ code, time: Date.now(), hours });
            savedParts.push("验证码");
        }
        setExpireHours(hours);
        savedParts.push("有效期设置");

        // 若仍停留在登录页，立即把新值同步到表单
        refreshFormFieldsFromStorage();

        const opts = modalOpts;
        closeSettings(true);
        showToast(savedParts.join("、") + "已保存", "success");
        updateFabStatus();
        if (typeof opts.onSaved === "function") {
            setTimeout(opts.onSaved, 300);
        }
    }

    function onClearClick() {
        if (ui.clearBtn.dataset.armed !== "1") {
            ui.clearBtn.dataset.armed = "1";
            ui.clearBtn.textContent = "再点一次确认清空";
            setTimeout(resetClearBtn, 3000);
            return;
        }
        setAccount(null);
        setCaptcha(null);
        setExpireHours(DEFAULT_HOURS);
        ui.accInput.value = "";
        ui.pwdInput.value = "";
        ui.capInput.value = "";
        updateCaptchaStatus();
        resetExpireControls();
        updateFabStatus();
        hideFailureBanner();
        showToast("账号、密码、验证码等数据已全部清空", "success");
        closeSettings(false);
    }

    function bindUiEvents() {
        ui.closeBtn.addEventListener("click", () => closeSettings(false));
        ui.cancelBtn.addEventListener("click", () => closeSettings(false));
        ui.saveBtn.addEventListener("click", onSaveClick);
        ui.clearBtn.addEventListener("click", onClearClick);

        ui.eyeBtn.addEventListener("click", () => {
            const show = ui.pwdInput.type === "password";
            ui.pwdInput.type = show ? "text" : "password";
            ui.eyeBtn.textContent = show ? "隐藏" : "显示";
        });

        ui.expireTrigger.addEventListener("click", (e) => {
            e.stopPropagation();
            toggleExpireMenu();
        });

        ui.expireTrigger.addEventListener("keydown", (e) => {
            const menuOpen = ui.expireMenu.style.display === "block";
            if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                if (menuOpen) {
                    closeExpireMenu();
                } else {
                    openExpireMenu();
                    const idx = Math.max(0, ui.expireOptions.findIndex((o) => o.dataset.value === expireChoice));
                    focusExpireOptionAt(idx);
                }
            } else if (e.key === "ArrowDown") {
                e.preventDefault();
                openExpireMenu();
                const idx = Math.max(0, ui.expireOptions.findIndex((o) => o.dataset.value === expireChoice));
                focusExpireOptionAt(idx);
            } else if (e.key === "ArrowUp") {
                e.preventDefault();
                openExpireMenu();
                focusExpireOptionAt(ui.expireOptions.length - 1);
            }
        });

        ui.expireMenu.addEventListener("click", (e) => {
            const opt = e.target.closest(".cuh-expire-option");
            if (!opt) return;
            e.stopPropagation();
            chooseExpireOption(opt.dataset.value);
        });

        ui.expireMenu.addEventListener("keydown", (e) => {
            const opts = ui.expireOptions || [];
            if (opts.length === 0) return;
            let idx = opts.indexOf(document.activeElement);
            if (e.key === "ArrowDown") {
                idx = (idx + 1) % opts.length;
                e.preventDefault();
                focusExpireOptionAt(idx);
            } else if (e.key === "ArrowUp") {
                idx = idx <= 0 ? opts.length - 1 : idx - 1;
                e.preventDefault();
                focusExpireOptionAt(idx);
            } else if (e.key === "Home") {
                idx = 0;
                e.preventDefault();
                focusExpireOptionAt(idx);
            } else if (e.key === "End") {
                idx = opts.length - 1;
                e.preventDefault();
                focusExpireOptionAt(idx);
            } else if (e.key === "Enter" || e.key === " ") {
                const cur = opts[idx];
                if (cur) {
                    e.preventDefault();
                    chooseExpireOption(cur.dataset.value);
                }
            }
        });

        document.addEventListener("mousedown", (e) => {
            if (ui.expireMenu && ui.expireMenu.style.display === "block" && !ui.expireWrap.contains(e.target)) {
                closeExpireMenu();
            }
        });

        ui.capClearBtn.addEventListener("click", () => {
            setCaptcha(null);
            ui.capInput.value = "";
            updateCaptchaStatus();
            updateFabStatus();
            showToast("验证码已清除", "info");
        });

        ui.capInput.addEventListener("input", () => {
            const cur = getCaptcha();
            const val = ui.capInput.value.trim();
            if (val && (!cur || val !== cur.code)) {
                ui.capStatus.textContent = "已输入新验证码，保存后开始计时";
            } else {
                updateCaptchaStatus();
            }
        });

        ui.overlay.addEventListener("mousedown", (e) => {
            if (e.target === ui.overlay) closeSettings(false);
        });

        document.addEventListener("keydown", (e) => {
            if (e.key === "Escape") {
                if (ui.expireMenu && ui.expireMenu.style.display === "block") {
                    closeExpireMenu();
                    ui.expireTrigger.focus();
                    return;
                }
                if (modalOpen) closeSettings(false);
            }
        });

        ui.bannerRetry.addEventListener("click", retryNow);
        ui.bannerSettings.addEventListener("click", () => {
            openSettings({
                title: "修改账号 / 验证码",
                notice: "保存后将使用新账号密码自动重新登录",
                focus: "pwd",
                onSaved: retryNow
            });
        });
        ui.bannerClose.addEventListener("click", hideFailureBanner);
    }

    // ===== 等待元素 =====
    function waitFor(selector, timeout) {
        timeout = timeout || 10000;
        return new Promise((resolve, reject) => {
            const start = Date.now();
            const timer = setInterval(() => {
                const el = document.querySelector(selector);
                if (el) {
                    clearInterval(timer);
                    resolve(el);
                    return;
                }
                if (Date.now() - start > timeout) {
                    clearInterval(timer);
                    reject("timeout");
                }
            }, 200);
        });
    }

    // ===== 悬浮按钮 =====
    function fabStatus() {
        const accData = getAccount();
        if (!accData || !accData.acc || !accData.pwd) {
            return { state: "none", text: "尚未设置账号，点击打开设置" };
        }
        if (captchaValid()) {
            return { state: "ready", text: "验证码有效，点击打开设置" };
        }
        return { state: "warn", text: "验证码缺失或已过期，点击打开设置" };
    }

    function updateFabStatus() {
        const fab = $("cuh-fab");
        if (!fab) return;
        const s = fabStatus();
        const dot = fab.querySelector(".cuh-fab-dot");
        if (dot) dot.dataset.state = s.state;
        fab.title = s.text;
        fab.setAttribute("aria-label", s.text);
    }

    function ensureFabStyle() {
        if ($("cuh-fab-style")) return;
        const st = document.createElement("style");
        st.id = "cuh-fab-style";
        st.textContent = FAB_CSS;
        (document.head || document.documentElement).appendChild(st);
    }

    function createFab() {
        if ($("cuh-fab")) return;

        ensureFabStyle();

        const fab = document.createElement("div");
        fab.id = "cuh-fab";
        fab.setAttribute("role", "button");
        fab.tabIndex = 0;
        fab.innerHTML = `
            <span class="cuh-fab-ic" aria-hidden="true">
                <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M5 12.55a11 11 0 0 1 14.08 0"></path>
                    <path d="M8.53 16.11a6 6 0 0 1 6.95 0"></path>
                    <line x1="12" y1="20" x2="12.01" y2="20"></line>
                </svg>
            </span>
            <span class="cuh-fab-label">校园网</span>
            <span class="cuh-fab-dot" data-state="none"></span>
        `;

        // 兜底内联样式：即使页面样式表或 CSP 干扰，悬浮球仍保持可见可点
        fab.style.cssText = `
            position: fixed;
            right: 20px;
            bottom: 80px;
            z-index: 2147483647;
            display: flex;
            align-items: center;
            gap: 9px;
            padding: 7px 15px 7px 8px;
            border-radius: 999px;
            background: #ffffff;
            color: #111827;
            border: 1px solid rgba(15, 23, 42, 0.10);
            box-shadow: 0 6px 20px rgba(15, 23, 42, 0.16), 0 1px 3px rgba(15, 23, 42, 0.08);
            cursor: pointer;
            user-select: none;
            font: 600 13px/1.4 "Microsoft YaHei", system-ui, sans-serif;
            box-sizing: border-box;
        `;

        const openPanel = () => {
            openSettings({ title: "校园网自动登录设置" });
        };

        fab.addEventListener("click", (e) => {
            e.preventDefault();
            e.stopPropagation();
            openPanel();
        });
        fab.addEventListener("keydown", (e) => {
            if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                openPanel();
            }
        });

        document.documentElement.appendChild(fab);
        updateFabStatus();
        setInterval(updateFabStatus, 60000);
    }

    // ===== 自动填充表单 =====
    function dispatchInput(el) {
        el.dispatchEvent(new Event("input", { bubbles: true }));
    }

    function fillCredentialFields() {
        const accData = getAccount();
        if (!accData || !accData.acc || !accData.pwd) return false;

        const accInput = document.querySelector("#f1_div form input:nth-of-type(3)");
        const pwdInput = document.querySelector("#f1_div form input:nth-of-type(4)");
        if (!accInput || !pwdInput) {
            console.log("未找到账号/密码输入框");
            return false;
        }

        accInput.value = accData.acc;
        pwdInput.value = accData.pwd;
        dispatchInput(accInput);
        dispatchInput(pwdInput);

        // 运营商固定：中国电信
        const select = document.querySelector("#f1_div select[name='ISP_select']");
        if (select) {
            select.value = "2";
            select.dispatchEvent(new Event("change", { bubbles: true }));
        }
        return true;
    }

    function fillCaptchaField() {
        const cap = getCaptcha();
        const capInput = document.querySelector("#dynPass");
        if (!capInput) {
            // 检测不到验证码输入框：只关闭加载提示，流程保持原样
            hidePageLoading();
            return false;
        }
        if (!cap || !cap.code) return false;
        capInput.value = cap.code;
        dispatchInput(capInput);
        return true;
    }

    function refreshFormFieldsFromStorage() {
        fillCredentialFields();
        fillCaptchaField();
    }

    // ===== 登录流程 =====
    function startLoginAttempt() {
        if (loginInProgress) return;
        loginInProgress = true;

        setTimeout(() => {
            const btn = document.querySelector("#login_btn");
            if (!btn) {
                loginInProgress = false;
                hidePageLoading();
                console.log("登录按钮不存在");
                showToast("未找到登录按钮", "error");
                return;
            }

            console.log("执行登录");
            btn.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
            btn.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
            btn.dispatchEvent(new MouseEvent("click", { bubbles: true }));
            btn.click();
            // 点击登录按钮后，加载提示使命完成
            hidePageLoading();

            // 轮询检测登录结果
            let count = 0;
            const timer = setInterval(() => {
                count++;
                const msg = document.querySelector("#message");
                console.log("检测登录结果...", count);

                if (msg && msg.innerText && msg.innerText.trim() !== "") {
                    clearInterval(timer);
                    loginInProgress = false;
                    const text = msg.innerText.trim();
                    console.log("登录失败：", text);
                    // 验证码在有效期内可重复使用，因此不自动清除
                    showFailureBanner(text);
                    return;
                }

                // 10 秒无失败信息 → 视为成功
                if (count >= 10) {
                    clearInterval(timer);
                    loginInProgress = false;
                    console.log("未检测到失败信息（可能登录成功）");
                    hideFailureBanner();
                    showToast("登录成功", "success");
                }
            }, 1000);
        }, 1200);
    }

    function retryNow() {
        hideFailureBanner();

        const accData = getAccount();
        if (!accData || !accData.acc || !accData.pwd) {
            openSettings({
                title: "设置校园网账号",
                notice: "请填写账号和密码，保存后自动继续登录",
                focus: "acc",
                requireAccount: true,
                onSaved: retryNow
            });
            return;
        }

        if (!captchaValid()) {
            openSettings({
                title: "请输入验证码",
                notice: "验证码缺失或已过期，请输入当前页面显示的验证码",
                focus: "cap",
                blankCaptcha: true,
                onSaved: retryNow
            });
            return;
        }

        refreshFormFieldsFromStorage();
        startLoginAttempt();
    }

    async function fillForm() {
        const accData = getAccount();
        if (!accData || !accData.acc || !accData.pwd) {
            hidePageLoading();
            const saved = await openSettings({
                title: "首次使用设置",
                notice: "请填写校园网账号和密码；验证码可在需要时再填写",
                focus: "acc",
                requireAccount: true
            });
            if (!saved || !getAccount()) {
                showToast("未设置账号，已取消自动登录", "info");
                return;
            }
        }

        if (!captchaValid()) {
            hidePageLoading();
            const saved = await openSettings({
                title: "请输入验证码",
                notice: "验证码缺失或已过期，请输入当前页面显示的验证码",
                focus: "cap",
                blankCaptcha: true
            });
            if (!saved || !captchaValid()) {
                showToast("未设置有效验证码，已取消自动登录", "info");
                return;
            }
        }

        refreshFormFieldsFromStorage();
        startLoginAttempt();
    }

    async function init() {
        // 悬浮球独立创建：不依赖弹窗 UI，也不依赖 body，最大程度兼容原版行为
        createFab();
        // 页面刚打开、等待登录表单加载时给出提示
        showPageLoading("正在填入验证码…");

        try {
            await waitFor("#f1_div");
            fillForm();
        } catch (e) {
            hidePageLoading();
            console.log("页面未加载完成");
        }
    }

    init();
})();

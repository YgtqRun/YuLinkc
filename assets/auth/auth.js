// 御连 YuLink —— 认证页注入脚本（由 登录脚本.js 裁剪）
//
// 由 Rust 通过 WebView2 eval() 注入。__YL_CFG__ 会被 Rust 替换为 JSON：
// {
//   "account": "...", "password": "...", "isp": "2", "smsCode": "...",
//   "beacon": "http://127.0.0.1:<port>/report", "run": 1,
//   "timeoutMs": 15000, "pollMs": 500, "okWaitMs": 3500, "okWaitCount": 10
// }
(function () {
    'use strict';

    // 幂等守卫：同一文档只执行一次
    if (window.__YL_AUTH_RUNNING__) return;
    window.__YL_AUTH_RUNNING__ = true;

    // 注入可能落在初始 about:blank / dev 页面：上报 probe 供 Rust 侧诊断，不当作正式启动
    if (!/^https?:/i.test(location.protocol)) {
        report("probe", "proto:" + location.protocol + " url:" + location.href);
        return;
    }

    const CFG = __YL_CFG__;
    if (!CFG || typeof CFG !== "object" || !CFG.beacon) return;

    // ===== 结果回传：Image 信标，GET 请求，跨域不受 CORS 限制 =====
    function report(state, msg) {
        try {
            new Image().src = CFG.beacon +
                "?run=" + encodeURIComponent(CFG.run || 0) +
                "&state=" + encodeURIComponent(state) +
                "&msg=" + encodeURIComponent(msg || "");
        } catch (e) { /* 忽略回传异常，不阻塞主流程 */ }
    }

    // ===== 等待元素 =====
    function waitFor(selector, timeoutMs) {
        timeoutMs = timeoutMs || 15000;
        return new Promise(function (resolve, reject) {
            const start = Date.now();
            const timer = setInterval(function () {
                const el = document.querySelector(selector);
                if (el) {
                    clearInterval(timer);
                    resolve(el);
                    return;
                }
                if (Date.now() - start > timeoutMs) {
                    clearInterval(timer);
                    reject(new Error("timeout: " + selector));
                }
            }, 150);
        });
    }

    function fireMouse(el, type) {
        try {
            el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true }));
        } catch (e) {
            const ev = document.createEvent("MouseEvents");
            ev.initEvent(type, true, true);
            el.dispatchEvent(ev);
        }
    }

    function setValue(el, value) {
        el.value = value;
        el.dispatchEvent(new Event("input", { bubbles: true }));
    }

    function isAuthCodeError(text) {
        return /动态密码|短信|验证码|auth.?code|sms/i.test(text);
    }

    async function run() {
        report("started", "url:" + location.href);

        try {
            // 1) 等待认证表单（与油猴脚本 waitFor("#f1_div") 一致）
            await waitFor("#f1_div", CFG.timeoutMs || 15000);

            // 2) 账号 / 密码 / 运营商 / 动态密码
            const accInput = document.querySelector("#f1_div form input:nth-of-type(3)");
            const pwdInput = document.querySelector("#f1_div form input:nth-of-type(4)");
            if (!accInput || !pwdInput) {
                report("error", "input-not-found");
                return;
            }
            setValue(accInput, CFG.account || "");
            setValue(pwdInput, CFG.password || "");

            const isp = document.querySelector("#f1_div select[name='ISP_select']");
            if (isp && CFG.isp) {
                isp.value = CFG.isp;
                isp.dispatchEvent(new Event("change", { bubbles: true }));
            }

            const dynPass = document.querySelector("#dynPass");
            if (dynPass) setValue(dynPass, CFG.smsCode || "");

            report(
                "filled",
                "acc:" + (accInput.value === CFG.account ? 1 : 0) +
                ",pwd:" + (pwdInput.value === CFG.password ? 1 : 0) +
                ",isp:" + (isp ? isp.value : "-")
            );

            // 3) 点击登录（与油猴脚本一致：mousedown/mouseup/click + click）
            const btn = document.querySelector("#login_btn");
            if (!btn) {
                report("error", "login-btn-not-found");
                return;
            }
            fireMouse(btn, "mousedown");
            fireMouse(btn, "mouseup");
            fireMouse(btn, "click");
            btn.click();

            // 4) 轮询 #message：有文案 → 失败；到期无文案 → ok
            const startedAt = Date.now();
            const pollMs = CFG.pollMs || 1000;
            let count = 0;
            const timer = setInterval(function () {
                count++;
                const msg = document.querySelector("#message");
                if (msg && msg.innerText && msg.innerText.trim() !== "") {
                    clearInterval(timer);
                    const text = msg.innerText.trim();
                    report(isAuthCodeError(text) ? "captcha-error" : "failed", text);
                    return;
                }
                if (Date.now() - startedAt >= (CFG.okWaitMs || 10000) || count >= (CFG.okWaitCount || 10)) {
                    clearInterval(timer);
                    report("ok", "");
                }
            }, pollMs);
        } catch (err) {
            report("error", String((err && err.message) || err));
        }
    }

    run();
})();

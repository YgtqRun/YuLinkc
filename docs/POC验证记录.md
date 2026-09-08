# P0 POC 验证记录（2026-09-08）

目标（对应 `实现方案.md` §14.1）：

1. 验证屏幕外可见窗口下 `eval()` 注入可执行；
2. 用 mock 认证页验证注入脚本：填表 → 提交 → Image 信标回传成功；
3. 验证窗口创建/销毁循环无泄漏、无残留进程。

## 结果：全部通过

| POC 项 | 验证方式 | 结果 |
|---|---|---|
| 屏幕外窗口 eval | 窗口置于 (-32000,-32000)、无边框、不进任务栏、保持 visible | 通过，页面导航后第 1 次 eval 即收到 `started` |
| mock 填表/提交/回传 | 4 场景（无线成功 / 认证失败 / 动态密码错误 / 有线成功） | 4/4 通过，含中文错误消息 percent 解码 |
| 创建/销毁压力测试 | 同场景连续创建/销毁 10 次 | 10/10 通过；进程退出后无 yulink / WebView2 残留 |

## 运行方式

```powershell
# 完整四场景
$env:YULINK_POC="1"; $env:YULINK_POC_MODE="all"; npm run tauri dev

# 创建/销毁 10 次
$env:YULINK_POC="1"; $env:YULINK_POC_MODE="success"; $env:YULINK_POC_RUNS="10"; npm run tauri dev

# 强制显示/隐藏认证窗口（覆盖默认策略）
$env:YULINK_AUTH_VISIBLE="1"; $env:YULINK_POC="1"; $env:YULINK_POC_MODE="all"; npm run tauri dev
```

> 注意：WebView2 子进程在受管沙箱内无法启动（窗口不导航、eval 无响应），
> POC 须在非沙箱的真实桌面会话中运行。屏幕外位置在真实桌面上验证通过，
> 与“隐藏窗口 no-op”风险不冲突。

## 认证窗口显示配置（auth_window.rs）

认证页窗口的显示策略作为正式配置落在 `src-tauri/src/auth_window.rs`：

| 条件 | 认证窗口形态 |
|---|---|
| debug 构建（默认） | 屏幕内显示（带边框可调、带焦点），开发/联调时可直接观察真实页面 |
| release 构建（默认） | 屏幕外隐藏（-32000,-32000、无边框、不进任务栏，保持 visible 状态） |
| `YULINK_AUTH_VISIBLE=1/0` | 强制覆盖，例如 debug 下模拟生产隐藏路径，或 release 下临时显示排障 |

POC 用 `YULINK_AUTH_VISIBLE=0` 跑过隐藏路径（4/4 通过）；未设置变量时 debug
跑过显示路径（1/1 通过）。注入脚本与回传链路在两种形态下行为一致。

## 验证到的事实

- 认证窗口必须保持 `visible` 状态；`-32000,-32000` 屏幕外 + `skip_taskbar` +
  `decorations(false)` 时页面加载与 eval 均正常，且不打扰用户。
- 注入脚本在页面 `Finished` 后一次 eval 即成功；Rust 侧 400ms 重试 + 幂等守卫的
  设计可以应对“先到 about:blank / 页面未就绪”的时序。
- Image 信标（`http://127.0.0.1:<port>/report?run=&state=&msg=`）跨域可用，
  中文消息需按 UTF-8 percent 解码；本轮用 204 应答。
- `#message` 文案分类回传：普通失败 `failed`，命中
  `/动态密码|短信|验证码|auth.?code|sms/i` 时回传 `captcha-error`，
  为“动态密码作废”保留依据（油猴脚本未做此区分）。

## mock 页面 DOM 约定

`#f1_div form` 内前 2 个 input 为隐藏字段，第 3/4 个 input 对应账号/密码
（与注入脚本 `input:nth-of-type(3)/(4)` 一致），其后为 `#dynPass`、
`select[name='ISP_select']`、`#login_btn`、`#message`。
真实页面联调时用同一套选择器校准（见 `实现方案.md` §14.3）。

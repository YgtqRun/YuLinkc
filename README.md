# 御连 YuLink

面向校园网的自动认证程序。Windows 版常驻托盘，默认直接请求 Dr.COM 接口完成登录，
并保留隐藏 WebView2 填表流程；Linux / 飞牛 OS 版提供无界面常驻服务和 Web 设置页。
断线后自动恢复，已在线时不会重复登录。

> 当前版本：v1.1.0

> ⚠️ **仅支持暨阳学院的校园网认证**。默认认证页（无线 `http://172.26.255.2/`、
> 有线 `http://172.26.255.3/`）及页面元素选择器均针对暨阳学院门户定制；
> 其他校园网环境需要自行修改认证网址与 `selectors` 才能使用。

## 功能特性

- **后台自动登录**：默认直接请求 Dr.COM 认证接口完成登录；旧 WebView2 隐藏窗口
  填表流程保留，可在设置页切换；切换后下一轮巡检立即生效。
- **介质自适应**：识别当前承载默认路由的网卡——Wi-Fi 走无线认证页
  `http://172.26.255.2/`，有线走 `http://172.26.255.3/`（可在设置中修改）。
- **实时网络感知**：8 秒轻量心跳探测网络翻转，连续失败才认定离线，避免单次
  抖动误触发登录。
- **避免重复登录**：认证页没有登录表单但存在“注销”按钮时，判定为已在线，
  不重复提交登录。
- **登录失败自动重试**：失败按 30s → 60s → 120s → 240s 指数退避重试，
  动态密码在有效期内保留。
- **可信判定**：页面判定成功只作为候选，Rust 外网探测通过后才显示“已连接”。
- **凭据加密**：账号密码与动态密码用 Windows DPAPI 加密落盘，日志不写明文。
- **开机自启自愈**：自启项登记的是「当前 exe 路径」并每次启动自检，免安装版改名或
  挪目录后会自动改写登记路径；运行中改名也会在一分钟内自动修正。
- **更新检查**：联网后自动查一次 GitHub Release 版本，设置页「关于」区显示结果；
  只提示、不下载不安装，点击可跳转发布页。
- **主题可切换**：设置页可选「跟随系统 / 浅色 / 暗色」，跟随系统时随 Windows 的
  浅色/暗色设置实时切换。
- **本地时区日志**：`%APPDATA%\com.tauri-app.yulink\logs\yulink.log`，自动轮转。

## 安装

从 [Releases](https://github.com/YgtqRun/YuLinkc/releases) 下载 v1.1.0 产物：

| 文件 | 平台 | 说明 |
| --- | --- | --- |
| `yulink_1.1.0_x64-setup.exe` | Windows | NSIS 安装包（推荐） |
| `yulink_1.1.0_x64_en-US.msi` | Windows | MSI 安装包 |
| `yulink.v1.1.0.exe` | Windows | 免安装单文件 |
| `yulink-linux-x86_64` | Linux | x86_64 二进制（glibc，需要系统库） |
| `yulink_1.1.0_x86.fpk` | 飞牛 OS | 应用中心安装包 |

### Windows

前置依赖：Windows 10/11 自带的 WebView2 Runtime（绝大多数系统已内置）。

- 安装包：运行 `yulink_1.1.0_x64-setup.exe` 或 `yulink_1.1.0_x64_en-US.msi`，
  按提示安装后从开始菜单启动。
- 免安装：直接运行 `yulink.v1.1.0.exe`，改名或移动目录后首次运行会自动修正开机自启路径。

### Linux

前提：x86_64 Linux，建议 Debian 12、Ubuntu 24.04 或更新版本（二进制为 glibc 动态链接）。
Linux 版不依赖 WebView2 和 DPAPI，使用纯 HTTP 登录，也可以只跑命令行。

1. 安装二进制：

```bash
chmod +x yulink-linux-x86_64
sudo install -m 755 yulink-linux-x86_64 /usr/local/bin/yulink-linux
```

2. 初始化配置：

```bash
sudo mkdir -p /opt/yulink/data
sudo yulink-linux --data-dir /opt/yulink/data init
sudo nano /opt/yulink/data/config.json      # 填写 username / password
sudo yulink-linux --data-dir /opt/yulink/data set-sms 140423 27
```

`set-sms` 的第二个参数是有效期（小时），`-1` 表示永久有效；修改动态密码后同样执行
这条命令重新计时。

3. 先手动验证：

```bash
sudo yulink-linux --data-dir /opt/yulink/data status   # 查询门户状态
sudo yulink-linux --data-dir /opt/yulink/data once     # 离线则登录一次
```

4. 常驻运行（心跳 + 自动重登）：

```bash
sudo yulink-linux --data-dir /opt/yulink/data run
```

5. 开机自启（systemd）：

```ini
# /etc/systemd/system/yulink.service
[Unit]
Description=YuLink campus network client
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/yulink-linux --data-dir /opt/yulink/data run
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now yulink
systemctl status yulink
tail -f /opt/yulink/data/logs/yulink.log
```

可选 Web 设置页：`yulink-linux` 二进制本身不带前端文件，请从 FPK 的 `app/www`
提取，或在源码目录执行 `npm run build:web` 生成，然后启动：

```bash
sudo yulink-linux --data-dir /opt/yulink/data serve \
  --www-dir /opt/yulink/www --bind 0.0.0.0 --port 8080
```

浏览器打开 `http://<机器 IP>:8080/`。当前设置页没有登录保护，只建议在可信内网使用。

### 飞牛 OS（fnOS）

前提：x86_64 飞牛 OS 设备。FPK 已包含服务端二进制、Web 设置页和应用图标，安装后自动
启动 `serve`（设置页）和 `run`（心跳 + 自动重登），不需要再手动点登录。

1. 下载 `yulink_1.1.0_x86.fpk`。
2. 打开飞牛「应用中心 → 手动安装」，选择该 FPK，安装到任意存储卷。
3. 安装完成后，从桌面「御连 YuLink」图标打开，或在统一网关的 YuLink 入口
   （路径 `/app/yulink/`）打开设置页。
4. 在设置页填写账号、密码和动态密码，点「保存凭据」。设置页右上角可确认运行状态。
5. 确认应用中心里该应用的「开机自启」为开启；之后 NAS 开机即会自动联网。

日志位置（`<卷>` 为安装时选择的存储卷，通常为 `vol1`）：

- 认证与心跳：`/<卷>/@appdata/yulink/logs/yulink.log`
- Web 设置页：`/<卷>/@appdata/yulink/web.log`
- 常驻进程：`/<卷>/@appdata/yulink/run.log`

升级时下载新 FPK 后在同一位置再次手动安装覆盖即可，账号与动态密码等配置保存在
`@appdata/yulink/config.json`，不会丢失。

更多命令行、Docker 与联调说明见 [`linux/README.md`](linux/README.md)。

## 快速开始

1. 连接到校园网（无线或有线，认证前网络即可）。
2. 左键单击托盘图标打开设置。
3. 填写账号、密码（运营商固定为中国电信）。
4. 填写电信短信下发的 6 位动态密码，选择有效期（默认 27 小时）。
5. 点“保存凭据”，再点“立即登录”；或保存后等待自动登录。

状态点颜色：绿 = 已连接，黄 = 需要动态密码，蓝 = 登录中，红 = 失败/异常。

## 托盘与设置

- 左键单击托盘：打开/收起设置窗口。
- 右键菜单：**立即登录** / **退出**。

设置页包含：

| 设置项 | 说明 |
| --- | --- |
| 账号 / 密码 / 动态密码 | 校园网凭据；动态密码可设置 1/3/9/18/27 小时、永久或自定义 |
| 离校模式 | 暂停一切自动认证与手动验证 |
| 开机自启动 | 登录 Windows 后自动运行；登记项路径变化（改名/挪目录）时自动修正 |
| 开机启动加速 | 关闭 Windows 给「启动应用」加的延迟（写 `StartupDelayInMSec=0`，对本用户所有自启项生效，效果依系统版本而定） |
| 显示认证窗口 | 勾选后登录时可见门户页面；默认隐藏 |
| 断网自动重试 | 开：掉线后一直自动重登；关：仅开机后自动登录到首次成功，之后只保留心跳检测与状态提示 |
| 连接成功后退出 | 开机后首次连接成功即退出程序；开启时“断网自动重试”置灰（互斥） |
| 主题 | 跟随系统 / 浅色 / 暗色；跟随系统时随 Windows 的浅色/暗色设置实时切换 |
| 登录方式 | HTTP（新，默认）：直接请求 Dr.COM 接口；WebView2（旧）：隐藏窗口填表登录 |
| 无线/有线认证网址 | 按介质选择认证页 |

## 登录判定流程

默认 HTTP 模式直接执行 `chkstatus` → `checkPassStatus` → `login` → `chkstatus`，
成功后做一次外网确认。以下为切回 WebView2 旧模式时的流程：

```text
外网探活失败（多端点，连续 2 次）
        │
        ▼
按介质探测认证页可达性
        │
        ▼
加载认证页（隐藏 WebView2）
        │
        ├─ 出现登录表单 (#f1_div) → 自动填表登录 → Rust 外网确认 → 已连接
        ├─ 无表单但有注销按钮 → 已在线，不重复登录
        └─ 两者皆无 → 刷新重试一次，仍失败则指数退避
```

门户改版时可修改配置中的 `selectors`，无需改程序。

## 数据与日志位置

- 配置：`%APPDATA%\com.tauri-app.yulink\config.json`（凭据为 DPAPI 密文）
- 日志：`%APPDATA%\com.tauri-app.yulink\logs\yulink.log`
- 若日志初始化失败，会在配置目录生成 `logger-init-error.txt` 记录原因

## 更新检查

程序连上网络后会自动检查一次版本，也可以随时点设置页「关于」区的「更新」一行手动
检查。检查结果只有三种：`已是最新` / `发现新版本 x.y.z ↗`（点击打开 Release 页）/
`检查失败，点击重试`。**只提示，不下载也不安装**，更新动作由你在浏览器里完成。

版本信息来源按顺序尝试：

1. Release 资产里的 `version.json`（推荐每次发版都传一份）
   `https://github.com/YgtqRun/YuLinkc/releases/latest/download/version.json`

   ```json
   { "version": "1.1.0", "notes": "修了 xxx", "url": "https://github.com/YgtqRun/YuLinkc/releases/tag/v1.1.0" }
   ```

   只有 `version` 是必需的；`url` 缺省时点击会跳到 Releases 页面。tauri-action 生成的
   `latest.json` 也能直接识别。
2. GitHub API `releases/latest`：兜底方案。未认证时限 60 次/小时/IP，校园网出口 IP
   共享时容易被限流，所以缺 `version.json` 时会退到这条路径并记录日志
   （`更新清单不可用（HTTP 404），改用 GitHub API`）。

注意：仓库必须是公开的，否则两条路径都会返回 404（日志里会写明 HTTP 状态码）。
程序自身不做签名校验——它只读取版本号并提示，真正的安装包始终由你自己在浏览器中下载。

## 从源码构建

Windows 环境要求：Rust stable、Node.js 18+、WebView2 Runtime。

```powershell
npm install
npm run tauri dev     # 开发调试
npm run tauri build   # 打包
```

打包产物位于 `src-tauri\target\release\bundle\`。

Linux / 飞牛 OS 版：

```bash
cd linux
cargo build --release
cd ..
npm run build:web
```

把 `linux/target/release/yulink-linux` 与 `dist-web/` 分别放入 FPK 的
`app/server/` 和 `app/www/`；标签发布时 `.github/workflows/release.yml`
会自动完成这一步。默认构建依赖系统的 glibc；需要完全静态的二进制时，可自行安装
musl 工具链后使用 `--target x86_64-unknown-linux-musl` 构建。

> 注意：仓库不纳入本地联调素材（`docs/`、`scripts/`、`实现方案.md`、
> `登录脚本.js`）。`mock/wired.html` 与 `mock/wireless.html` 是 POC/E2E 的
> 编译依赖，已随仓库提供，保证 CI 和全新克隆可以直接构建。

## 仓库结构

```text
src/                     Vue 3 设置界面
src-tauri/src/           调度、网络探活、认证窗口、登录会话、配置存储
linux/                   无界面 HTTP 客户端、Web API 与飞牛 OS 包结构
assets/auth/auth.js      认证页注入脚本（填表/注销按钮识别）
public/                  前端静态资源
```

## 常见问题

- **动态密码过期**：设置页会提示补码；登录失败不会自动清除动态密码，有效期内可重试。
- **提示“认证页未返回登录表单”**：多为门户处于已在线/中间态，程序不会重复登录。
- **提示“未检测到校园网认证页”**：请确认已连接校园网无线/有线，而非仅连接外网热点。
- **登录后仍显示未确认**：检查外网探活地址是否被校园网策略拦截，可修改
  `externalProbeUrl` 后重启。
- **想观察登录过程**：在设置中开启“显示认证窗口”。
- **改了 exe 名字 / 换了目录后开机不自启**：程序会在每次启动时把自启项改写成当前
  路径（日志里会记录「已自动修复开机自启项（程序位置变化：旧路径）」），所以改名后
  **手动运行一次**即可恢复。另外 Windows「任务管理器 → 启动应用」里若把它关掉，程序
  不会擅自打开，设置页会同步显示为关闭，需要你手动打开开关。

# YuLink Linux / fnOS 版

纯 HTTP 的 Dr.COM 校园网客户端，不依赖 WebView2、Playwright、DPAPI 或 Windows API。
提供命令行、常驻服务、Web 设置页和飞牛 OS FPK，当前版本 v1.1.0。

## 已验证的协议

- 状态：`GET /drcom/chkstatus`
- 注销：`GET /drcom/logout`
- 动态密码校验：`GET :801/eportal/?c=Portal&a=checkPassStatus`
- 登录：`GET /drcom/login`
- 电信固定参数：`R3=2`、`0MKKey=123456`
- 动态密码只用于 `checkPassStatus` 的 `pass` 参数
- 门户按源 IP 识别，因此不需要 `wlan_user_ip` / `wlan_user_mac`

## 构建

```bash
cd linux
cargo build --release
```

二进制：`target/release/yulink-linux`

Linux/fnOS 上可以直接在设备上编译；也可以把仓库推到 GitHub 后手动运行
`linux-build` workflow，下载 `yulink-linux-x86_64` artifact。该 workflow
使用 `x86_64-unknown-linux-musl` 静态链接，适合作为 FPK 的 `app/server`。

## 配置

首次运行：

```bash
./target/release/yulink-linux init
```

编辑 `data/config.json`，填入账号、密码和动态密码。然后设置动态密码的保存时间：

```bash
./target/release/yulink-linux set-sms 140423 27
```

`smsHours=-1` 表示永久有效。命令会从当前时间开始计算有效期。

> 当前配置是本地 JSON，Unix 下会自动设为 `0600`。后续会换成
> AES-GCM/密钥文件方案；不要把 `data/` 提交到版本库。

## 使用

```bash
./target/release/yulink-linux status   # 查询门户在线状态
./target/release/yulink-linux logout   # 注销
./target/release/yulink-linux login    # 检查动态密码并登录
./target/release/yulink-linux once     # 探外网；离线则登录一次
./target/release/yulink-linux run      # 常驻：心跳 + 自动重登
./target/release/yulink-linux serve --www-dir ./www --port 8080
```

配置里同时保存两个认证页：

- 无线：`http://172.26.255.2/`
- 有线：`http://172.26.255.3/`

程序会读取默认路由网卡类型：有线优先 `.3`，无线优先 `.2`；第一个不可达时
自动尝试另一个。也可以用命令行强制指定：

```bash
./target/release/yulink-linux --portal http://172.26.255.3/ status
```

日志：`data/logs/yulink.log`，同时输出到控制台。

## Web 设置页

在仓库根目录构建 Web 版前端：

```bash
npm run build:web
```

把 `dist-web/` 放到 `linux/www/`，然后启动：

```bash
./target/release/yulink-linux serve \
  --www-dir ./www --bind 0.0.0.0 --port 8080
```

打开 `http://< NAS 地址 >:8080/`。页面复用 Windows 版的 Vue 设置界面，
宽屏下是一张居中的竖版卡片，背景由 `--web-bg-image` CSS 变量预留自定义入口。

Web API：

- `GET /api/status`
- `GET/POST /api/settings`
- `POST /api/login`
- `POST /api/logout`
- `POST /api/sms/clear`
- `POST /api/clear-all`

注意：当前 Web 页面会回显已保存的密码和动态密码，且 8080 端口没有额外登录保护。
只建议在可信内网使用；后续 FPK 会接统一网关或增加访问认证。

## 开机自启

仓库内的 `yulink-web.service` 是当前 NAS 使用的 systemd 模板：

```bash
sudo cp yulink-web.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now yulink-web
```

查看状态和日志：

```bash
systemctl status yulink-web
journalctl -u yulink-web -f
```

## 本地 mock 联调

在仓库根目录：

```bash
node scripts/mock-drcom.js
```

另一个终端：

```bash
cd linux
./target/release/yulink-linux \
  --config /tmp/yulink-mock.json \
  --portal http://127.0.0.1:18080/ \
  --eportal-port 18081 \
  status
```

需要先在 `/tmp/yulink-mock.json` 填入 mock 账号 `testuser` / `testpass` /
`140423`，或使用命令行参数后续补充的配置命令。

## 发布产物

- `yulink-linux-x86_64`：musl 静态链接二进制；
- `yulink_1.1.0_x86.fpk`：飞牛 OS 应用包（`app/server` + `app/www` +
  `cmd/main` + `manifest`，统一网关入口 `/app/yulink`）。

推送 `v*` 标签后由 `.github/workflows/release.yml` 自动构建并上传。

## 后续

1. 配置加密与 0600 密钥文件；
2. Web 设置页访问认证；
3. 统一网关入口复用 NAS 登录态。

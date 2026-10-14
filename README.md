# PigeonBox Desktop

[![Tag](https://img.shields.io/github/v/tag/pigeonbox/desktop)](https://github.com/pigeonbox/desktop/tags)
[![License](https://img.shields.io/github/license/pigeonbox/desktop)](LICENSE)

PigeonBox 文件快递柜桌面客户端（Tauri 2）。托盘常驻，一键连接你的文件快递柜服务器；内置 p2pc，支持设备间 P2P 直传。

> 🗂️ [PigeonBox 生态](https://github.com/orgs/pigeonbox)成员仓 · 总览见 [装配仓 pigeonbox](https://github.com/pigeonbox/pigeonbox)

## 下载

三平台安装包(Windows / macOS / Linux)统一发布在 [hub 仓 Releases](https://github.com/pigeonbox/pigeonbox/releases)(`desktop-v*` 资产);打 `desktop-v*` tag 后 CI 自动构建并回挂。

### 平台支持矩阵

| 平台 | 架构 | 安装包格式 |
|---|---|---|
| macOS | Apple Silicon / Intel | dmg |
| Windows | x64 | msi / nsis |
| Linux 通用 | amd64 / arm64 | deb / rpm / AppImage |

Linux 版依赖**系统提供的 webkit2gtk-4.1（libsoup3）**，要求 **glibc ≥ 2.35**（CI 在 ubuntu-22.04 / ubuntu-22.04-arm 构建，发布前有 glibc 地板守卫）：

- ✅ 适配目标：deepin 23+、统信 UOS 桌面专业版 **V25（2500）**、银河麒麟桌面 **V11**，以及 Ubuntu 22.04+ / Debian 12+ 等通用发行版（统信/麒麟上架前需真机验证：`ldd --version`、`apt policy libwebkit2gtk-4.1-0`）
- ❌ **不支持**信创老底座：统信 UOS V20 全系（1050/1060/1070，Debian 10 底座，glibc 2.28 且系统无 webkit2gtk-4.1）、银河麒麟桌面 V10 SP1（glibc 2.31）。替代：浏览器访问服务器 Web UI（功能完整）+ [p2pc-web 网页模式](https://github.com/pigeonbox/p2p/releases)（老底座专用：单静态二进制本地界面，内置 p2p 直传）

## 功能（v1 远程模式）

- 服务器地址持久化，启动自动重连
- 系统托盘：显示主窗 / 服务器设置 / 退出；左键单击托盘唤回主窗
- 连接后主窗即服务器完整 Web UI（分享/取件/管理后台全功能）

### 设备直传（v1.3+，内置 p2pc sidecar）

设置页「设备直传」面板：选文件发送 → 出 `XXXX-XXXX-XXXX` 口令；对端输口令 → 直连取走，文件落系统下载目录。

- 传输走 PAKE 口令认证 + UDP 打洞直连（失败自动回落加密中继），**服务端只见密文，文件不落服务器**——只需一个可达的 [p2pd 注册中心](https://github.com/pigeonbox/p2p)（联邦部署见 chart `p2p.enabled` 或 `docker run`）
- p2pc 以 Tauri sidecar 内嵌（CI 按 target triple 从 [p2p Releases](https://github.com/pigeonbox/p2p/releases) 拉取 `p2pc-<triple>`）；自 2026-10-08 起纳入发布列车钉版（`DEPS.env` `P2PC_REF`，由 trainctl 统一管理），当前钉 `v0.6.2`
- **传输协议 v2（desktop v1.3.1 起，p2pc 0.4+）与旧版互不兼容**：双端须升级到同版本，旧版互传首帧直接失败
- 当前钉版：desktop **v1.21.0** / p2pc **v0.6.2**（发布列车同号）；**wire 协议 v4 为破坏性变更**（p2pc v0.6 起），双端（desktop/p2pc）必须同版互传

## Roadmap

- [ ] 本机模式：内嵌 server 二进制（sidecar），一键把电脑变成文件柜
- [ ] 直传多文件/目录、进度条与速度显示
- [ ] 开机自启、消息通知原生集成
- [ ] 信创老底座适配线（统信 V20 全系 / 麒麟 V10 SP1）：p2pc-web 网页模式已在 [p2p 仓](https://github.com/pigeonbox/p2p)落地（单静态二进制，回环 Web UI + 内置直传），满足不了再评估 GTK3 原生壳；Tauri 2 依赖 webkit2gtk-4.1，老底座确定走不通，不采用 Tauri 1.x 双轨

## 开发

```bash
# 本地构建前需准备 sidecar(二进制不入库):
mkdir -p src-tauri/binaries
# 方式一: 从 p2p Releases 下载当前平台 triple
gh release download --repo pigeonbox/p2p --pattern "p2pc-aarch64-apple-darwin*" --dir src-tauri/binaries
# 方式二: 从本地 p2p 仓源码编译
(cd ../PigeonBox/p2p && GOOS=darwin GOARCH=arm64 CGO_ENABLED=0 go build -o ../../desktop/src-tauri/binaries/p2pc-aarch64-apple-darwin ./cmd/p2pc)

cargo build          # 编译检查
# 打包三平台安装包：push desktop-v* tag，CI（GitHub Actions）自动构建并发 Release
```

## License

Apache-2.0

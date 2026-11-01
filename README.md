# PigeonBox Desktop

[![Tag](https://img.shields.io/github/v/tag/pigeonbox/desktop)](https://github.com/pigeonbox/desktop/tags)
[![License](https://img.shields.io/github/license/pigeonbox/desktop)](LICENSE)

PigeonBox 文件快递柜桌面客户端（Tauri 2）。托盘常驻，一键连接你的文件快递柜服务器。设备间直传请使用独立的 **PigeonBeam（信鸽直传）** 应用。

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
- ❌ **不支持**信创老底座：统信 UOS V20 全系（1050/1060/1070，Debian 10 底座，glibc 2.28 且系统无 webkit2gtk-4.1）、银河麒麟桌面 V10 SP1（glibc 2.31）。替代：浏览器访问服务器 Web UI（功能完整）+ [PigeonBeam 网页模式](https://github.com/pigeonbox/p2p/releases)（老底座专用：单静态二进制本地界面，内置 p2p 直传）

## 功能（v1 远程模式）

- 服务器地址持久化，启动自动重连
- 系统托盘：显示主窗 / 服务器设置 / 退出；左键单击托盘唤回主窗
- 连接后主窗即服务器完整 Web UI（分享/取件/管理后台全功能）

### 一等取件端（托盘当快递柜）

把 PigeonBox 当成自己的文件柜：客户端常驻托盘，后台轮询服务器，**柜子有件待取**时弹系统通知，托盘菜单直接给出「一键取件」。

- **待取判定**：轮询 `GET /api/v1/user/shares`（未过期且尚未被取走的分享），并用 `GET /api/v1/notifies/mine` 里的「投递到件」通知反查哪些是**别人投到我柜子里的**（可在设置页把范围切到「全部未取件分享」）
- **一键取件**：托盘菜单「待取清单」点任一件 ⇒ 该件的取件码进剪贴板
- **🔒 剪贴板里只有 6 位取件码**——不放分享链接、不放取件密码（`pickup::clipboard_payload` 单点把闸：不是合法 6 位码就拒写）
- **凭据**：API Key（`pb_sk_` 开头）存**系统钥匙串**（macOS 钥匙串 / Windows 凭据管理器 / Linux Secret Service），不明文落盘
- 设置页（托盘「服务器设置」进入）可配置 API Key、柜子范围，并查看待取清单

### 设备直传：已独立为 PigeonBeam（信鸽直传）

设备直传（p2pc）已拆为独立应用 **PigeonBeam（信鸽直传）**，本客户端**不再随包 p2pc sidecar、不再编排设备直传**（2026-10-22 起）。

- 设置页「设备直传」面板保留入口但不内置组件：探测失败即明示「请安装 PigeonBeam」，相关控件停用
- 直传能力与版本线归 PigeonBeam / [p2p 仓](https://github.com/pigeonbox/p2p)维护：PAKE 口令认证 + UDP 打洞直连（失败自动回落加密中继），**服务端只见密文，文件不落服务器**——只需一个可达的 [p2pd 注册中心](https://github.com/pigeonbox/p2p)（联邦部署见 chart `p2p.enabled` 或 `docker run`）
- **传输协议 v2（desktop v1.3.1 起，p2pc 0.4+）与旧版互不兼容**：双端须升级到同版本，旧版互传首帧直接失败

## Roadmap

- [ ] 本机模式：内嵌 server 二进制（sidecar），一键把电脑变成文件柜
- [ ] 开机自启
- [x] 消息通知原生集成（一等取件端：托盘通知 + 系统 toast）
- [ ] 信创老底座适配线（统信 V20 全系 / 麒麟 V10 SP1）：PigeonBeam 网页模式已在 [p2p 仓](https://github.com/pigeonbox/p2p)落地（单静态二进制，回环 Web UI + 内置直传），满足不了再评估 GTK3 原生壳；Tauri 2 依赖 webkit2gtk-4.1，老底座确定走不通，不采用 Tauri 1.x 双轨

## 开发

```bash
cargo build          # 编译检查
# 打包三平台安装包：push desktop-v* tag，CI（GitHub Actions）自动构建并发 Release
```

## License

Apache-2.0

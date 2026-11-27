# TREE LINK — 电脑互联与多机实时操控

[English](README.md) | **简体中文** | [架构设计文档](docs/REMOTE_DESKTOP_DESIGN.md)

> **开源衍生项目声明（GPL-3.0-only 及附加条款）：**
> **TREE LINK** 是基于开源项目 **[Oink](https://github.com/oinkvm/oink)** (`v0.3.7`) 修改开发的衍生项目。本项目完整保留原作者署名及 [`LICENSE`](LICENSE)、[`LICENSE-ADDITIONAL-TERMS.md`](LICENSE-ADDITIONAL-TERMS.md)、[`NOTICE`](NOTICE) 与 [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md) 法律声明，并明确标注全部新增与修改内容；本项目为独立衍生作品，不以原版 Oink 名义宣传或背书。

## 核心特性

TREE LINK 完整保留了 Oink 优秀的底层跨平台能力——**原生键鼠/触控板共享、双向剪贴板同步、文件互传与跨设备拖拽、低延迟多机音频混音与麦克风转发、mTLS 默认加密**，并重点新增以下核心功能：

1. **Windows 单屏/多屏实时掌控多台 Mac 桌面（`crates/treelink-screen` + 实时控屏工作台）**
   - 在你的 Windows 显示器上**同时打开并实时操控两台（或多台）Mac 桌面**，支持**左右分屏 / 上下分屏 / 单屏聚焦 / 全屏**，无需频繁抬头看其他电脑即可多机并行开发。
   - **macOS 原生硬件采集与编码**：基于系统原生 `ScreenCaptureKit` + `VideoToolbox`（H.264 / HEVC 硬件编码，零 B 帧低延迟，最高 60fps，支持多显示器切换与动态关键帧恢复）。
   - **Windows 硬件解码渲染**：基于 WebCodecs (`VideoDecoder`) 硬件解码直绘，清晰锐利适合写代码。
   - **开发者友好的实时操控**：窗口内绝对坐标精准点击/拖拽/滚轮，复用 `oink-input` 原生 macOS 键位映射，一键切换 **`Ctrl ↔ ⌘Cmd` 快捷键模式**，内置 `⌘Space`、`⌘Tab`、自动防卡键保护与单机音频一键静音。
2. **局域网 IP + 有线/雷电直连统一架构**
   - 底层统一基于 IP 传输，无缝支持 **千兆/2.5G 有线局域网**、**网线直连 / Thunderbolt / USB4 雷电网桥（`169.254.0.0/16`）** 与 **Wi-Fi**。
   - 内置 **多网卡智能优先级排序（`network_rank.rs`）**：自动优先选用 `169.254.x.x` 直连线缆与有线网卡，支持同组自动认证令牌与手动 IP 立即连接。

## 30 秒实机试玩（无需在 Windows 上编译安装）

1. **在你的 Mac 上启动 TREE LINK 被控端**：
   - 双击项目根目录的 [`start-mac-host.command`](start-mac-host.command)，或在终端运行：
     ```bash
     cargo run --release -p treelink-screen --bin treelink-screen-host -- --request-permissions
     ```
   - 首次运行会引导开启 macOS **「屏幕录制」** 与 **「辅助功能」** 权限（在「系统设置 → 隐私与安全性」中勾选终端或 TREE LINK，随后重开一次程序生效）。
   - 终端会打印本机局域网 / 直连 IP 地址，例如：`http://192.168.50.67:47474/`（或网线/雷电直连的 `http://169.254.x.x:47474/`）。
2. **在你的 Windows 显示器上直接打开并操控**：
   - 用 Windows 自带的 **Edge 或 Chrome** 直接打开上面的 `http://<Mac_IP>:47474/`。
   - **开箱即用**：免证书自动启用 Apple Silicon GPU 实时视频流（若通过 Tauri 客户端或 HTTPS/localhost 访问则自动启用 60fps 硬件 H.264/HEVC）。
   - 点击右上角 **「+ 添加一台 Mac」** 输入第二台 Mac 的 IP，即可在 Windows 显示器上**左右并排同时实时操控两台 Mac**！
   - **直接拖拽 Windows 文件到任意 Mac 画面上**即可将文件传到对应 Mac 的 `~/Downloads/TREE-LINK-Received/`，也可一键互通剪贴板。

## 快速开发与构建

```bash
# 1. 编译与测试实时桌面引擎及独立 Mac 推流端 CLI
cargo test -p treelink-screen
cargo run -p treelink-screen --bin treelink-screen-host

# 2. 编译与测试桌面端前端界面
npm ci
npm run -w apps/oink typecheck
npm run -w apps/oink test
npm run -w apps/oink build
```

推送到 GitHub 后，`.github/workflows/treelink-build.yml` 会自动在 GitHub Actions 上执行测试并构建 **macOS (arm64)** 与 **Windows (x64)** 双端产物。

## 许可证

本项目以 **[GPL-3.0-only](LICENSE)** 开源发布，并完整保留上游 Oink 的 [附加条款](LICENSE-ADDITIONAL-TERMS.md) 与 [NOTICE](NOTICE)。

# TREE LINK（电脑互联以及电脑实时操控）— 架构与功能设计文档

> **项目名称**：**TREE LINK**（仓库名 `tree-link`）
> **项目基准**：基于 [oinkvm/oink](https://github.com/oinkvm/oink) `v0.3.7`（分支 `dev/remote-desktop`，上游 remote 为 `upstream`）。
> **合规说明**：Oink 采用 `GPL-3.0-only` 及附加条款（见 [`LICENSE-ADDITIONAL-TERMS.md`](../LICENSE-ADDITIONAL-TERMS.md)）。本项目开源为独立衍生作品 **TREE LINK**，保留 GPL-3.0-only 协议与对 Oink 的原始署名与法律声明，明确标注所有修改与新增内容，不以原版 Oink 名义宣传。

---

## 1. 核心目标

以 Oink 现有的高水准跨平台互联能力为基础，补齐 **“在 Windows 显示器上同时查看并实时操控两台 Mac 桌面”** 的能力，让开发者只需面对一块（或多块）Windows 主显示器即可同步开发、调试并掌控多台 Mac，同时 **完整保留** Oink 原有能力：

| 能力模块 | Oink 现有基座（完整保留） | 本项目新增/增强 |
| --- | --- | --- |
| **实时桌面画面** | 仅支持静态壁纸缩略图（`oink-display`） | **新增**：macOS 硬件采集编码（ScreenCaptureKit + VideoToolbox H.264/HEVC）→ Windows 硬件解码多窗口/分屏实时显示（同时看 2 台 Mac） |
| **实时键鼠操控** | 跨屏边缘穿越式 KVM（`oink-input` + `oink-routing`） | **新增**：窗口内绝对坐标映射 + 独立焦点捕获模式，点击/聚焦哪台 Mac 窗口即直接操控该 Mac，支持 `Win/Ctrl/Alt ↔ Cmd/Option` 开发快捷键映射；同时保留原 KVM 边缘穿越模式 |
| **剪贴板与文件互传** | 文本/图片/文件剪贴板同步 + 跨系统拖拽（`oink-clipboard`、`apps/file-provider`、`apps/finder-sync`） | **保留并增强**：支持直接把 Windows 本地文件拖拽进对应 Mac 的实时桌面窗口内完成投递 |
| **音频共享** | 低延迟系统音频采集（macOS CoreAudio / Windows WASAPI）、Opus/PCM 传输、多路混音、独立音量控制、麦克风转发（`oink-audio`） | **100% 保留**：两台 Mac 声音可同时汇总到 Windows 耳机/音箱，并在远程桌面工具栏提供快捷静音与音量滑块 |
| **发现与传输** | QUIC (UDP 47470, mTLS) + TCP Bulk 池 (`LowPool`, 活动感知限速) + mDNS/UDP 广播（已保留 `169.254/16` 直连网段）+ 手动 IP | **增强**：新增**多网卡优选与直连 IP 绑定**（有线/雷电直连优先于 Wi-Fi），以及独立的低延迟视频媒体通道 |

---

## 2. 连接方式选型：局域网 IP vs 直接插线数据连接

### 核心结论：**传输层统一基于 IP（QUIC + 视频流通道 + TCP 大文件池），物理层同时兼容局域网与“有线/雷电直连”**

1. **为什么不建议使用私有裸 USB Host-to-Host 协议？**
   - Windows PC 和 Mac 均为 USB Host（主机），普通 USB-C 数据线直接对插无法在硬件层建立 Host-to-Host 数据通道（需要专用桥接芯片线材与非标驱动），维护成本高、不稳定且难以 1 台 Win 同时直连 2 台 Mac。
2. **如何用“插线直连”获得极速体验，同时与局域网无缝统一？**
   - **方案 A（最推荐，1 Win + 2 Mac 综合最优）：千兆 / 2.5G 有线局域网（路由器或桌面小交换机）**
     - 延迟 `< 1 ms`，带宽 1000–2500 Mbps。两路 2K/4K 60fps 硬件编码桌面流总码率仅约 30–80 Mbps，留出充足余量给文件传输和音频。
     - 只需每台设备插一根网线（Mac 可用 USB-C 2.5G 网卡/拓展坞），Oink 自动发现或绑定静态 IP，开机即连。
   - **方案 B（极致低延迟/无路由器插线直连）：Thunderbolt / USB4 雷电桥接网络 或 网线直连**
     - 若 Windows 主板带 Thunderbolt 3/4 / USB4 接口，可直接用雷电线连 Mac，系统会自动建立 **10–40 Gbps 雷电网桥（Thunderbolt Bridge）** 并分配 `169.254.x.x` Link-Local IP。
     - 若用网线直连双机（或 USB-C 转网卡直连），系统同样自动分配 `169.254.x.x`。
     - **Oink 原生代码已显式支持该网段**：见 [`crates/oink-network/src/discovery/mod.rs:92`](../crates/oink-network/src/discovery/mod.rs#L92)（`// 169.254/16 也保留：两台电脑用网线直连时就是这种地址。`）。
   - **方案 C（无线兜底）：Wi-Fi 6 / 6E / 7 同局域网**
     - 适合移动场景，通过码率自适应保持流畅；当同时存在有线/直连 IP 与 Wi-Fi IP 时，本项目的**网卡优先级调度**将自动优先选用有线/雷电/2.5G 网卡链路。

---

## 3. 实时桌面与操控技术架构

### 3.1 为什么视频流不能直接塞进 Oink 现有的控制帧 / 音频 Datagram？
检查 [`crates/oink-protocol/src/v1/limits.rs`](../crates/oink-protocol/src/v1/limits.rs) 可知：
- `RELIABLE_FRAME = 16 KiB`（可靠控制流单帧上限）
- `DATAGRAM = 1100 B`（实时 Datagram 单包上限，按最小路径 MTU 设计）
- `STREAM_RECEIVE_WINDOW = 64 KiB`（QUIC 流窗口，面向小包低延迟控制）

而 2K/4K 60fps 关键帧（IDR）通常在 50–300 KiB 级别。如果强行塞进现有的 `I0`/`M0` 通道会造成严重的队头阻塞或频繁拆包，影响 Oink 最核心的“键鼠零卡顿 + 音频不爆音”体验。

**设计方案**：
- 复用 Oink 现有的 **mTLS 身份认证、设备组信任关系、IP 发现与直连地址管理**；
- 新增独立的视频传输通道（`crates/oink-screen`）：
  - **控制信令**（分辨率/帧率/显示器切换、请求关键帧 IDR、码率反馈、窗口焦点绑定）：走低开销控制通道；
  - **视频码流**（Annex-B H.264 / HEVC 帧，附带时间戳、帧类型、显示器 ID、光标位置/形状元数据）：走专用高吞吐 TLS/TCP（开启 `TCP_NODELAY`）或独立大窗口 QUIC 流，并与键鼠/音频通道完全隔离；
  - **背压与丢帧策略**：接收端解码积压超过 2 帧时，发送端主动丢弃非关键帧并请求下一个低延迟 IDR，保证**永远展示最新一帧（低延迟优先）**而非越积越慢。

### 3.2 各端实现方案

1. **macOS 被控端（采集 + 硬件编码）**：
   - **屏幕采集**：`ScreenCaptureKit`（`SCStream`，支持 macOS 14.2+，与 Oink 最低版本完全一致），支持按显示器 `CGDirectDisplayID` 精确选择屏幕、捕获原生光标或独立光标元数据、支持 Retina HiDPI 缩放与可变帧率（静止画面几乎 0 流量）。
   - **硬件编码**：`VideoToolbox`（`VTCompressionSession`，Apple Silicon 硬件加速）：
     - 编码格式：默认 H.264 High Profile（全平台 WebCodecs 零兼容问题），可选 HEVC（H.265，画质更锐利、更省带宽，适合 4K 写代码）；
     - 参数：`RealTime = true`、`AllowFrameReordering = false`（零 B 帧）、`EnableLowLatencyRateControl = true`、动态码率 6–40 Mbps。
   - **远程输入注入**：直接复用 Oink 现有 `oink-input` 的 macOS 原生注入能力（`CGEvent` + 完整键位表 [`crates/oink-input/src/macos/keymap.rs`](../crates/oink-input/src/macos/keymap.rs)），新增窗口内**绝对坐标移动/点击/滚轮/快捷键注入**入口。

2. **Windows 主控端（硬件解码 + 多 Mac 工作台 UI）**：
   - **多 Mac 工作台视图（支持同时控 2 台 Mac）**：
     - **独立窗口模式**：为 Mac 1、Mac 2 各自弹出独立的 Tauri/WebView2 远程桌面窗口，可自由拖到 Windows 多块显示器或左右贴边分屏；
     - **聚合工作台模式**：主界面内提供分屏（1×2 并排）或标签页切换，顶部精简状态栏显示：连接路径（直连/LAN/Wi-Fi + RTT 延迟 + FPS + 码率）、显示器切换、一键静音/音频出口、剪贴板/文件投递快捷按钮、快捷键透传开关（如 `Win键 → Cmd`、`Alt+Tab` 是否透传给 Mac）。
   - **硬件解码**：使用 Chromium/WebView2 原生 **WebCodecs (`VideoDecoder`)** + D3D11/DXVA 硬件解码直绘 `<canvas>`（极低延迟、支持 4:4:4/高码率清晰文字渲染）。

---

## 4. 分阶段实施路线图

- **Phase 1 — 工程初始化与验证闭环（当前阶段）**
  - 完成项目创建与 `oinkvm/oink` 基准克隆（已完成，位于本目录）。
  - 搭建 `crates/oink-screen` 模块骨架与 macOS 原生 `ScreenCaptureKit + VideoToolbox` 采集编码器，并先提供可立即在 Windows 上打开验证的低延迟远程桌面预览与控制原型。
- **Phase 2 — 深度集成 Oink 协议与多 Mac 并发操控**
  - 在 `oink-protocol` / `oink-agent` 中接入远程桌面能力协商、窗口绝对坐标与键盘事件路由（复用 `oink-input`），支持同时连接 2 台 Mac。
  - 在 `oink-network` 中加入多网卡测速/优先级策略（直连/有线 IP 优先于 Wi-Fi）及界面手动绑定偏好链路。
- **Phase 3 — 开发者体验增强**
  - 窗口内文件直接拖拽投递至对应 Mac（桥接 `oink-clipboard` / `Dnd`）；
  - 远程桌面工具栏集成 Oink 音频混音控制；
  - 快捷键透传方案（IDE 友好模式：`Cmd+Space`、`Cmd+Tab`、`Ctrl+C/V` 等自定义映射）；
  - 可选支持 Mac 合盖/息屏虚拟显示器（Headless 开发模式）。
- **Phase 4 — 双端打包与 CI**
  - 配置 macOS `.dmg` 与 Windows `.msi`/`.exe` 自动构建（GitHub Actions 复用 Oink 现有 `.github/workflows/` 管线）。

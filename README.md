# TREE LINK — Multi-Computer Interconnection & Real-Time Remote Desktop Control

**English** | [简体中文](README.zh-CN.md) | [Architecture & Design (ZH)](docs/REMOTE_DESKTOP_DESIGN.md)

> **Derivative Work Notice (GPL-3.0-only + Section 7 Attribution):**
> **TREE LINK** is a modified derivative work based on **[Oink](https://github.com/oinkvm/oink)** (`v0.3.7`, GPL-3.0-only with [additional terms](LICENSE-ADDITIONAL-TERMS.md)). Original Oink notices, copyright attributions, and third-party license inventories are preserved in [`LICENSE`](LICENSE), [`LICENSE-ADDITIONAL-TERMS.md`](LICENSE-ADDITIONAL-TERMS.md), [`NOTICE`](NOTICE), and [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md). TREE LINK is an independent derivative project and is not endorsed by or presented as the original Oink.

## What TREE LINK Adds on Top of Oink

TREE LINK retains all core Oink peer-to-peer capabilities — **native keyboard/mouse/trackpad KVM**, **bidirectional clipboard sync**, **cross-OS file transfer & drag-and-drop**, and **low-latency multi-device audio mixing & microphone forwarding** — and extends it with:

1. **Real-Time Multi-Mac Desktop Viewing & Control on Windows (`crates/treelink-screen` + `apps/oink/src/remote-desktop`)**
   - View and control **multiple Macs simultaneously** (side-by-side split, stacked split, or single focus) directly on your Windows monitor.
   - **macOS Native Capture & Hardware Encoding**: Built on `ScreenCaptureKit` + `VideoToolbox` hardware H.264 / HEVC (`RealTime`, zero B-frames, 60fps, multi-display switching, dynamic keyframe recovery).
   - **Windows Hardware Decoding**: WebCodecs (`VideoDecoder`) hardware-accelerated rendering inside WebView2 / Chromium.
   - **Developer-Friendly Remote Input**: Pixel-accurate absolute pointer mapping, drag/scroll support, USB HID keyboard injection (`oink_input::mac_keymap`), toggleable `Ctrl ↔ ⌘Cmd` shortcut mode, quick `⌘Space` / `⌘Tab` triggers, and automatic anti-stuck-key cleanup on blur/disconnect.
2. **Unified LAN IP & Direct-Cable Networking (`169.254.0.0/16` + Wired Priority)**
   - Works seamlessly across **Gigabit/2.5G Ethernet LAN**, **Thunderbolt / USB4 Bridge**, **direct Ethernet cable (`169.254.x.x`)**, and **Wi-Fi**.
   - Automatic interface ranking prioritizes direct-cable (`169.254/16`, Thunderbolt/USB4) and wired Ethernet links ahead of Wi-Fi, with automatic group-derived authentication tokens and manual IP override.

## Quick Start

### Build Locally (macOS Apple Silicon)

```bash
# 1. Build and test TREE LINK remote desktop engine + standalone host CLI
cargo test -p treelink-screen
cargo run -p treelink-screen --bin treelink-screen-host

# 2. Build frontend & verify tests
npm ci
npm run -w apps/oink typecheck
npm run -w apps/oink test
npm run -w apps/oink build
```

### Automated Cross-Platform Builds (Windows x64 + macOS arm64)

Push to GitHub or trigger the **TREE LINK Build & Test** workflow (`.github/workflows/treelink-build.yml`) in GitHub Actions to build both Windows x64 and macOS arm64 artifacts automatically.

## License

TREE LINK is released under **[GPL-3.0-only](LICENSE)**, retaining upstream **[additional terms](LICENSE-ADDITIONAL-TERMS.md)** and **[NOTICE](NOTICE)** from Oink.

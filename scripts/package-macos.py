#!/usr/bin/env python3
"""Build and package TREE LINK macOS .app bundle and drag-to-Applications .dmg installer.

Works locally on Apple Silicon / Intel Macs and in GitHub Actions macos-15 runners
without requiring paid Apple Developer certificates (uses clean bottom-up ad-hoc signing).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IDENTITY = json.loads((ROOT / "packaging/identity.json").read_text())
VERSION = IDENTITY["version"]
MIN_MACOS = IDENTITY["macos_minimum"]

LOCALIZED_STRINGS = {
    "en": {
        "CFBundleName": "TREE LINK",
        "CFBundleDisplayName": "TREE LINK",
        "NSScreenCaptureUsageDescription": "TREE LINK captures your Mac display to stream real-time 60fps remote desktop to your paired devices.",
        "NSLocalNetworkUsageDescription": "TREE LINK discovers and connects your Macs and Windows PC over LAN or direct cable.",
        "NSAudioCaptureUsageDescription": "TREE LINK shares system audio with your paired devices.",
        "NSMicrophoneUsageDescription": "TREE LINK shares your microphone only while another paired device uses it.",
    },
    "zh-Hans": {
        "CFBundleName": "TREE LINK",
        "CFBundleDisplayName": "TREE LINK",
        "NSScreenCaptureUsageDescription": "TREE LINK 需要录制本机屏幕，以便在你的 Windows 显示器上实时 60fps 操控这台 Mac。",
        "NSLocalNetworkUsageDescription": "TREE LINK 在局域网或网线/雷电直连中发现并连接你的多台电脑。",
        "NSAudioCaptureUsageDescription": "TREE LINK 将系统音频低延迟共享到你选择的设备。",
        "NSMicrophoneUsageDescription": "TREE LINK 仅在其他已配对设备使用时共享你的麦克风。",
    },
}

README_ZH = """TREE LINK macOS 安装与使用说明
====================================

1. 安装方法：
   将本窗口中的「TREE LINK.app」拖入右侧的「Applications（应用程序）」文件夹即可。

2. 首次启动（免签名提示处理）：
   因为本安装包为开源自构建版本（未付费经过 Apple 公证），首次在“应用程序”中打开时：
   - 方法 A：右键点击「TREE LINK.app」→ 选择「打开」→ 在弹窗中再点一次「打开」。
   - 方法 B：若系统提示“已损坏”或无法验证开发者，在终端执行一次：
     xattr -dr com.apple.quarantine "/Applications/TREE LINK.app"
     然后双击即可正常启动。

3. 首次授权（仅需设置一次）：
   打开「系统设置 → 隐私与安全性」并勾选「TREE LINK」：
   - 「录屏与系统录音」：用于在 Windows 显示器上实时查看本机 60fps 画面
   - 「辅助功能」：用于接收来自 Windows 的鼠标点击、滚轮和键盘操控

4. 两种连接方式：
   - 方式 A（推荐）：在 Windows 安装 TREE LINK Windows 版（.msi），与两台 Mac 配对后，
     在「远程桌面」标签页一键并排打开两台 Mac（自动优先直连线缆/有线 LAN，免证书 60fps H.264，
     同时支持跨屏拖拽文件、剪贴板自动同步、多机低延迟音频）。
   - 方式 B（零安装浏览器直连）：在 Mac 端打开 TREE LINK → 「远程桌面」→ 复制「60fps 链接」，
     直接在 Windows Edge/Chrome 打开即可控制。

开源许可声明：
TREE LINK 基于 GPL-3.0 开源项目 Oink 衍生修改，保留全部原始开源许可与归属声明
（详见应用包 Contents/Resources/ 下的 LICENSE、LICENSE-ADDITIONAL-TERMS.md 与 NOTICE）。
"""


def run(cmd: list[str | Path], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    printable = " ".join(str(x) for x in cmd)
    print(f"[TREE LINK macOS] $ {printable}", flush=True)
    subprocess.run([str(x) for x in cmd], cwd=cwd, env=env, check=True)


def write_plist(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(plistlib.dumps(data))


def write_lproj(bundle: Path, include_permissions: bool) -> None:
    for lang, strings in LOCALIZED_STRINGS.items():
        filtered = {
            k: v
            for k, v in strings.items()
            if include_permissions or not k.endswith("UsageDescription")
        }
        out = bundle / "Contents/Resources" / f"{lang}.lproj" / "InfoPlist.strings"
        write_plist(out, filtered)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def copy_legal_notices(resources_dir: Path) -> None:
    for name in [
        "LICENSE",
        "LICENSE-ADDITIONAL-TERMS.md",
        "NOTICE",
        "THIRD_PARTY_LICENSES.md",
        "third-party-licenses.json",
    ]:
        src = ROOT / name
        if src.is_file():
            shutil.copy2(src, resources_dir / name)


def build_binaries(target: str, env: dict[str, str], skip_frontend: bool) -> Path:
    if not skip_frontend:
        run(["npm", "run", "-w", "apps/oink", "build"], env=env)
    run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--target",
            target,
            "--features",
            "oink-ui/custom-protocol",
            "-p",
            "oink-ui",
            "-p",
            "oink-agent",
            "-p",
            "oink-crash",
            "-p",
            "treelink-screen",
        ],
        env=env,
    )
    cargo_target = Path(env.get("CARGO_TARGET_DIR", ROOT / "target"))
    return cargo_target / target / "release"


def assemble_app(binaries: Path, app_path: Path, arch: str, build_label: str) -> None:
    if app_path.exists():
        shutil.rmtree(app_path)
    mac_dir = app_path / "Contents/MacOS"
    res_dir = app_path / "Contents/Resources"
    mac_dir.mkdir(parents=True)
    res_dir.mkdir(parents=True)

    for exe_name in ["oink-backend", "oink-agent", "oink-crash-collector", "treelink-screen-host"]:
        src = binaries / exe_name
        if not src.is_file():
            raise FileNotFoundError(f"Expected compiled binary missing: {src}")
        shutil.copy2(src, mac_dir / exe_name)
        os.chmod(mac_dir / exe_name, 0o755)

    icon_src = ROOT / "apps/oink/src-tauri/icons/icon.icns"
    shutil.copy2(icon_src, res_dir / "icon.icns")
    copy_legal_notices(res_dir)

    # Nested UI helper at the exact path oink-backend launches (Contents/Helpers/Oink.app).
    helper = app_path / "Contents/Helpers/Oink.app"
    (helper / "Contents/MacOS").mkdir(parents=True)
    (helper / "Contents/Resources").mkdir(parents=True)
    shutil.copy2(binaries / "oink", helper / "Contents/MacOS/oink")
    os.chmod(helper / "Contents/MacOS/oink", 0o755)
    shutil.copy2(icon_src, helper / "Contents/Resources/icon.icns")

    write_plist(
        helper / "Contents/Info.plist",
        {
            "CFBundleExecutable": "oink",
            "CFBundleIdentifier": IDENTITY["macos"]["ui_id"],
            "CFBundleName": "TREE LINK",
            "CFBundleDisplayName": "TREE LINK",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": VERSION,
            "CFBundleVersion": build_label,
            "CFBundleIconFile": "icon.icns",
            "CFBundleDevelopmentRegion": "en",
            "CFBundleLocalizations": ["en", "zh-Hans"],
            "LSMinimumSystemVersion": MIN_MACOS,
            "LSUIElement": True,
            "NSHighResolutionCapable": True,
        },
    )
    write_lproj(helper, include_permissions=False)

    # Main outer bundle metadata (cleanly branded TREE LINK).
    main_info = {
        "CFBundleExecutable": "oink-backend",
        "CFBundleIdentifier": "com.treelink.app",
        "CFBundleName": "TREE LINK",
        "CFBundleDisplayName": "TREE LINK",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": VERSION,
        "CFBundleVersion": build_label,
        "CFBundleIconFile": "icon.icns",
        "CFBundleDevelopmentRegion": "en",
        "CFBundleLocalizations": ["en", "zh-Hans"],
        "LSMinimumSystemVersion": MIN_MACOS,
        "LSUIElement": True,
        "NSHighResolutionCapable": True,
        "NSBonjourServices": [IDENTITY["bonjour_service"]],
        **LOCALIZED_STRINGS["en"],
    }
    write_plist(app_path / "Contents/Info.plist", main_info)
    write_lproj(app_path, include_permissions=True)

    metadata = {
        "schema": 1,
        "product": "TREE LINK",
        "derivedFrom": "Oink",
        "version": VERSION,
        "build": build_label,
        "architecture": arch,
        "minimum_os": MIN_MACOS,
        "signing": "ad-hoc",
    }
    (res_dir / "treelink-build.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")

    # Bottom-up ad-hoc signing so nested signatures stay valid for macOS TCC.
    run(["codesign", "--force", "--sign", "-", "--identifier", IDENTITY["macos"]["ui_id"], helper])
    run(["codesign", "--force", "--sign", "-", "--identifier", "com.treelink.app", app_path])
    run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", app_path])


def run_smoke_test(app_path: Path) -> None:
    """Verify the packaged binary boots cleanly in isolated launch-smoke mode."""
    with tempfile.TemporaryDirectory(prefix="treelink-smoke-") as tmp:
        env = os.environ.copy()
        env["OINK_HOME"] = tmp
        backend = app_path / "Contents/MacOS/oink-backend"
        proc = subprocess.Popen(
            [str(backend), "--launch-smoke"],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            ready = Path(tmp) / "launch-smoke-ready"
            import time

            deadline = time.monotonic() + 15.0
            while time.monotonic() < deadline:
                if ready.is_file():
                    print("[TREE LINK macOS] Launch smoke verified: AppKit + backend ready", flush=True)
                    return
                if proc.poll() is not None:
                    out, err = proc.communicate(timeout=2)
                    raise RuntimeError(
                        f"launch-smoke exited early ({proc.returncode}): {err.decode(errors='replace') or out.decode(errors='replace')}"
                    )
                time.sleep(0.2)
            raise RuntimeError("launch-smoke timed out waiting for readiness marker")
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()


def create_dmg(app_path: Path, dmg_path: Path) -> None:
    if dmg_path.exists():
        dmg_path.unlink()
    with tempfile.TemporaryDirectory(prefix="treelink-dmg-stage-") as tmp:
        stage = Path(tmp) / "TREE LINK"
        stage.mkdir()
        shutil.copytree(app_path, stage / "TREE LINK.app", symlinks=True)
        os.symlink("/Applications", stage / "Applications")
        (stage / "安装与使用说明.txt").write_text(README_ZH, encoding="utf-8")
        run(
            [
                "hdiutil",
                "create",
                "-volname",
                "TREE LINK",
                "-srcfolder",
                stage,
                "-ov",
                "-format",
                "UDZO",
                dmg_path,
            ]
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--arch",
        choices=["arm64", "x64"],
        default="arm64" if platform.machine().lower() == "arm64" else "x64",
    )
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist-packages")
    parser.add_argument("--binaries-dir", type=Path, help="Use precompiled release binaries instead of building")
    parser.add_argument("--skip-frontend", action="store_true")
    parser.add_argument("--skip-smoke", action="store_true")
    parser.add_argument("--build-label", default="1")
    args = parser.parse_args()

    if platform.system() != "Darwin":
        raise SystemExit("scripts/package-macos.py must be run on macOS")

    target = "aarch64-apple-darwin" if args.arch == "arm64" else "x86_64-apple-darwin"
    env = os.environ.copy()
    env["MACOSX_DEPLOYMENT_TARGET"] = MIN_MACOS
    env["CMAKE_POLICY_VERSION_MINIMUM"] = "3.5"
    env["npm_config_engine_strict"] = "false"
    cargo_bin = Path.home() / ".cargo/bin"
    local_bin = Path.home() / ".local/bin"
    env["PATH"] = f"{cargo_bin}:{local_bin}:{env.get('PATH', '')}"

    binaries = args.binaries_dir or build_binaries(target, env, args.skip_frontend)
    out_dir = args.output_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    app_path = out_dir / "TREE LINK.app"
    dmg_name = f"TREE-LINK-{VERSION}-macos-{args.arch}.dmg"
    dmg_path = out_dir / dmg_name

    assemble_app(binaries, app_path, args.arch, args.build_label)
    if not args.skip_smoke:
        run_smoke_test(app_path)
    create_dmg(app_path, dmg_path)

    digest = sha256_file(dmg_path)
    sums_path = out_dir / f"SHA256SUMS-macos-{args.arch}.txt"
    sums_path.write_text(f"{digest}  {dmg_name}\n", encoding="utf-8")

    size_mb = dmg_path.stat().st_size / (1024 * 1024)
    print(f"\n[TREE LINK macOS] Package ready: {dmg_path} ({size_mb:.1f} MB)")
    print(f"[TREE LINK macOS] SHA-256: {digest}")


if __name__ == "__main__":
    main()

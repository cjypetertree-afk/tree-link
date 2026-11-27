#!/usr/bin/env python3
"""Build a complete, self-contained TREE LINK Windows x64 MSI installer (unsigned).

Compiles the TREE LINK desktop UI (`oink.exe` with embedded frontend), `OinkInput`
UIAccess Windows Service (`oink-service.exe`), `treelink-screen-host.exe`, helper
executables, native shell/installer DLLs, VB-CABLE archive, and GPL-3.0 legal notices
into `TREE-LINK-<version>-windows-x64.msi` using WiX 5.0.2 with zero signing secrets.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import urllib.request
import uuid
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "build-support"))
sys.path.insert(0, str(ROOT / "packaging/windows"))

from identity import IDENTITY, encode_build, windows_product_version  # noqa: E402
from payload import copy_notices  # noqa: E402
from report_manifest import write_manifest  # noqa: E402
from vbcable import bundle as bundle_vbcable, cleanup_header  # noqa: E402
from windows_credential_provider import build as build_credential_provider  # noqa: E402
from windows_version import set_display_version  # noqa: E402

WIX_VERSION = "5.0.2"
WIX_SHA256 = "f30ef0c74e2a986126539c5780be93ac24e8136eaf723b1937b26272703ae173"
UI_SHA256 = "5ef2c707614b9f70b6bbadd2d4abcb4124efee215e9b16bfbc80113079a604c7"
FIREWALL_SHA256 = "c9a1c99afab82225127b34036b719dea6c98c451e21b341ae1c62a674d0ba5ef"

VERSION = IDENTITY["version"]
TARGET = "x86_64-pc-windows-msvc"


def run(cmd: list[str | Path], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    printable = " ".join(str(x) for x in cmd)
    print(f"[TREE LINK Windows] $ {printable}", flush=True)
    subprocess.run([str(x) for x in cmd], cwd=cwd, env=env, check=True)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def ensure_msvc_env(env: dict[str, str]) -> dict[str, str]:
    if shutil.which("cl.exe", path=env.get("PATH")):
        return env
    vswhere = Path(env.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
    location = subprocess.check_output(
        [
            str(vswhere),
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ],
        text=True,
    ).strip()
    devcmd = Path(location) / "Common7/Tools/VsDevCmd.bat"
    if not location or not devcmd.is_file():
        raise RuntimeError("MSVC x64 build tools are required")
    lines = subprocess.check_output(f'cmd.exe /d /s /c ""{devcmd}" -arch=x64 >nul && set"', text=True)
    merged = env.copy()
    for line in lines.splitlines():
        key, sep, value = line.partition("=")
        if sep and key:
            merged[key] = value
    return merged


def wix_tool(cache: Path) -> list[str]:
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"wix-{WIX_VERSION}.zip"
    if not archive.exists():
        urllib.request.urlretrieve(
            f"https://api.nuget.org/v3-flatcontainer/wix/{WIX_VERSION}/wix.{WIX_VERSION}.nupkg",
            archive,
        )
    if sha256_file(archive) != WIX_SHA256:
        raise RuntimeError("WiX download hash mismatch")
    dest = cache / f"wix-{WIX_VERSION}"
    if not (dest / "tools/net6.0/any/wix.dll").is_file():
        with zipfile.ZipFile(archive) as zf:
            zf.extractall(dest)
    return ["dotnet", str(dest / "tools/net6.0/any/wix.dll")]


def wix_extension(cache: Path, name: str, expected_sha256: str) -> Path:
    package = f"WixToolset.{name}.wixext"
    dest = cache / f"{package}-{WIX_VERSION}"
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"{package}-{WIX_VERSION}.zip"
    if not archive.exists():
        url = f"https://api.nuget.org/v3-flatcontainer/{package.lower()}/{WIX_VERSION}/{package.lower()}.{WIX_VERSION}.nupkg"
        urllib.request.urlretrieve(url, archive)
    if sha256_file(archive) != expected_sha256:
        raise RuntimeError(f"WiX {name} extension download hash mismatch")
    dll = dest / "wixext5" / f"{package}.dll"
    if not dll.is_file():
        with zipfile.ZipFile(archive) as zf:
            zf.extractall(dest)
    return dll


def write_license_rtf(path: Path) -> None:
    banner = (
        "TREE LINK — Multi-Mac Real-Time Remote Desktop, KVM, Clipboard, File Transfer & Audio\n"
        "Derived from Oink (GPL-3.0-only). Modified and branded as TREE LINK.\n\n"
    )
    text = banner + (ROOT / "LICENSE").read_text(encoding="utf-8") + "\n" + (ROOT / "LICENSE-ADDITIONAL-TERMS.md").read_text(encoding="utf-8")
    text = text.replace("\\", "\\\\").replace("{", "\\{").replace("}", "\\}").replace("\n", "\\par\n")
    encoded = "".join(c if ord(c) < 128 else f"\\u{ord(c) if ord(c) < 32768 else ord(c) - 65536}?" for c in text)
    path.write_text("{\\rtf1\\ansi{\\fonttbl{\\f0 Segoe UI;}}\\f0\\fs18 " + encoded + "}", encoding="ascii")


def write_notices_wxs(path: Path, payload: Path, files: list[str]) -> None:
    ns = "http://wixtoolset.org/schemas/v4/wxs"
    ET.register_namespace("", ns)

    def node(parent, tag, **attrs):
        return ET.SubElement(parent, "{" + ns + "}" + tag, attrs)

    root = ET.Element("{" + ns + "}Wix")
    fragment = node(root, "Fragment")
    directories = {"": node(fragment, "DirectoryRef", Id="INSTALLFOLDER")}
    components = []
    for relative in files:
        current = ""
        for part in Path(relative).parts[:-1]:
            parent = current
            current = (Path(current) / part).as_posix()
            if current not in directories:
                ident = "NoticeDir_" + hashlib.sha256(current.encode()).hexdigest()[:20]
                directories[current] = node(directories[parent], "Directory", Id=ident, Name=part)
        ident = "Notice_" + hashlib.sha256(relative.encode()).hexdigest()[:20]
        comp = node(directories[current], "Component", Id=ident, Guid="*")
        node(comp, "File", Id=ident + "_file", Source=str(payload / relative), KeyPath="yes")
        components.append(ident)
    group = node(fragment, "ComponentGroup", Id="Notices")
    for ident in components:
        node(group, "ComponentRef", Id=ident)
    ET.indent(root)
    path.write_bytes(ET.tostring(root, encoding="utf-8", xml_declaration=True))


def git_head_sha() -> str:
    try:
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    except Exception:
        return "0" * 40


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=ROOT / "dist-packages")
    parser.add_argument("--tool-cache", type=Path, default=Path.home() / ".cache/treelink-win-tools")
    parser.add_argument("--sequence", type=int, default=1)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    if platform.system() != "Windows":
        raise SystemExit("scripts/package-windows.py must be run on Windows x64")

    seq = ((max(1, args.sequence) - 1) % 999) + 1
    build_num = encode_build(VERSION, seq)
    msi_version = windows_product_version(build_num)
    commit = git_head_sha()
    tree_hash = hashlib.sha256(commit.encode()).hexdigest()

    env = ensure_msvc_env(os.environ.copy())
    env["OINK_RELEASE_BUILD"] = env["RELEASE_BUILD"] = str(build_num)
    env["OINK_RELEASE_VERSION"] = VERSION
    env["OINK_SOURCE_COMMIT"] = commit
    env["OINK_SOURCE_TREE_SHA256"] = tree_hash
    env["CMAKE_POLICY_VERSION_MINIMUM"] = "3.5"
    env["npm_config_engine_strict"] = "false"
    env["CARGO_PROFILE_RELEASE_DEBUG"] = "1"
    env["CARGO_PROFILE_RELEASE_STRIP"] = "none"
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
        ["-C", "link-arg=/PDBALTPATH:%_PDB%", "-C", "target-feature=+crt-static"]
    )
    env.pop("RUSTFLAGS", None)

    binaries = Path(env.get("CARGO_TARGET_DIR", ROOT / "target")) / TARGET / "release"
    if not args.skip_build:
        run(["npm.cmd", "run", "-w", "apps/oink", "build"], env=env)
        run(
            [
                "cargo",
                "build",
                "--release",
                "--locked",
                "--target",
                TARGET,
                "--features",
                "oink-ui/custom-protocol",
                "-p",
                "oink-ui",
                "-p",
                "oink-agent",
                "-p",
                "oink-crash",
                "-p",
                "oink-winsvc",
                "-p",
                "treelink-screen",
            ],
            env=env,
        )
        build_credential_provider(binaries, env)

    out_dir = args.output_dir.resolve()
    work_dir = out_dir / "win-msi-work"
    if work_dir.exists():
        shutil.rmtree(work_dir)
    payload = work_dir / "payload"
    symbols = work_dir / "symbols"
    payload.mkdir(parents=True)
    symbols.mkdir(parents=True)

    b, w = IDENTITY["binaries"], IDENTITY["windows"]
    roles = ["ui", "agent", "service", "update_worker", "crash_collector"]
    for role in roles:
        src = binaries / (b[role] + ".exe")
        if not src.is_file():
            raise FileNotFoundError(f"Compiled binary missing: {src}")
        dst = payload / src.name
        shutil.copy2(src, dst)
        set_display_version(dst, VERSION)

    screen_host = binaries / "treelink-screen-host.exe"
    if not screen_host.is_file():
        raise FileNotFoundError(f"Compiled binary missing: {screen_host}")
    shutil.copy2(screen_host, payload / "treelink-screen-host.exe")

    cred_dll = binaries / "OinkCredentialProvider.dll"
    if not cred_dll.is_file():
        build_credential_provider(binaries, env)
    shutil.copy2(cred_dll, payload / "OinkCredentialProvider.dll")
    set_display_version(payload / "OinkCredentialProvider.dll", VERSION)

    report = write_manifest(
        payload / "report-manifest.json",
        tree_hash,
        VERSION,
        build_num,
        [
            ("agent", payload / (b["ui"] + ".exe")),
            ("agent", payload / (b["agent"] + ".exe")),
            ("ui_rust", payload / (b["ui"] + ".exe")),
        ],
    )

    product_code = str(uuid.uuid5(uuid.UUID(w["upgrade_code"]), f"treelink-{build_num}")).upper()
    record = {
        "schema": 1,
        "product": "TREE LINK",
        "derivedFrom": "Oink",
        "provenance": "source-tree",
        "source_tree_sha256": tree_hash,
        "version": VERSION,
        "source_commit": commit,
        "build": build_num,
        "architecture": "x64",
        "minimum_os": IDENTITY["windows_minimum"],
        "minimumWindowsBuild": IDENTITY["minimumWindowsBuild"],
        "protocol_baseline": 1,
        "upgrade_code": w["upgrade_code"],
        "components": [b[r] + ".exe" for r in roles] + ["treelink-screen-host.exe"],
        "updater_included": True,
        "collector_included": True,
        "signing": "unsigned",
        "report_manifest": report,
    }
    notice_files = copy_notices(payload) + bundle_vbcable(args.tool_cache.resolve(), payload)
    (payload / IDENTITY["metadata_file"]).write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    receipt = {
        k: record[k]
        for k in (
            "schema",
            "product",
            "source_tree_sha256",
            "version",
            "build",
            "architecture",
            "minimum_os",
            "minimumWindowsBuild",
            "updater_included",
            "collector_included",
            "protocol_baseline",
            "upgrade_code",
            "components",
        )
    }
    receipt["signature"] = "unsigned-open-source-build"
    (payload / w["receipt"]).write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")

    notes_file = work_dir / "embedded-update-notes.json"
    notes_file.write_text(
        json.dumps(
            {
                "schema": 1,
                "kind": "inhouse",
                "build": build_num,
                "source_commit": commit,
                "previous": None,
                "notes": {
                    "en": "TREE LINK multi-Mac real-time 60fps remote desktop, KVM, clipboard sync, file transfer, and audio.",
                    "zh-Hans": "TREE LINK 多 Mac 实时 60fps 远程桌面、键鼠共享、剪贴板同步、文件直传与低延迟音频。",
                },
            },
            ensure_ascii=False,
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    license_rtf = work_dir / "license.rtf"
    write_license_rtf(license_rtf)

    header = "#pragma once\n" + "\n".join(
        f"#define ID_{key} L{json.dumps(value)}"
        for key, value in {
            "DATA_DIRECTORY": IDENTITY["data_directory"],
            "INSTALL_DIR": w["install_directory"],
            "SERVICE": w["service_name"],
            "UI": b["ui"] + ".exe",
            "AGENT": b["agent"] + ".exe",
            "SERVICE_EXE": b["service"] + ".exe",
            "UPDATE_WORKER": b["update_worker"] + ".exe",
            "CRASH_COLLECTOR": b["crash_collector"] + ".exe",
            "MAINTENANCE": w["maintenance_directory"],
            "RUN_VALUE": w["run_value"],
            "RECEIPT": w["receipt"],
            "MAINTENANCE_LOCK": w["maintenance_lock"],
            "PRODUCT_CODE": product_code,
            "UPGRADE_CODE": "{" + w["upgrade_code"] + "}",
        }.items()
    ) + "\n"
    (work_dir / "identity.h").write_text(header, encoding="utf-8")
    (work_dir / "microphone-cleanup.h").write_text(
        cleanup_header((ROOT / "packaging/windows/Microphone.ps1").read_text(encoding="utf-8")),
        encoding="utf-8",
    )
    (work_dir / "paste-menu-version.h").write_text(
        f"#define OINK_BUILD_HIGH {build_num >> 16}\n"
        f"#define OINK_BUILD_LOW {build_num & 65535}\n"
        f"#define OINK_DISPLAY_VERSION {json.dumps(VERSION)}\n",
        encoding="utf-8",
    )

    # Compile PasteMenu.dll and InstallerCA.dll with static MSVC CRT (/MT).
    run(
        [
            "rc.exe",
            "/nologo",
            f"/i{work_dir}",
            f"/i{ROOT / 'packaging/windows'}",
            f"/fo{work_dir}/PasteMenu.res",
            ROOT / "packaging/windows/PasteMenu.rc",
        ],
        env=env,
    )
    run(
        [
            "cl.exe",
            "/nologo",
            "/LD",
            "/Z7",
            "/EHsc",
            "/std:c++17",
            "/W4",
            "/WX",
            "/MT",
            "/DUNICODE",
            "/D_UNICODE",
            ROOT / "packaging/windows/PasteMenu.cpp",
            work_dir / "PasteMenu.res",
            f"/Fo{work_dir}/PasteMenu.obj",
            "/link",
            "/DEBUG:FULL",
            f"/PDB:{symbols}/OinkPasteMenu.pdb",
            "/PDBALTPATH:%_PDB%",
            f"/OUT:{payload}/OinkPasteMenu.dll",
            f"/IMPLIB:{work_dir}/PasteMenu.lib",
            f"/DEF:{ROOT / 'packaging/windows/PasteMenu.def'}",
            "user32.lib",
            "uuid.lib",
        ],
        env=env,
    )
    run(
        [
            "cl.exe",
            "/nologo",
            "/LD",
            "/Z7",
            "/EHsc",
            "/std:c++17",
            "/W4",
            "/WX",
            "/MT",
            "/DUNICODE",
            "/D_UNICODE",
            f"/I{work_dir}",
            f"/I{ROOT / 'crates/oink-credential-provider'}",
            ROOT / "packaging/windows/InstallerCA.cpp",
            ROOT / "crates/oink-winsvc/native/unlock.cpp",
            f"/Fo{work_dir}/",
            "/link",
            "/DEBUG:FULL",
            f"/PDB:{symbols}/InstallerCA.pdb",
            "/PDBALTPATH:%_PDB%",
            f"/OUT:{work_dir}/InstallerCA.dll",
            f"/IMPLIB:{work_dir}/InstallerCA.lib",
            "crypt32.lib",
            "wtsapi32.lib",
            "oleaut32.lib",
            "msi.lib",
            "advapi32.lib",
            "shell32.lib",
            "ole32.lib",
            "bcrypt.lib",
            "user32.lib",
            "userenv.lib",
        ],
        env=env,
    )

    notices_wxs = work_dir / "notices.wxs"
    write_notices_wxs(notices_wxs, payload, notice_files)

    wix = wix_tool(args.tool_cache.resolve())
    firewall = wix_extension(args.tool_cache.resolve(), "Firewall", FIREWALL_SHA256)
    ui = wix_extension(args.tool_cache.resolve(), "UI", UI_SHA256)

    msi_name = f"TREE-LINK-{VERSION}-windows-x64.msi"
    msi_path = out_dir / msi_name
    if msi_path.exists():
        msi_path.unlink()

    values = {
        "UpdateNotesFile": str(notes_file),
        "LicenseRtf": str(license_rtf),
        "DialogBmp": str(ROOT / "packaging/windows/installer-background.bmp"),
        "BannerBmp": str(ROOT / "packaging/windows/installer-banner.bmp"),
        "ProductName": "TREE LINK",
        "Manufacturer": "TREE LINK (derived from Oink)",
        "ProductVersion": msi_version,
        "SourceTreeSha256": tree_hash,
        "ProductCode": product_code,
        "UpgradeCode": w["upgrade_code"],
        "MinimumWindowsBuild": str(IDENTITY["minimumWindowsBuild"]),
        "InstallDirectory": w["install_directory"],
        "ServiceName": w["service_name"],
        "ServiceDisplayName": "TREE LINK Input Service",
        "RegistryKey": w["registry_key"],
        "RunValue": w["run_value"],
        "MaintenanceDirectory": w["maintenance_directory"],
        "UiExe": b["ui"] + ".exe",
        "AgentExe": b["agent"] + ".exe",
        "ServiceExe": b["service"] + ".exe",
        "WorkerExe": b["update_worker"] + ".exe",
        "HasUpdateWorker": "1",
        "CollectorExe": b["crash_collector"] + ".exe",
        "HasCrashCollector": "1",
        "Receipt": w["receipt"],
        "DisplayVersion": VERSION,
        "Build": str(build_num),
        "MaintenanceGuid": str(uuid.uuid5(uuid.UUID(w["upgrade_code"]), "maintenance-directory-v1")),
        "Payload": str(payload),
        "CustomAction": str(work_dir / "InstallerCA.dll"),
        "AppIcon": str(ROOT / "apps/oink/src-tauri/icons/icon.ico"),
    }
    defines = [part for k, v in values.items() for part in ["-d", f"{k}={v}"]]
    run(
        [
            *wix,
            "build",
            ROOT / "packaging/treelink/Package.wxs",
            ROOT / "packaging/treelink/Dialogs.wxs",
            notices_wxs,
            "-ext",
            firewall,
            "-ext",
            ui,
            "-arch",
            "x64",
            "-culture",
            "en-US",
            *defines,
            "-intermediateFolder",
            work_dir / "obj",
            "-o",
            msi_path,
        ],
        env=env,
    )

    digest = sha256_file(msi_path)
    (out_dir / "SHA256SUMS-windows-x64.txt").write_text(f"{digest}  {msi_name}\n", encoding="utf-8")
    size_mb = msi_path.stat().st_size / (1024 * 1024)
    print(f"\n[TREE LINK Windows] MSI ready: {msi_path} ({size_mb:.1f} MB)")
    print(f"[TREE LINK Windows] SHA-256: {digest}")


if __name__ == "__main__":
    main()

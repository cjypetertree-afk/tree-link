#!/usr/bin/env bash
# TREE LINK macOS one-line automatic installer & deployer.
# Usage on any Apple Silicon Mac:
#   curl -fsSL https://raw.githubusercontent.com/cjypetertree-afk/tree-link/main/scripts/install-mac.sh | bash
set -euo pipefail

REPO="cjypetertree-afk/tree-link"
DMG_NAME="TREE-LINK-0.3.7-macos-arm64.dmg"
if [[ -n "${TREELINK_TAG:-}" ]]; then
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TREELINK_TAG}/${DMG_NAME}"
  LABEL="${TREELINK_TAG}"
else
  DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${DMG_NAME}"
  LABEL="latest"
fi

TMP_DIR="$(mktemp -d /tmp/treelink-install.XXXXXX)"
MOUNT_POINT="${TMP_DIR}/mnt"
DMG_PATH="${TMP_DIR}/${DMG_NAME}"

cleanup() {
  if mount | grep -q "${MOUNT_POINT}"; then
    hdiutil detach "${MOUNT_POINT}" -quiet || true
  fi
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

echo "==> [1/4] Downloading ${DMG_NAME} (${LABEL})..."
curl -fL --progress-bar "${DOWNLOAD_URL}" -o "${DMG_PATH}"

echo "==> [2/4] Mounting installer image..."
mkdir -p "${MOUNT_POINT}"
hdiutil attach "${DMG_PATH}" -nobrowse -readonly -mountpoint "${MOUNT_POINT}" -quiet

echo "==> [3/4] Installing TREE LINK.app to /Applications..."
pkill -x oink-backend 2>/dev/null || true
pkill -x oink 2>/dev/null || true
sleep 0.5
rm -rf "/Applications/TREE LINK.app"
cp -R "${MOUNT_POINT}/TREE LINK.app" "/Applications/TREE LINK.app"
xattr -dr com.apple.quarantine "/Applications/TREE LINK.app" 2>/dev/null || true

echo "==> [4/4] Launching TREE LINK..."
open "/Applications/TREE LINK.app"
sleep 2

LAN_IP="$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || true)"
echo ""
echo "============================================================"
echo "  TREE LINK installed to /Applications/TREE LINK.app"
if [[ -n "${LAN_IP}" ]]; then
  echo "  This Mac LAN IP : ${LAN_IP}"
  echo "  Remote Desktop  : port 47474 (HTTP/WS) / 47475 (HTTPS 60fps H.264)"
fi
echo "  Next (first time only on this Mac):"
echo "    System Settings -> Privacy & Security -> enable TREE LINK in:"
echo "      1) Screen & System Audio Recording"
echo "      2) Accessibility"
echo "============================================================"

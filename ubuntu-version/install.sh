#!/usr/bin/env bash
# Builds the oneko-rust GNOME/Ubuntu backend and installs it: the oneko-daemon
# binary (to ~/.local/bin) plus the GNOME Shell extension that talks to it
# (to ~/.local/share/gnome-shell/extensions), then enables the extension.
set -euo pipefail

UUID="oneko-rust@abishek-pechiappan.github.io"
BIN_NAME="oneko-daemon"
INSTALL_DIR="${HOME}/.local/bin"
EXT_DIR="${HOME}/.local/share/gnome-shell/extensions/${UUID}"

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found. Install the Rust toolchain (e.g. 'sudo apt install rustc cargo', or https://rustup.rs)." >&2
    exit 1
}
command -v gnome-extensions >/dev/null 2>&1 || {
    echo "error: gnome-extensions not found. This installer is for a GNOME Shell desktop." >&2
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "${SCRIPT_DIR}")"
cd "${REPO_ROOT}"

echo "==> Building release binary..."
cargo build --release -p "${BIN_NAME}"

echo "==> Installing daemon to ${INSTALL_DIR}..."
mkdir -p "${INSTALL_DIR}"
install -m 755 "target/release/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) echo "note: ${INSTALL_DIR} is not on your PATH - that's fine, the extension launches the daemon by its full path." ;;
esac

echo "==> Installing GNOME Shell extension to ${EXT_DIR}..."
mkdir -p "${EXT_DIR}"
cp "${SCRIPT_DIR}/extension/metadata.json" "${SCRIPT_DIR}/extension/extension.js" "${EXT_DIR}/"

echo "==> Enabling extension..."
if gnome-extensions enable "${UUID}"; then
    echo "==> Done. The cat should appear and start following your cursor."
else
    echo "==> Installed, but couldn't enable it automatically."
    echo "    This is normal for a brand-new extension directory on some GNOME versions - log out and back in, then run:"
    echo "        gnome-extensions enable ${UUID}"
fi

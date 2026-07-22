#!/usr/bin/env bash
# Builds oneko-rust and installs it, with an optional Hyprland autostart entry.
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
BIN_NAME="oneko-rust"
HYPR_CONF="${HOME}/.config/hypr/hyprland.conf"

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found. Install the Rust toolchain (e.g. 'pacman -S rust')." >&2
    exit 1
}

echo "==> Building release binary..."
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"
cargo build --release -p "${BIN_NAME}"

echo "==> Installing to ${INSTALL_DIR}..."
mkdir -p "${INSTALL_DIR}"
install -m 755 "target/release/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) echo "warning: ${INSTALL_DIR} is not on your PATH. Add it in your shell config." ;;
esac

if [ -f "${HYPR_CONF}" ]; then
    EXEC_LINE="exec-once = ${INSTALL_DIR}/${BIN_NAME}"
    if grep -qF "${BIN_NAME}" "${HYPR_CONF}"; then
        echo "==> Hyprland autostart entry already present, skipping."
    else
        read -rp "Add autostart entry to ${HYPR_CONF}? [y/N] " reply
        if [[ "${reply}" =~ ^[Yy]$ ]]; then
            printf '\n%s\n' "${EXEC_LINE}" >> "${HYPR_CONF}"
            echo "==> Added: ${EXEC_LINE}"
        fi
    fi
else
    echo "==> No Hyprland config found at ${HYPR_CONF}, skipping autostart setup."
    echo "    To start manually, run: ${BIN_NAME}"
fi

echo "==> Done. Run '${BIN_NAME}' to start, or restart Hyprland if autostart was added."

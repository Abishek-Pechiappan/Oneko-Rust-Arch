#!/usr/bin/env bash
# Builds oneko-rust, installs it, and optionally wires up a Hyprland autostart
# entry with the options you pick. Re-run it any time to change those options.
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
BIN_NAME="oneko-rust"
HYPR_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/hypr"
HYPR_LUA="${HYPR_DIR}/hyprland.lua"
HYPR_CONF="${HYPR_DIR}/hyprland.conf"

# Every line this script writes carries this marker, so a re-run can find and
# replace its own entry instead of appending a second one.
MARKER="oneko-rust (managed by install.sh)"

# Defaults, matching the binary's own. Used as-is when running non-interactively.
SKIN="neko"
FPS="30"

ASSUME_YES=0
DO_AUTOSTART=1

usage() {
    cat <<EOF
Usage: ./install.sh [options]

Builds oneko-rust, installs it to ${INSTALL_DIR}, and offers to set up a
Hyprland autostart entry with the skin and framerate you choose.

Options:
  -y, --yes           Don't prompt; accept defaults (--skin ${SKIN} --fps ${FPS})
      --skin NAME     Use this skin instead of asking
      --fps N         Use this motion framerate instead of asking
      --no-autostart  Build and install only; don't touch any config
  -h, --help          Show this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        -y|--yes)       ASSUME_YES=1 ;;
        --skin)         SKIN="${2:?--skin needs a name}"; shift ;;
        --fps)          FPS="${2:?--fps needs a number}"; shift ;;
        --no-autostart) DO_AUTOSTART=0 ;;
        -h|--help)      usage; exit 0 ;;
        *)              echo "error: unknown option '$1'" >&2; usage >&2; exit 1 ;;
    esac
    shift
done

# Prompts only make sense on a terminal. Piped into a shell or run from CI, fall
# through to the defaults rather than hanging on a read that never returns.
[ -t 0 ] || ASSUME_YES=1

ask() { # ask <prompt> <default:y|n>
    local prompt="$1" default="$2" reply
    if [ "${ASSUME_YES}" -eq 1 ]; then [ "${default}" = "y" ]; return; fi
    local hint="[y/N]"; [ "${default}" = "y" ] && hint="[Y/n]"
    read -rp "${prompt} ${hint} " reply
    reply="${reply:-${default}}"
    [[ "${reply}" =~ ^[Yy]$ ]]
}

say()  { printf '==> %s\n' "$1"; }
warn() { printf 'warning: %s\n' "$1" >&2; }

# --- build -------------------------------------------------------------------

command -v cargo >/dev/null 2>&1 || {
    echo "error: cargo not found. Install the Rust toolchain (e.g. 'pacman -S rust')." >&2
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

say "Building release binary..."
cargo build --release

say "Installing to ${INSTALL_DIR}..."
mkdir -p "${INSTALL_DIR}"
install -m 755 "target/release/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
BIN="${INSTALL_DIR}/${BIN_NAME}"

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) warn "${INSTALL_DIR} is not on your PATH. Add it in your shell config, or use the full path ${BIN}." ;;
esac

# --- choose options ----------------------------------------------------------

# Read the skin list from the binary we just built, so this can never drift out
# of sync with what's actually compiled in.
list_skins() {
    "${BIN}" --list-skins 2>/dev/null |
        awk '/^built-in:/ {inlist=1; next} /^[^ ]/ {inlist=0} inlist && NF {print $1}'
}

if [ "${ASSUME_YES}" -eq 0 ]; then
    mapfile -t SKINS < <(list_skins)
    if [ "${#SKINS[@]}" -gt 0 ]; then
        echo
        say "Which cat?"
        # A hand-rolled menu rather than bash's `select`: `select` redisplays the
        # list on empty input instead of accepting it, so an advertised default
        # would never actually apply when you just press Enter.
        default_idx=1
        for i in "${!SKINS[@]}"; do
            n=$((i + 1))
            if [ "${SKINS[$i]}" = "${SKIN}" ]; then
                default_idx="${n}"
                printf '      %d) %s  (default)\n' "${n}" "${SKINS[$i]}"
            else
                printf '      %d) %s\n' "${n}" "${SKINS[$i]}"
            fi
        done
        read -rp "Choose [${default_idx}]: " choice
        choice="${choice:-${default_idx}}"
        if [[ "${choice}" =~ ^[0-9]+$ ]] && [ "${choice}" -ge 1 ] && [ "${choice}" -le "${#SKINS[@]}" ]; then
            SKIN="${SKINS[$((choice - 1))]}"
        else
            warn "'${choice}' isn't one of the choices; keeping ${SKIN}."
        fi
    fi

    echo
    say "Motion smoothness, in frames per second."
    echo "    The sprite animation always keeps its original 8 Hz cadence; this"
    echo "    only changes how smoothly the cat moves."
    echo "      30  smooth, costs almost nothing (default)"
    echo "       8  the classic stepped chase, exactly like the original"
    echo "      60  smoother, but about 4x the CPU of 30"
    read -rp "Choose [${FPS}]: " fps_choice
    fps_choice="${fps_choice:-${FPS}}"
    if [[ "${fps_choice}" =~ ^[0-9]+$ ]] && [ "${fps_choice}" -ge 1 ] && [ "${fps_choice}" -le 240 ]; then
        FPS="${fps_choice}"
    else
        warn "'${fps_choice}' isn't a number between 1 and 240; keeping ${FPS}."
    fi
fi

# Only pass flags that differ from the binary's own defaults, so the autostart
# line stays readable and doesn't pin values the program already uses.
ARGS=""
[ "${SKIN}" != "neko" ] && ARGS="${ARGS} --skin ${SKIN}"
[ "${FPS}" != "30" ]    && ARGS="${ARGS} --fps ${FPS}"
CMD="${BIN}${ARGS}"

echo
say "Command: ${CMD}"

# --- autostart ---------------------------------------------------------------

backup() {
    local file="$1" stamp
    stamp="$(date +%Y%m%d-%H%M%S)"
    cp -- "${file}" "${file}.bak-${stamp}"
    say "Backed up ${file} -> ${file}.bak-${stamp}"
}

# Replaces our previously-written line if there is one. Returns 0 if it did.
replace_existing() {
    local file="$1" newline="$2"
    grep -qF "${MARKER}" "${file}" || return 1
    backup "${file}"
    # Rewrite in place with awk rather than sed, so the replacement text is
    # never reinterpreted as a sed expression (paths and flags contain / and &).
    awk -v marker="${MARKER}" -v repl="${newline}" \
        'index($0, marker) { print repl; next } { print }' \
        "${file}" > "${file}.tmp" && mv -- "${file}.tmp" "${file}"
    say "Updated the existing entry in ${file}"
    return 0
}

setup_lua() {
    local line="    hl.exec_cmd(\"${CMD}\") -- ${MARKER}"

    if replace_existing "${HYPR_LUA}" "${line}"; then return; fi

    if grep -qF "${BIN_NAME}" "${HYPR_LUA}"; then
        warn "${HYPR_LUA} already mentions ${BIN_NAME}, but not in a line this script wrote."
        echo "    Leaving it alone. Edit it by hand if you want to change the options."
        return
    fi

    echo
    echo "    Would add to ${HYPR_LUA}:"
    echo "    ${line}"
    ask "Add it?" y || { say "Skipped autostart."; return; }

    backup "${HYPR_LUA}"
    if grep -q 'hl\.on("hyprland\.start"' "${HYPR_LUA}"; then
        # Insert into the existing startup handler rather than registering a
        # second one: whether hl.on stacks handlers or replaces them isn't
        # something this script can safely assume, and guessing wrong would
        # silently disable whatever else you start at login.
        awk -v repl="${line}" \
            'BEGIN { done=0 }
             { print }
             !done && /hl\.on\("hyprland\.start"/ { print repl; done=1 }' \
            "${HYPR_LUA}" > "${HYPR_LUA}.tmp" && mv -- "${HYPR_LUA}.tmp" "${HYPR_LUA}"
        say "Added to the existing hyprland.start handler."
    else
        printf '\n-- %s\nhl.on("hyprland.start", function()\n%s\nend)\n' \
            "${MARKER}" "${line}" >> "${HYPR_LUA}"
        say "Added a new hyprland.start handler."
    fi
}

setup_conf() {
    local line="exec-once = ${CMD} # ${MARKER}"

    if replace_existing "${HYPR_CONF}" "${line}"; then return; fi

    if grep -qF "${BIN_NAME}" "${HYPR_CONF}"; then
        warn "${HYPR_CONF} already mentions ${BIN_NAME}, but not in a line this script wrote."
        echo "    Leaving it alone. Edit it by hand if you want to change the options."
        return
    fi

    echo
    echo "    Would add to ${HYPR_CONF}:"
    echo "    ${line}"
    ask "Add it?" y || { say "Skipped autostart."; return; }

    backup "${HYPR_CONF}"
    printf '\n%s\n' "${line}" >> "${HYPR_CONF}"
    say "Added autostart entry."
}

if [ "${DO_AUTOSTART}" -eq 1 ]; then
    # Lua first: on Hyprland >= 0.55 a hyprland.lua takes precedence, and a
    # leftover hyprland.conf may still be sitting next to it.
    if [ -f "${HYPR_LUA}" ]; then
        setup_lua
    elif [ -f "${HYPR_CONF}" ]; then
        setup_conf
    else
        say "No Hyprland config found in ${HYPR_DIR}, skipping autostart."
        echo "    Start it yourself with: ${CMD}"
    fi
fi

# --- start it now ------------------------------------------------------------

echo
if pgrep -x "${BIN_NAME}" >/dev/null 2>&1; then
    if ask "${BIN_NAME} is already running. Restart it with the new options?" y; then
        pkill -x "${BIN_NAME}" || true
        sleep 0.3
        setsid "${BIN}" ${ARGS} >/dev/null 2>&1 < /dev/null &
        say "Restarted."
    fi
elif ask "Start ${BIN_NAME} now?" y; then
    # setsid so the cat outlives this shell instead of dying with it.
    setsid "${BIN}" ${ARGS} >/dev/null 2>&1 < /dev/null &
    sleep 0.5
    if pgrep -x "${BIN_NAME}" >/dev/null 2>&1; then
        say "Running."
    else
        warn "It exited immediately. Run it directly to see why: ${CMD}"
    fi
fi

echo
say "Done. Stop the cat with 'pkill ${BIN_NAME}', or re-run this script to change options."

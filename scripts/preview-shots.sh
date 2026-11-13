#!/usr/bin/env bash
# Capture the synthetic UI preview in each view and appearance on Hyprland.
#
# Usage: scripts/preview-shots.sh [-o OUTPUT_DIR] [-p NAME_PREFIX] [-a "light dark"] [-s WxH] [-e SCENARIO] [destination ...]
#
# Destinations: accounts mail mail-reader calendar settings (default: all of them).
# -e sets EINFACH_MAIL_UI_PREVIEW_SCENARIO, for example full-access.
# Each shot opens a real window on the active workspace, so leave the display alone
# while the script runs. The script parks the pointer in the bottom-right corner of the
# first monitor, so no hover state reaches a shot. Needs hyprctl, grim, jq, and a
# ui-preview build.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

output_dir="$repo_root/target/preview-shots"
name_prefix="preview"
appearances="light dark"
window_size="1280x800"
scenario=""
window_timeout_seconds=40
settle_seconds=4

while getopts "o:p:a:s:e:h" option; do
    case "$option" in
        o) output_dir="$OPTARG" ;;
        p) name_prefix="$OPTARG" ;;
        a) appearances="$OPTARG" ;;
        s) window_size="$OPTARG" ;;
        e) scenario="$OPTARG" ;;
        *) sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2 ;;
    esac
done
shift $((OPTIND - 1))
destinations=("$@")
if ((${#destinations[@]} == 0)); then
    destinations=(accounts mail mail-reader calendar settings)
fi

for tool in hyprctl grim jq cargo; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        printf 'Required tool is missing: %s\n' "$tool" >&2
        exit 127
    fi
done
if [[ -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]]; then
    printf '%s\n' "Run this script inside a Hyprland session." >&2
    exit 1
fi
if [[ ! "$window_size" =~ ^[0-9]+x[0-9]+$ ]]; then
    printf 'Window size must look like 1280x800, got %s\n' "$window_size" >&2
    exit 2
fi
window_width="${window_size%x*}"
window_height="${window_size#*x}"

export OPENSSL_NO_VENDOR="${OPENSSL_NO_VENDOR:-1}"
cargo build --locked --package einfach-mail-desktop --features ui-preview --bin einfach-mail
target_directory="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
preview_binary="$target_directory/debug/einfach-mail"

mkdir -p "$output_dir"
preview_pid=""

stop_preview() {
    if [[ -n "$preview_pid" ]]; then
        kill "$preview_pid" 2>/dev/null || true
        wait "$preview_pid" 2>/dev/null || true
        preview_pid=""
    fi
}
trap stop_preview EXIT

# Prints "x,y WxH" for the window that belongs to the preview process.
window_region() {
    hyprctl clients -j | jq -r --argjson pid "$preview_pid" \
        '.[] | select(.pid == $pid and .mapped) | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' |
        head -n 1
}

wait_for_window() {
    local waited=0
    while [[ -z "$(window_region)" ]]; do
        if ! kill -0 "$preview_pid" 2>/dev/null; then
            printf '%s\n' "The preview exited before it opened a window." >&2
            return 1
        fi
        if ((waited >= window_timeout_seconds * 4)); then
            printf 'No preview window appeared within %s seconds.\n' "$window_timeout_seconds" >&2
            return 1
        fi
        sleep 0.25
        waited=$((waited + 1))
    done
}

# Floats the window, then pins its size and position so every shot has the same geometry.
place_window() {
    local window="pid:$1"
    hyprctl eval "hl.dispatch(hl.dsp.window.float({ action = \"enable\", window = \"$window\" }))" >/dev/null
    hyprctl eval "hl.dispatch(hl.dsp.window.resize({ x = $window_width, y = $window_height, window = \"$window\" }))" >/dev/null
    hyprctl eval "hl.dispatch(hl.dsp.window.move({ x = 40, y = 60, window = \"$window\" }))" >/dev/null
}

# Moves the pointer to the bottom-right corner of the first monitor, away from the window.
park_pointer() {
    local corner
    corner="$(hyprctl monitors -j | jq -r '.[0] | "\(.x + .width - 1) \(.y + .height - 1)"')"
    hyprctl eval "hl.dispatch(hl.dsp.cursor.move({ x = ${corner% *}, y = ${corner#* } }))" >/dev/null
}

capture() {
    local destination="$1" appearance="$2"
    local shot="$output_dir/$name_prefix-$destination-$appearance.png"

    park_pointer
    env ${scenario:+EINFACH_MAIL_UI_PREVIEW_SCENARIO="$scenario"} \
        EINFACH_MAIL_UI_PREVIEW_DESTINATION="$destination" \
        EINFACH_MAIL_UI_PREVIEW_APPEARANCE="$appearance" \
        EINFACH_MAIL_UI_PREVIEW_WINDOW_SIZE="$window_size" \
        "$preview_binary" &
    preview_pid=$!

    wait_for_window
    # The window has no class or title, so the process ID is the only reliable match.
    place_window "$preview_pid"
    park_pointer
    sleep "$settle_seconds"

    local region
    region="$(window_region)"
    grim -g "$region" "$shot"
    printf '%s (%s)\n' "$shot" "$region"
    stop_preview
}

for destination in "${destinations[@]}"; do
    for appearance in $appearances; do
        capture "$destination" "$appearance"
    done
done

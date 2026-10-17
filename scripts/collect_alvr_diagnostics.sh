#!/usr/bin/env bash
set -euo pipefail

# Collect only diagnostic logs and basic runtime details for VeRantyx/ALVR.
# Deliberately excludes Steam credentials, account configuration, and full env dumps.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
OUTPUT_BASE="${TMPDIR:-/tmp}"
OUTPUT_DIR="$OUTPUT_BASE/Verantyx-diagnostics-$TIMESTAMP"
ARCHIVE="$OUTPUT_DIR.zip"

VERANTYX_VR_DRIVE="${VERANTYX_VR_DRIVE:-$HOME/Verantyx_VR_Drive}"
if [[ -n "${CROSSOVER_BOTTLE:-}" ]]; then
    WINEPREFIX="${WINEPREFIX:-$HOME/Library/Application Support/CrossOver/Bottles/$CROSSOVER_BOTTLE}"
else
    WINEPREFIX="${WINEPREFIX:-$VERANTYX_VR_DRIVE/SteamVR_Prefix_Wine11_Test}"
fi
LOG_DIR="${LOG_DIR:-$REPO_ROOT/logs}"
DXVK_LOG_DIR="${DXVK_LOG_DIR:-$LOG_DIR/dxvk}"
STEAM_ROOT="${STEAM_ROOT:-$WINEPREFIX/drive_c/Program Files (x86)/Steam}"
STEAM_LOG_DIR="$STEAM_ROOT/logs"

# Collect from each known Wine/CrossOver prefix instead of trusting stale log
# timestamps to identify the active bottle.
WINE_PREFIX_CANDIDATES=()
add_prefix_candidate() {
    local candidate="${1%/}" existing
    [[ -d "$candidate/drive_c" ]] || return 0
    for existing in "${WINE_PREFIX_CANDIDATES[@]}"; do
        [[ "$existing" == "$candidate" ]] && return 0
    done
    WINE_PREFIX_CANDIDATES+=("$candidate")
}
add_prefix_candidate "$WINEPREFIX"
if [[ -n "${CROSSOVER_BOTTLE:-}" ]]; then
    add_prefix_candidate "$HOME/Library/Application Support/CrossOver/Bottles/$CROSSOVER_BOTTLE"
fi
CROSSOVER_BOTTLES="$HOME/Library/Application Support/CrossOver/Bottles"
if [[ -d "$CROSSOVER_BOTTLES" ]]; then
    for candidate in "$CROSSOVER_BOTTLES"/*; do
        add_prefix_candidate "$candidate"
    done
fi
if [[ -d "$VERANTYX_VR_DRIVE" ]]; then
    for candidate in "$VERANTYX_VR_DRIVE"/*; do
        add_prefix_candidate "$candidate"
    done
fi

mkdir -p "$OUTPUT_DIR"/{repo,steam,dxvk,wine-prefix,system}

copy_file() {
    local source="$1"
    local destination="$2"
    [[ -f "$source" ]] || return 0
    mkdir -p "$(dirname "$OUTPUT_DIR/$destination")"
    cp -p "$source" "$OUTPUT_DIR/$destination"
    printf 'Collected %s\n' "$destination"
}

copy_glob() {
    local destination_dir="$1"
    shift
    local pattern file
    for pattern in "$@"; do
        # compgen expands a glob without word-splitting paths that contain spaces.
        while IFS= read -r file; do
            [[ -f "$file" ]] || continue
            copy_file "$file" "$destination_dir/$(basename "$file")"
        done < <(compgen -G "$pattern" || true)
    done
}

{
    printf 'Collected at: %s\n' "$(date)"
    printf 'Repository: %s\n' "$REPO_ROOT"
    printf 'Current branch/commit (when available):\n'
    git -C "$REPO_ROOT" branch --show-current 2>/dev/null || true
    git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || true
    printf '\nSelected runtime settings (paths may reveal your local username):\n'
    printf 'VERANTYX_TRANSPORT=%s\n' "${VERANTYX_TRANSPORT:-unset}"
    printf 'VERANTYX_D3D_BACKEND=%s\n' "${VERANTYX_D3D_BACKEND:-unset}"
    printf 'VERANTYX_USE_DXVK=%s\n' "${VERANTYX_USE_DXVK:-unset}"
    printf 'CROSSOVER_BOTTLE=%s\n' "${CROSSOVER_BOTTLE:-unset}"
    printf 'WINEPREFIX=%s\n' "$WINEPREFIX"
    printf 'STEAM_ROOT=%s\n' "$STEAM_ROOT"
    printf 'STEAM_LOG_DIR=%s\n' "$STEAM_LOG_DIR"
    printf 'LOG_DIR=%s\n' "$LOG_DIR"
    printf 'DXVK_LOG_DIR=%s\n' "$DXVK_LOG_DIR"
    printf 'Candidate Wine/CrossOver prefixes:\n'
    for candidate in "${WINE_PREFIX_CANDIDATES[@]}"; do printf '  %s\n' "$candidate"; done
    printf 'STEAM_ROOT=%s\n' "$STEAM_ROOT"
    printf 'STEAM_LOG_DIR=%s\n' "$STEAM_LOG_DIR"
    printf 'LOG_DIR=%s\n' "$LOG_DIR"
    printf 'DXVK_LOG_DIR=%s\n' "$DXVK_LOG_DIR"
    printf '\nWorking tree status:\n'
    git -C "$REPO_ROOT" status --short 2>/dev/null || true
} > "$OUTPUT_DIR/system/collection-info.txt" 2>&1

sw_vers > "$OUTPUT_DIR/system/macos-version.txt" 2>&1 || true
uname -a > "$OUTPUT_DIR/system/uname.txt" 2>&1 || true
sysctl -n hw.memsize > "$OUTPUT_DIR/system/memory-bytes.txt" 2>&1 || true
system_profiler SPDisplaysDataType > "$OUTPUT_DIR/system/displays.txt" 2>&1 || true
ps -axo pid,ppid,%cpu,%mem,etime,command > "$OUTPUT_DIR/system/processes.txt" 2>&1 || true

# Main launcher and native encoder/bridge logs.
for name in \
    alvr-bridge.log hardware-encoder.log steamvr-launch.log \
    steam-client-launch.log half-life-alyx.log steamvr-app-*.log; do
    for source in "$LOG_DIR/$name"; do
        [[ -f "$source" ]] || continue
        copy_file "$source" "repo/$(basename "$source")"
    done
done

# SteamVR logs: collect from every plausible prefix and namespace by bottle.
STEAM_ROOTS=()
add_steam_root() {
    local root="${1%/}" existing
    [[ -d "$root/logs" ]] || return 0
    for existing in "${STEAM_ROOTS[@]}"; do
        [[ "$existing" == "$root" ]] && return 0
    done
    STEAM_ROOTS+=("$root")
}
add_steam_root "$STEAM_ROOT"
for prefix in "${WINE_PREFIX_CANDIDATES[@]}"; do
    add_steam_root "$prefix/drive_c/Program Files (x86)/Steam"
    add_steam_root "$prefix/drive_c/Program Files/Steam"
done
for root in "${STEAM_ROOTS[@]}"; do
    prefix_label="$(basename "$(dirname "$(dirname "$(dirname "$root")")")" | tr -cs '[:alnum:]_.-' '_')"
    target_dir="steam/$prefix_label"
    for name in vrcompositor.txt vrserver.txt connection_log.txt \
        webhelper.txt steamui_html.txt console_log.txt cef_log.txt; do
        copy_file "$root/logs/$name" "$target_dir/$name"
    done
    copy_glob "$target_dir" "$root/logs/vrcompositor*.txt" "$root/logs/vrserver*.txt"
done

# DXVK logs may reside beside the runtime drive rather than the ZIP download.
copy_glob dxvk "$DXVK_LOG_DIR/vrcompositor*.log" "$DXVK_LOG_DIR/vrserver*.log"
copy_glob dxvk "$VERANTYX_VR_DRIVE/logs/dxvk/vrcompositor*.log" "$VERANTYX_VR_DRIVE/logs/dxvk/vrserver*.log"

# Driver and capture-proxy logs from all detected prefixes.
for prefix in "${WINE_PREFIX_CANDIDATES[@]}"; do
    prefix_label="$(basename "$prefix" | tr -cs '[:alnum:]_.-' '_')"
    copy_file "$prefix/drive_c/vr_shared_frame.log" "wine-prefix/$prefix_label-vr_shared_frame.log"
    copy_file "$prefix/drive_c/vr_openvr_capture_proxy.log" "wine-prefix/$prefix_label-vr_openvr_capture_proxy.log"
    copy_glob "wine-prefix/$prefix_label" "$prefix/drive_c/"*verantyx*.log
done

# MoltenVR runs a separate video_bridge in some setups; collect only log files.
MOLTENVR_SUPPORT="$HOME/Library/Application Support/MoltenVR"
copy_glob moltenvr "$MOLTENVR_SUPPORT/"*.log "$MOLTENVR_SUPPORT/alvr-config/"*.log
copy_glob moltenvr "$HOME/Library/Logs/MoltenVR/"*.log

# Include other top-level logs that are text-based diagnostics, but never the
# Steam config/login files or arbitrary files from the prefix.
copy_glob repo "$LOG_DIR/"*.log "$LOG_DIR/"*.txt

if ! command -v zip >/dev/null 2>&1; then
    echo "Collected logs in: $OUTPUT_DIR" >&2
    echo "The 'zip' utility is unavailable; compress that folder manually if needed." >&2
    exit 0
fi

zip -qr "$ARCHIVE" "$OUTPUT_DIR"
echo
echo "Diagnostics bundle created: $ARCHIVE"
echo "Review it before sharing; logs may contain local paths, IP addresses, or game details."

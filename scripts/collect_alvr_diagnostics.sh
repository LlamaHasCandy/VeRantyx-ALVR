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
# If STEAM_ROOT is not explicitly configured, prefer the prefix with the
# newest SteamVR/compositor logs. This avoids silently collecting an old Wine
# prefix when Steam is actually running from a CrossOver bottle.
if [[ -z "${STEAM_ROOT:-}" ]]; then
    STEAM_ROOT="$WINEPREFIX/drive_c/Program Files (x86)/Steam"
    best_log_epoch=0
    if [[ -d "$STEAM_ROOT/logs" ]]; then
        for log in "$STEAM_ROOT/logs/vrcompositor.txt" "$STEAM_ROOT/logs/vrserver.txt" "$STEAM_ROOT/logs/console_log.txt"; do
            [[ -f "$log" ]] || continue
            epoch="$(stat -f %m "$log" 2>/dev/null || echo 0)"
            (( epoch > best_log_epoch )) && best_log_epoch="$epoch"
        done
    fi

    CROSSOVER_BOTTLES="$HOME/Library/Application Support/CrossOver/Bottles"
    if [[ -d "$CROSSOVER_BOTTLES" ]]; then
        for candidate in "$CROSSOVER_BOTTLES"/*/drive_c/Program\ Files\ \(x86\)/Steam; do
            [[ -d "$candidate/logs" ]] || continue
            candidate_epoch=0
            for log in "$candidate/logs/vrcompositor.txt" "$candidate/logs/vrserver.txt" "$candidate/logs/console_log.txt"; do
                [[ -f "$log" ]] || continue
                epoch="$(stat -f %m "$log" 2>/dev/null || echo 0)"
                (( epoch > candidate_epoch )) && candidate_epoch="$epoch"
            done
            if (( candidate_epoch > best_log_epoch )); then
                STEAM_ROOT="$candidate"
                # The bottle root is the directory immediately above drive_c.
                WINEPREFIX="${candidate%/drive_c/Program Files (x86)/Steam}"
                best_log_epoch="$candidate_epoch"
            fi
        done
    fi
fi
STEAM_LOG_DIR="$STEAM_ROOT/logs"
LOG_DIR="${LOG_DIR:-$REPO_ROOT/logs}"
DXVK_LOG_DIR="${DXVK_LOG_DIR:-$LOG_DIR/dxvk}"

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

# SteamVR/Wine logs most useful for error 461 and delayed compositor exits.
for name in vrcompositor.txt vrserver.txt connection_log.txt \
    webhelper.txt steamui_html.txt console_log.txt cef_log.txt; do
    copy_file "$STEAM_LOG_DIR/$name" "steam/$name"
done
copy_glob steam "$STEAM_LOG_DIR/vrcompositor*.txt" "$STEAM_LOG_DIR/vrserver*.txt"
copy_glob dxvk "$DXVK_LOG_DIR/vrcompositor*.log" "$DXVK_LOG_DIR/vrserver*.log"

# Driver and capture-proxy diagnostics live in the prefix's drive_c folder.
copy_file "$WINEPREFIX/drive_c/vr_shared_frame.log" "wine-prefix/vr_shared_frame.log"
copy_file "$WINEPREFIX/drive_c/vr_openvr_capture_proxy.log" "wine-prefix/vr_openvr_capture_proxy.log"
copy_glob wine-prefix "$WINEPREFIX/drive_c/"*verantyx*.log

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

#!/bin/bash
set -euo pipefail

VERANTYX_VR_DRIVE="${VERANTYX_VR_DRIVE:-$HOME/Verantyx_VR_Drive}"
if [[ -n "${CROSSOVER_BOTTLE:-}" ]]; then
    WINEPREFIX="${WINEPREFIX:-$HOME/Library/Application Support/CrossOver/Bottles/$CROSSOVER_BOTTLE}"
else
    WINEPREFIX="${WINEPREFIX:-$VERANTYX_VR_DRIVE/SteamVR_Prefix_Wine11_Test}"
fi
WINE_RUNTIME_BIN="${WINE_RUNTIME_BIN:-$VERANTYX_VR_DRIVE/Beer/wine/WineStaging.app/Contents/Resources/wine/bin/wine}"
GPTK_ROOT="${GPTK_ROOT:-$VERANTYX_VR_DRIVE/gptk_wine_local}"
WINE_WRAPPER="${WINE_WRAPPER:-$VERANTYX_VR_DRIVE/wine_wrapper}"

export WINEPREFIX

# CrossOver mode uses CodeWeavers' supported --bottle/--cx-app interface.
# Keep its runner separate from the project's Wine 11 environment overrides.
if [[ -n "${CROSSOVER_BOTTLE:-}" ]]; then
    CROSSOVER_WINE_BIN="${CROSSOVER_WINE_BIN:-/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine}"
    if [[ ! -x "$CROSSOVER_WINE_BIN" ]]; then
        echo "CrossOver's wine launcher was not found: $CROSSOVER_WINE_BIN" >&2
        echo "Set CROSSOVER_WINE_BIN to CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine." >&2
        exit 1
    fi
    if (($# == 0)); then
        echo "CrossOver mode needs a Windows executable to launch." >&2
        exit 2
    fi
    crossover_app="$1"
    case "$crossover_app" in
        reg) crossover_app="reg.exe" ;;
        wineboot) crossover_app="wineboot.exe" ;;
    esac
    if [[ "$crossover_app" == "$WINEPREFIX/drive_c/"* ]]; then
        crossover_relative="${crossover_app#"$WINEPREFIX/drive_c/"}"
        crossover_app="C:\\$(printf '%s' "$crossover_relative" | sed 's|/|\\|g')"
    fi
    crossover_options=(--bottle "$CROSSOVER_BOTTLE" --cx-app "$crossover_app")
    [[ -z "${WINEDLLOVERRIDES:-}" ]] || crossover_options+=(--dll "$WINEDLLOVERRIDES")
    [[ -z "${WINEDEBUG:-}" ]] || crossover_options+=(--debugmsg "$WINEDEBUG")
    exec "$CROSSOVER_WINE_BIN" "${crossover_options[@]}" "${@:2}"
fi

export WINEDEBUG="${WINEDEBUG:--all,err+all}"
export WINEESYNC=1
export WINEDLLOVERRIDES="dxgi,d3d9,d3d10core,d3d11=n,b;${WINEDLLOVERRIDES:-}"
export D3D11_ENABLE=1
export D3DMETAL_ENABLE=1
export D3DMetal=1

if [[ -x "$WINE_RUNTIME_BIN" ]]; then
    RUNTIME_ROOT="$(cd "$(dirname "$WINE_RUNTIME_BIN")/.." && pwd)"
    export PATH="$RUNTIME_ROOT/bin:$PATH"
    export DYLD_FALLBACK_LIBRARY_PATH="$RUNTIME_ROOT/lib:$GPTK_ROOT/lib:/usr/lib"
    export DYLD_LIBRARY_PATH="$RUNTIME_ROOT/lib:$RUNTIME_ROOT/lib/external:$GPTK_ROOT/lib/external:${MOLTENVK_ENV:-$VERANTYX_VR_DRIVE/moltenvk_env}:${DYLD_LIBRARY_PATH:-}"
    version="$("$WINE_RUNTIME_BIN" --version 2>/dev/null || true)"
    echo "Using local Windows runtime: ${version:-$WINE_RUNTIME_BIN}"
    exec "$WINE_RUNTIME_BIN" "$@"
fi

if [[ ! -x "$WINE_WRAPPER" ]]; then
    echo "Local Wine 11 runtime not found: $WINE_RUNTIME_BIN" >&2
    echo "Custom Windows runtime wrapper not found or not executable: $WINE_WRAPPER" >&2
    exit 1
fi
if [[ ! -x "$GPTK_ROOT/bin/wine64" && ! -x "$GPTK_ROOT/bin/wine" ]]; then
    echo "Custom GPTK runtime not found under: $GPTK_ROOT/bin" >&2
    echo "Set WINE_RUNTIME_BIN or GPTK_ROOT to the project runtime." >&2
    exit 1
fi

export PATH="$GPTK_ROOT/bin:$PATH"
export DYLD_FALLBACK_LIBRARY_PATH="$GPTK_ROOT/lib:/usr/lib"
export DYLD_LIBRARY_PATH="$GPTK_ROOT/lib/external:${MOLTENVK_ENV:-$VERANTYX_VR_DRIVE/moltenvk_env}:${DYLD_LIBRARY_PATH:-}"
echo "Using legacy custom runtime wrapper: $WINE_WRAPPER" >&2
exec "$WINE_WRAPPER" "$@"

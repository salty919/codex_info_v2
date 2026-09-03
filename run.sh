#!/usr/bin/env bash
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only

# This source is copied byte-for-byte into each verified Linux generation.
# It never builds, copies, or executes a checkout binary. User-facing help
# comes from the installed Rust payload so the i18n catalog remains the only
# natural-language authority.
set -euo pipefail

operation='run'
case "$#" in
    0) operation='start' ;;
    1)
        case "$1" in
            --start) operation='start' ;;
            --ui) operation='ui' ;;
            --stop) operation='stop' ;;
            --disable-autostart) operation='disable-autostart' ;;
            --remove) operation='remove' ;;
            --status) operation='status' ;;
            --update) operation='update' ;;
            --help) operation='help' ;;
            *) printf 'codex-info: E_LAUNCHER_ARGUMENT\n' >&2; exit 2 ;;
        esac
        ;;
    *) printf 'codex-info: E_LAUNCHER_ARGUMENT\n' >&2; exit 2 ;;
esac

home_dir="${HOME:-}"
[[ -n "$home_dir" ]] || { printf 'codex-info: E_HOME_REQUIRED\n' >&2; exit 1; }
payload="$home_dir/.local/bin/codex_info"
installer="$home_dir/.local/libexec/codex-info-install.sh"

require_payload() {
    [[ -L "$payload" && "$(readlink -- "$payload")" == '../share/codex-info/current/codex_info' && -x "$payload" ]] || {
        printf 'codex-info: E_PAYLOAD_UNAVAILABLE\n' >&2
        exit 1
    }
}

# --help is a side-effect-free payload readback. The exact marker asks the
# installed payload for launcher help rather than its raw service/development
# CLI help. It deliberately does not initialize the installer, acquire L1, or
# invoke any repository executable.
if [[ "$operation" == help ]]; then
    require_payload
    export CODEX_INFO_LAUNCHER_HELP=1
    exec "$payload" --help
fi

launcher_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repository_installer="$launcher_dir/packaging/install_linux_bundle.sh"
installer_ready=0
if [[ -L "$installer" && "$(readlink -- "$installer")" == '../share/codex-info/current/install.sh' && -x "$installer" ]]; then
    installer_ready=1
fi

# A repository checkout can be older than the first generation-aware install.
# In that one-way bootstrap state, the repository installer validates the
# legacy flat manifest/binary/units and reacquires the exact Release archive;
# no checkout binary is ever used as a runtime fallback.
if (( ! installer_ready )); then
    [[ -f "$repository_installer" && ! -L "$repository_installer" && -x "$repository_installer" ]] || {
        printf 'codex-info: E_INSTALLER_UNAVAILABLE\n' >&2
        exit 1
    }
    case "$operation" in
        ui)
            if ! "$repository_installer" --start; then
                "$repository_installer" --verify-ui --quiet || {
                    printf 'codex-info: E_UI_SOURCE_UNAVAILABLE\n' >&2
                    exit 1
                }
                require_payload
                export CODEX_INFO_UI_CLIENT_ONLY=1
                exec "$payload" --ui
            fi
            ;;
        start|stop|disable-autostart|remove|status|update)
            exec "$repository_installer" "--$operation"
            ;;
    esac
    [[ -L "$installer" && "$(readlink -- "$installer")" == '../share/codex-info/current/install.sh' && -x "$installer" ]] || {
        printf 'codex-info: E_HANDOFF_UNAVAILABLE\n' >&2
        exit 1
    }
fi

case "$operation" in
    start|stop|disable-autostart|remove|status|update)
        exec "$installer" "--$operation"
        ;;
esac

# Start/readiness failure is allowed to surface in the verified UI client as a
# localized connection-failure/retry screen, but only after the installer has
# proved that generation and owner identity are safe. The client-only marker
# prevents the UI payload from creating a raw listener or recorder of its own.
if ! "$installer" --start; then
    "$installer" --verify-ui --quiet || {
        printf 'codex-info: E_UI_SOURCE_UNAVAILABLE\n' >&2
        exit 1
    }
    require_payload
    export CODEX_INFO_UI_CLIENT_ONLY=1
    exec "$payload" --ui
fi

require_payload
unset WAYLAND_DISPLAY WAYLAND_SOCKET WINIT_X11_SCALE_FACTOR
export LIBGL_ALWAYS_SOFTWARE='1'
export MESA_LOADER_DRIVER_OVERRIDE='llvmpipe'
exec "$payload" --ui

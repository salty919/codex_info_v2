#!/usr/bin/env bash
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only

# This source is copied byte-for-byte into each verified Linux generation.
# It never builds, copies, or executes a checkout binary. User-facing help
# comes from the installed Rust payload so the i18n catalog remains the only
# natural-language authority.
set -euo pipefail

operation='run'
migration_option=()
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
    2)
        [[ "$1" == --update && "$2" == --migrate-recorder-override ]] || {
            printf 'codex-info: E_LAUNCHER_ARGUMENT\n' >&2; exit 2;
        }
        operation='update'
        migration_option=(--migrate-recorder-override)
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

launch_verified_ui() {
    exec python3 - "$payload" <<'PY_UI_STARTUP'
import os
import selectors
import threading
import subprocess
import sys
import tempfile
import time

with tempfile.TemporaryDirectory(prefix="codex-info-ui-") as directory:
    fifo = os.path.join(directory, "startup")
    os.mkfifo(fifo, 0o600)
    ready_fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
    environment = dict(os.environ, CODEX_INFO_UI_STARTUP_FIFO=fifo)
    child = subprocess.Popen(
        [sys.argv[1], "--ui"], env=environment, start_new_session=True,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=None if sys.stderr.isatty() else subprocess.DEVNULL,
    )
    exit_read_fd, exit_write_fd = os.pipe()
    def observe_exit():
        os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOWAIT)
        os.write(exit_write_fd, b"X")
    threading.Thread(target=observe_exit, daemon=True).start()
    try:
        with selectors.DefaultSelector() as events:
            events.register(ready_fd, selectors.EVENT_READ, "ready")
            events.register(exit_read_fd, selectors.EVENT_READ, "exit")
            deadline = time.monotonic() + 30
            received = b""
            while True:
                pending = events.select(max(0, deadline - time.monotonic()))
                if any(key.data == "ready" for key, _ in pending):
                    received += os.read(ready_fd, 16)
                    if received == b"READY\n":
                        sys.exit(0)
                    if b"\n" in received or len(received) > 6:
                        break
                if any(key.data == "exit" for key, _ in pending):
                    status = child.wait()
                    print(f"codex-info: E_UI_STARTUP_FAILED (exit={status})", file=sys.stderr)
                    sys.exit(status if 0 < status < 126 else 1)
                if time.monotonic() >= deadline:
                    break
        print("codex-info: E_UI_STARTUP_UNCONFIRMED", file=sys.stderr)
        child.terminate()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            print("codex-info: E_UI_STARTUP_CHILD_DID_NOT_EXIT", file=sys.stderr)
        sys.exit(1)
    finally:
        os.close(exit_read_fd)
        os.close(exit_write_fd)
        os.close(ready_fd)
PY_UI_STARTUP
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

if [[ "$operation" == ui && -s "$home_dir/.local/share/codex-info/last-update-failure.txt" ]]; then
    printf 'codex-info: last update failed: ' >&2
    cat -- "$home_dir/.local/share/codex-info/last-update-failure.txt" >&2 || true
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
                launch_verified_ui
            fi
            ;;
        start|stop|disable-autostart|remove|status|update)
            exec "$repository_installer" "--$operation" "${migration_option[@]}"
            ;;
    esac
    [[ -L "$installer" && "$(readlink -- "$installer")" == '../share/codex-info/current/install.sh' && -x "$installer" ]] || {
        printf 'codex-info: E_HANDOFF_UNAVAILABLE\n' >&2
        exit 1
    }
fi

case "$operation" in
    start|stop|disable-autostart|remove|status|update)
        exec "$installer" "--$operation" "${migration_option[@]}"
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
    launch_verified_ui
fi

require_payload
unset WAYLAND_DISPLAY WAYLAND_SOCKET WINIT_X11_SCALE_FACTOR
export LIBGL_ALWAYS_SOFTWARE='1'
export MESA_LOADER_DRIVER_OVERRIDE='llvmpipe'
# The installed UI is always a REST client.  It must never recreate the
# retired combined recorder/REST owner if the REST process restarts between
# installer readiness and UI launch.
export CODEX_INFO_UI_CLIENT_ONLY=1
launch_verified_ui

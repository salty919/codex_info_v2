#!/usr/bin/env bash
set -euo pipefail

# Verify the real (non-preview) Linux UI against the resident REST service.
# The app-server is a bounded local fixture, so no account or network is used.
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"
hold() { echo "x11-service-recovery-visual-gate: HOLD: $*" >&2; exit 2; }
fail() { echo "x11-service-recovery-visual-gate: FAIL: $*" >&2; exit 1; }
[[ -n "${DISPLAY:-}" ]] || hold 'DISPLAY is unavailable'
for command in cp curl python3 readlink xprop xwd xwininfo; do
    command -v "$command" >/dev/null 2>&1 || hold "$command is unavailable"
done
default_binary="$root_dir/target/release/codex_info"
binary="${CODEX_INFO_ACCEPTANCE_BINARY:-$default_binary}"
[[ "$binary" == /* ]] || binary="$root_dir/$binary"
binary_dir="$(cd -- "$(dirname -- "$binary")" 2>/dev/null && pwd -P)" \
    || fail 'acceptance binary parent directory is unavailable'
binary="$binary_dir/$(basename -- "$binary")"
recorder_binary="$binary_dir/codex_info_recorder"
rest_binary="$binary_dir/codex_info_rest"
if [[ "$binary" == "$default_binary" &&
      ( ! -x "$binary" || ! -x "$recorder_binary" || ! -x "$rest_binary" ) ]]; then
    command -v cargo >/dev/null 2>&1 || fail 'cargo is required to build missing release binaries'
    cargo build --release --locked \
        -p codex_info -p codex-info-recorder -p codex-info-rest
fi
for release_binary in "$binary" "$recorder_binary" "$rest_binary"; do
    [[ -f "$release_binary" && -x "$release_binary" && ! -L "$release_binary" ]] \
        || fail "acceptance binary is not an executable regular file: $release_binary"
done

# The product rejects executables below world/group-writable ancestors.  Keep
# the fixture below the checked-out repository (whose ancestors are trusted)
# instead of /tmp, which is normally mode 1777 on Linux.
temp_parent="$root_dir"
temp_root="$(mktemp -d "$temp_parent/.codex-info-x11-recovery.XXXXXX")"
case "$temp_root" in
    "$temp_parent"/.codex-info-x11-recovery.*) ;;
    *) fail "unexpected temporary path: $temp_root" ;;
esac
service_pid=''
recorder_pid=''
ui_pid=''
reference_ui_pid=''
holder_pid=''
window_id=''
reference_window_id=''
graph_window_id=''
port=''
frame="$temp_root/frame.xwd"
graph_frame="$temp_root/graph.xwd"
ready_frame="$temp_root/ready.xwd"
ready_current="$temp_root/ready-current.json"
ready_threads="$temp_root/ready-threads.json"
current_headers="$temp_root/ready-current.headers"
threads_headers="$temp_root/ready-threads.headers"
reference_frame="$temp_root/thread-summary-reference.xwd"
thread_reference_dir="$temp_root/thread-summary-reference"
service_starttime=''
recorder_starttime=''
ui_starttime=''
reference_ui_starttime=''
holder_starttime=''
holder_exe="$temp_root/holder/codex"

proc_starttime() {
    local pid="$1"
    awk '{print $22}' "/proc/$pid/stat" 2>/dev/null || true
}

click_window() {
    python3 - "$1" "$2" "$3" <<'PY'
import ctypes
import sys
import time

window, x, y = (int(value, 0) for value in sys.argv[1:])
x11 = ctypes.CDLL("libX11.so.6")
xtst = ctypes.CDLL("libXtst.so.6")
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XRaiseWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XWarpPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong,
                             ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint,
                             ctypes.c_int, ctypes.c_int]
x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
xtst.XTestFakeButtonEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

display = x11.XOpenDisplay(None)
if not display:
    raise SystemExit("X display is unavailable")
try:
    x11.XRaiseWindow(display, window)
    x11.XWarpPointer(display, 0, window, 0, 0, 0, 0, x, y)
    x11.XSync(display, 0)
    time.sleep(0.05)
    if not xtst.XTestFakeButtonEvent(display, 1, 1, 0):
        raise SystemExit("X button press failed")
    if not xtst.XTestFakeButtonEvent(display, 1, 0, 0):
        raise SystemExit("X button release failed")
    x11.XSync(display, 0)
finally:
    x11.XCloseDisplay(display)
PY
}

write_thread_summary_reference() {
    local frame_path="$1"
    python3 - "$frame_path" "$thread_reference_dir" <<'PY'
import pathlib
import struct
import sys

data = pathlib.Path(sys.argv[1]).read_bytes()
target = pathlib.Path(sys.argv[2])
header = struct.unpack(">25I", data[:100])
header_size, width, height, bytes_per_line, colors = (
    header[0], header[4], header[5], header[12], header[19]
)
if (width, height) != (900, 480):
    raise SystemExit(f"unexpected reference image size: {width}x{height}")
offset = header_size + colors * 12
stride = bytes_per_line // width
if stride < 3:
    raise SystemExit("reference image pixel stride is invalid")

components = {
    "total": ((50, 264, 126, 288), (86, 178, 245)),
    "sol": ((182, 268, 258, 290), (245, 247, 251)),
    "terra": ((270, 268, 354, 290), (245, 247, 251)),
    "luna": ((366, 268, 442, 290), (245, 247, 251)),
    "astra": ((454, 268, 538, 290), (245, 247, 251)),
    "other": ((550, 268, 642, 290), (245, 247, 251)),
}

def rgb(x, y):
    index = offset + y * bytes_per_line + x * stride
    return data[index + 2], data[index + 1], data[index]

masks = {}
for name, (rect, color) in components.items():
    left, top, right, bottom = rect
    mask = bytes(
        int(sum((rgb(x, y)[index] - color[index]) ** 2 for index in range(3)) <= 70 ** 2)
        for y in range(top, bottom)
        for x in range(left, right)
    )
    foreground = sum(mask)
    if foreground < 8:
        raise SystemExit(f"reference {name} component is empty: {foreground}")
    masks[name] = mask

target.mkdir()
for name, mask in masks.items():
    (target / f"{name}.mask").write_bytes(mask)
PY
}

assert_thread_summary_components() {
    local frame_path="$1"
    python3 - "$frame_path" "$thread_reference_dir" <<'PY'
import pathlib
import struct
import sys

data = pathlib.Path(sys.argv[1]).read_bytes()
reference = pathlib.Path(sys.argv[2])
header = struct.unpack(">25I", data[:100])
header_size, width, height, bytes_per_line, colors = (
    header[0], header[4], header[5], header[12], header[19]
)
if (width, height) != (900, 480):
    raise SystemExit(f"unexpected real-service image size: {width}x{height}")
offset = header_size + colors * 12
stride = bytes_per_line // width
if stride < 3:
    raise SystemExit("real-service image pixel stride is invalid")

components = {
    "total": ((50, 264, 126, 288), (86, 178, 245)),
    "sol": ((182, 268, 258, 290), (245, 247, 251)),
    "terra": ((270, 268, 354, 290), (245, 247, 251)),
    "luna": ((366, 268, 442, 290), (245, 247, 251)),
    "astra": ((454, 268, 538, 290), (245, 247, 251)),
    "other": ((550, 268, 642, 290), (245, 247, 251)),
}

def rgb(x, y):
    index = offset + y * bytes_per_line + x * stride
    return data[index + 2], data[index + 1], data[index]

for name, (rect, color) in components.items():
    expected_path = reference / f"{name}.mask"
    if not expected_path.is_file():
        raise SystemExit(f"missing reference component: {name}")
    left, top, right, bottom = rect
    actual = bytes(
        int(sum((rgb(x, y)[index] - color[index]) ** 2 for index in range(3)) <= 70 ** 2)
        for y in range(top, bottom)
        for x in range(left, right)
    )
    expected = expected_path.read_bytes()
    if not expected or sum(expected) < 8 or len(expected) != len(actual):
        raise SystemExit(f"invalid reference component: {name}")
    if actual != expected:
        changed = sum(left != right for left, right in zip(actual, expected))
        raise SystemExit(f"thread summary component differs: {name}, changed={changed}")

values = [1, 1, 0, 0, 0, 0]
if sum(values[1:]) != values[0]:
    raise SystemExit("thread summary fixture invariant is invalid")
print(f"x11-service-recovery-visual-gate: thread summary PASS {values}")
PY
}

assert_threads_window_closed() {
    local owner_pid="$1" expected_main_window="$2"
    local candidate candidate_pid candidate_title owned_window_count=0
    while read -r candidate; do
        candidate_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
        [[ "$candidate_pid" == "$owner_pid" ]] || continue
        owned_window_count=$((owned_window_count + 1))
        if [[ "$candidate" != "$expected_main_window" ]]; then
            candidate_title="$(xprop -id "$candidate" _NET_WM_NAME WM_NAME 2>/dev/null || true)"
            fail "Threads-closed fixture has another product window: id=$candidate title=$candidate_title"
        fi
    done < <(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+/ { print $1 }')
    ((owned_window_count == 1)) \
        || fail "Threads-closed fixture expected one Main window, found $owned_window_count"
}

terminate_owned() {
    local pid="$1" label="$2" expected_starttime="$3" expected_exe="$4"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return 0
    fi
    [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$expected_exe" ]] || {
        echo "x11-service-recovery-visual-gate: refusing to terminate unowned $label PID $pid" >&2
        return 1
    }
    [[ "$expected_starttime" =~ ^[0-9]+$ && "$(proc_starttime "$pid")" == "$expected_starttime" ]] || {
        echo "x11-service-recovery-visual-gate: refusing to terminate reused $label PID $pid" >&2
        return 1
    }
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$expected_exe" ]] || return 1
        [[ "$(proc_starttime "$pid")" == "$expected_starttime" ]] || return 1
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

terminate_holder() {
    local pid="$holder_pid" expected_starttime="$holder_starttime"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return 0
    fi
    [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$holder_exe" ]] || {
        echo "x11-service-recovery-visual-gate: refusing to terminate unowned holder PID $pid" >&2
        return 1
    }
    [[ "$expected_starttime" =~ ^[0-9]+$ && "$(proc_starttime "$pid")" == "$expected_starttime" ]] || {
        echo "x11-service-recovery-visual-gate: refusing to terminate reused holder PID $pid" >&2
        return 1
    }
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$holder_exe" ]] || return 1
        [[ "$(proc_starttime "$pid")" == "$expected_starttime" ]] || return 1
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup_resources() {
    local cleanup_failed=0
    if ! terminate_owned "$reference_ui_pid" reference-UI "$reference_ui_starttime" "$binary"; then
        cleanup_failed=1
    fi
    if ! terminate_owned "$ui_pid" UI "$ui_starttime" "$binary"; then
        cleanup_failed=1
    fi
    if ! terminate_owned "$service_pid" REST "$service_starttime" "$rest_binary"; then
        cleanup_failed=1
    fi
    if ! terminate_owned "$recorder_pid" recorder "$recorder_starttime" "$recorder_binary"; then
        cleanup_failed=1
    fi
    if ! terminate_holder; then
        cleanup_failed=1
    fi
    ((cleanup_failed == 0)) || return 1
    case "$temp_root" in
        "$temp_parent"/.codex-info-x11-recovery.*) rm -rf -- "$temp_root" || return 1 ;;
        *)
            echo 'x11-service-recovery-visual-gate: refusing unexpected cleanup' >&2
            return 1
            ;;
    esac
}

cleanup_on_exit() {
    local prior_status="$?"
    trap - EXIT
    if ! cleanup_resources; then
        echo "x11-service-recovery-visual-gate: HOLD: cleanup ownership could not be proven; retained $temp_root" >&2
        exit 2
    fi
    exit "$prior_status"
}
trap cleanup_on_exit EXIT

mkdir -p "$temp_root"/{home,config,data,cache,state,runtime,codex/sessions,holder}
chmod 700 "$temp_root/runtime" "$temp_root/codex" "$temp_root/holder"
auth_fixture="$temp_root/codex/auth.json"
cat >"$auth_fixture" <<'JSON'
{"auth_mode":"chatgpt","tokens":{"account_id":"fixture-account-129"}}
JSON
chmod 600 "$auth_fixture"
# The normal isolated app-server path snapshots Codex's existing state index.
# Keep this fixture on that path instead of accidentally exercising the
# one-cycle global fallback used only when isolation preparation fails.
state_fixture="$temp_root/codex/state_5.sqlite"
python3 - "$state_fixture" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
connection.execute("PRAGMA user_version = 1")
connection.close()
PY
chmod 600 "$state_fixture"
fake_codex="$root_dir/scripts/fake_codex_app_server.py"

# Seed pre-boundary records that must be baselined without attribution. A
# verified append after the first ready generation supplies the visible model
# rows; this keeps the rendered fixture aligned with SESSION-129.
session_fixture="$temp_root/codex/sessions/fixture.jsonl"
python3 - "$session_fixture" <<'PY'
import datetime
import json
import sys
import time

timestamp = datetime.datetime.fromtimestamp(
    time.time() - 120, datetime.timezone.utc
).isoformat().replace("+00:00", "Z")
events = [
    ("gpt-5.6-sol", 1_000, 700, 100, 200),
    ("gpt-5.6-terra", 2_000, 1_400, 200, 400),
    ("gpt-5.6-luna", 3_000, 2_100, 300, 600),
]
with open(sys.argv[1], "w", encoding="utf-8") as stream:
    stream.write(json.dumps({
        "type": "session_meta",
        "payload": {"id": "fixture-thread"},
    }) + "\n")
    for model, total, input_tokens, cached, output in events:
        stream.write(json.dumps({"type": "thread_context", "model": model}) + "\n")
        stream.write(json.dumps({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "total_tokens": total,
                        "input_tokens": input_tokens,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                    }
                },
            },
        }) + "\n")
    # The last context is the active-thread model authority. Keep it SOL and
    # leave the lifecycle open so the exact one-thread Main fixture is live.
    stream.write(json.dumps({"type": "thread_context", "model": "gpt-5.6-sol"}) + "\n")
    stream.write(json.dumps({"type": "task_started"}) + "\n")
PY
chmod 600 "$session_fixture"

holder_source="$(readlink -f "$(command -v python3)")"
[[ -x "$holder_source" && ! -L "$holder_source" ]] || fail 'could not resolve the holder executable'
cp -- "$holder_source" "$holder_exe"
chmod 700 "$holder_exe"
"$holder_exe" -c 'import sys,time; stream=open(sys.argv[1], "rb"); time.sleep(3600)' \
    "$session_fixture" >"$temp_root/holder.log" 2>&1 &
holder_pid="$!"
holder_starttime="$(proc_starttime "$holder_pid")"
[[ "$holder_starttime" =~ ^[0-9]+$ ]] || fail 'holder starttime could not be recorded'
holder_ready=0
for _ in $(seq 1 30); do
    if [[ "$(tr -d '\n' <"/proc/$holder_pid/comm" 2>/dev/null || true)" == "codex" \
        && "$(readlink "/proc/$holder_pid/exe" 2>/dev/null || true)" == "$holder_exe" \
        && "$(proc_starttime "$holder_pid")" == "$holder_starttime" ]]; then
        for holder_fd in "/proc/$holder_pid/fd"/*; do
            if [[ "$(readlink -f "$holder_fd" 2>/dev/null || true)" == "$session_fixture" ]]; then
                holder_ready=1
                break
            fi
        done
    fi
    ((holder_ready == 1)) && break
    sleep 0.1
done
((holder_ready == 1)) || fail 'exact codex holder identity or session fd was not established'

port="$(python3 - <<'PY'
import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"
[[ -n "$port" ]] || fail 'could not find an unused loopback port'
fixture_reset_at=$(($(date +%s) + 604800 - 3600))
common_env=(
    "HOME=$temp_root/home"
    "XDG_CONFIG_HOME=$temp_root/config"
    "XDG_DATA_HOME=$temp_root/data"
    "XDG_CACHE_HOME=$temp_root/cache"
    "XDG_STATE_HOME=$temp_root/state"
    "XDG_RUNTIME_DIR=$temp_root/runtime"
    "CODEX_HOME=$temp_root/codex"
    "CODEX_INFO_DATA_DIR=$temp_root/data"
    "CODEX_INFO_CODEX_BIN=$fake_codex"
    "CODEX_INFO_FAKE_RESET_AT=$fixture_reset_at"
    "CODEX_INFO_FAKE_FAILURE_FILE=$temp_root/app-server-failure"
    "CODEX_INFO_FAKE_THREAD_ID=fixture-thread"
    "CODEX_INFO_FAKE_THREAD_PATH=$session_fixture"
    "CODEX_INFO_FAKE_THREAD_TITLE=fixture active thread"
    "CODEX_INFO_DAEMON_INTERVAL_SECS=2"
)
run_with_common_env() {
    env -u CODEX_INFO_PREVIEW -u CODEX_INFO_PREVIEW_SIZE "${common_env[@]}" "$@"
}
launch_service() {
    env -u CODEX_INFO_PREVIEW -u CODEX_INFO_PREVIEW_SIZE "${common_env[@]}" "$rest_binary" --port "$port" \
        >"$temp_root/service-$RANDOM.log" 2>&1 &
    service_pid="$!"
    service_starttime="$(proc_starttime "$service_pid")"
    [[ "$service_starttime" =~ ^[0-9]+$ ]] || fail 'resident service starttime could not be recorded'
}
launch_recorder() {
    env -u CODEX_INFO_PREVIEW -u CODEX_INFO_PREVIEW_SIZE "${common_env[@]}" \
        "$recorder_binary" --sessions-root "$temp_root/codex/sessions" --interval-secs 2 \
        >"$temp_root/recorder.log" 2>&1 &
    recorder_pid="$!"
    recorder_starttime="$(proc_starttime "$recorder_pid")"
    [[ "$recorder_starttime" =~ ^[0-9]+$ ]] \
        || fail 'recorder starttime could not be recorded'
}
recorder_ready() {
    kill -0 "$recorder_pid" 2>/dev/null || return 1
    [[ "$(readlink "/proc/$recorder_pid/exe" 2>/dev/null || true)" == "$recorder_binary" ]] \
        || return 1
    python3 - "$temp_root/data" <<'PY'
import pathlib
import sqlite3
import sys

databases = list(pathlib.Path(sys.argv[1]).glob(
    "history/accounts/v1/*/epoch-*/usage_history.sqlite3"
))
if len(databases) != 1:
    raise SystemExit(1)
try:
    connection = sqlite3.connect(
        f"file:{databases[0]}?mode=ro", uri=True, timeout=0.2
    )
    count = connection.execute(
        "SELECT COUNT(*) FROM session_checkpoints"
    ).fetchone()[0]
except sqlite3.Error:
    raise SystemExit(1)
finally:
    if "connection" in locals():
        connection.close()
raise SystemExit(0 if count >= 1 else 1)
PY
}
wait_recorder_ready() {
    for _ in $(seq 1 80); do
        recorder_ready && return 0
        sleep 0.25
    done
    sed -n '1,160p' "$temp_root/recorder.log" >&2 2>/dev/null || true
    return 1
}
assert_recorder_unchanged() {
    kill -0 "$recorder_pid" 2>/dev/null \
        && [[ "$(readlink "/proc/$recorder_pid/exe" 2>/dev/null || true)" == "$recorder_binary" ]] \
        && [[ "$(proc_starttime "$recorder_pid")" == "$recorder_starttime" ]]
}
service_ready() {
    local details
    details="$(curl --fail --silent --show-error --max-time 1 "http://127.0.0.1:$port/v1/details" 2>/dev/null)" || return 1
    python3 - "$details" <<'PY'
import json
import sys
try:
    details = json.loads(sys.argv[1])
except (IndexError, json.JSONDecodeError):
    raise SystemExit(1)
raise SystemExit(0 if details.get("state") == "ready" and details.get("authenticated") is True else 1)
PY
}
wait_service_ready() {
    for _ in $(seq 1 80); do
        service_ready && return 0
        sleep 0.25
    done
    sed -n '1,160p' "$temp_root"/service-*.log >&2 2>/dev/null || true
    curl --silent --show-error --max-time 1 "http://127.0.0.1:$port/v1/details" >&2 || true
    return 1
}
append_verified_usage() {
    python3 - "$session_fixture" <<'PY'
import datetime
import json
import sys
import time

timestamp = datetime.datetime.fromtimestamp(
    time.time(), datetime.timezone.utc
).isoformat().replace("+00:00", "Z")
events = [
    ("gpt-5.6-sol", 4_000, 2_800, 400, 800),
    ("gpt-5.6-sol", 5_000, 3_500, 500, 1_000),
    ("gpt-5.6-terra", 6_000, 4_200, 600, 1_200),
    ("gpt-5.6-terra", 7_000, 4_900, 700, 1_400),
    ("gpt-5.6-luna", 8_000, 5_600, 800, 1_600),
    ("gpt-5.6-luna", 9_000, 6_300, 900, 1_800),
]
with open(sys.argv[1], "a", encoding="utf-8") as stream:
    for model, total, input_tokens, cached, output in events:
        stream.write(json.dumps({"type": "thread_context", "model": model}) + "\n")
        stream.write(json.dumps({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "total_tokens": total,
                        "input_tokens": input_tokens,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                    }
                },
            },
        }) + "\n")
    stream.write(json.dumps({"type": "thread_context", "model": "gpt-5.6-sol"}) + "\n")
    stream.write(json.dumps({"type": "task_started"}) + "\n")
PY
}
service_models_ready() {
    local details
    details="$(curl --fail --silent --show-error --max-time 1 "http://127.0.0.1:$port/v1/details" 2>/dev/null)" || return 1
    python3 - "$details" <<'PY'
import json
import sys
try:
    details = json.loads(sys.argv[1])
except (IndexError, json.JSONDecodeError):
    raise SystemExit(1)
raise SystemExit(0 if (
    details.get("state") == "ready"
    and details.get("authenticated") is True
    and len(details.get("models", [])) == 3
) else 1)
PY
}
wait_service_models_ready() {
    for _ in $(seq 1 80); do
        service_models_ready && return 0
        sleep 0.25
    done
    sed -n '1,160p' "$temp_root"/service-*.log >&2 2>/dev/null || true
    curl --silent --show-error --max-time 1 "http://127.0.0.1:$port/v1/details" >&2 || true
    return 1
}

service_thread_bundle_ready() {
    local current threads
    current="$(curl --fail --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/current" 2>/dev/null)" || return 1
    threads="$(curl --fail --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/threads" 2>/dev/null)" || return 1
    python3 - "$current" "$threads" <<'PY'
import json
import sys

try:
    current = json.loads(sys.argv[1])
    threads = json.loads(sys.argv[2])
except (IndexError, json.JSONDecodeError):
    raise SystemExit(1)
rows = threads.get("threads", [])
raise SystemExit(0 if (
    current.get("state") == "ready"
    and current.get("authenticated") is True
    and current.get("active_thread_count") == 1
    and len(rows) == 1
    and rows[0].get("id") == "fixture-thread"
    and rows[0].get("model") == "gpt-5.6-sol"
    and rows[0].get("model_label") == "gpt-5.6-sol"
) else 1)
PY
}

wait_service_thread_bundle_ready() {
    for _ in $(seq 1 80); do
        service_thread_bundle_ready && return 0
        sleep 0.25
    done
    sed -n '1,160p' "$temp_root"/service-*.log >&2 2>/dev/null || true
    curl --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/current" >&2 || true
    curl --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/threads" >&2 || true
    return 1
}

service_last_good_error() {
    local current
    current="$(curl --fail --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/current" 2>/dev/null)" || return 1
    python3 - "$current" "$ready_current" <<'PY'
import json
import os
import sys
try:
    current = json.loads(sys.argv[1])
    ready = json.loads(open(sys.argv[2], encoding="utf-8").read())
except (IndexError, json.JSONDecodeError):
    raise SystemExit(1)

# A recorder cycle that started before failure injection may still commit one
# valid generation afterwards.  That generation is the actual last-good root,
# so retain it as the comparison baseline while waiting for the first error.
if current.get("state") == "ready" and current.get("authenticated") is True:
    temporary = f"{sys.argv[2]}.tmp"
    with open(temporary, "w", encoding="utf-8") as stream:
        stream.write(sys.argv[1])
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, sys.argv[2])
    raise SystemExit(1)

raise SystemExit(0 if (
    current.get("state") == "error"
    and current.get("authenticated") is True
    and all(current.get(key) == ready.get(key) for key in (
        "observed_at", "plan_label", "quota", "models"
    ))
) else 1)
PY
}

wait_service_last_good_error() {
    for _ in $(seq 1 80); do
        service_last_good_error && return 0
        sleep 0.25
    done
    sed -n '1,160p' "$temp_root"/service-*.log >&2 2>/dev/null || true
    curl --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/current" >&2 || true
    return 1
}

launch_recorder
wait_recorder_ready || fail 'fixture recorder did not publish its first durable checkpoint'
launch_service
wait_service_ready || fail 'fixture-backed resident service did not publish ready details'
append_verified_usage
wait_service_models_ready || fail 'post-baseline fixture usage did not publish model details'
wait_service_thread_bundle_ready || fail 'resident service did not publish the exact one-SOL thread bundle'
curl --fail --silent --show-error --max-time 1 --dump-header "$current_headers" \
    --output "$ready_current" "http://127.0.0.1:$port/v3/current"
curl --fail --silent --show-error --max-time 1 --dump-header "$threads_headers" \
    --output "$ready_threads" "http://127.0.0.1:$port/v3/threads"
python3 - "$ready_current" "$current_headers" "$ready_threads" "$threads_headers" <<'PY'
import json
import sys

def published_pair(path):
    for raw in open(path, "rb").read().splitlines():
        name, separator, value = raw.partition(b":")
        if separator and name.lower() == b"codex-info-published-pair":
            return value.strip().decode("ascii")
    return None

ready = json.loads(open(sys.argv[1], encoding="utf-8").read())
threads = json.loads(open(sys.argv[3], encoding="utf-8").read())
rows = threads.get("threads", [])
raise SystemExit(0 if (
    ready.get("state") == "ready"
    and ready.get("authenticated") is True
    and ready.get("observed_at") is not None
    and ready.get("quota") is not None
    and len(ready.get("models", [])) == 3
    and ready.get("active_thread_count") == 1
    and len(rows) == 1
    and rows[0].get("id") == "fixture-thread"
    and rows[0].get("model") == "gpt-5.6-sol"
    and published_pair(sys.argv[2]) is not None
    and published_pair(sys.argv[2]) == published_pair(sys.argv[4])
) else 1)
PY

# Build six fixed AccountActivity templates from the same candidate binary,
# display, dimensions, locale, and font stack. The direct Rust oracle fixes
# this preview to [total, SOL, TERRA, LUNA, ASTRA, other] = [1,1,0,0,0,0].
env "${common_env[@]}" CODEX_INFO_UI_CLIENT_ONLY=1 CODEX_INFO_PREVIEW=normal \
    CODEX_INFO_PREVIEW_SIZE=900x480 "$binary" --ui --port "$port" \
    >"$temp_root/reference-ui.log" 2>&1 &
reference_ui_pid="$!"
reference_ui_starttime="$(proc_starttime "$reference_ui_pid")"
[[ "$reference_ui_starttime" =~ ^[0-9]+$ ]] || fail 'reference UI starttime could not be recorded'
reference_ready=0
reference_error=''
for reference_attempt in $(seq 1 100); do
    kill -0 "$reference_ui_pid" 2>/dev/null || {
        sed -n '1,160p' "$temp_root/reference-ui.log" >&2 || true
        fail 'reference UI exited before rendering'
    }
    reference_window_id=''
    while read -r candidate; do
        candidate_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
        if [[ "$candidate_pid" == "$reference_ui_pid" ]]; then
            reference_window_id="$candidate"
            break
        fi
    done < <(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+/ { print $1 }')
    if [[ -z "$reference_window_id" ]]; then
        reference_error='reference UI window is not available yet'
    elif reference_error="$(xwd -silent -id "$reference_window_id" -out "$reference_frame" 2>&1)"; then
        if reference_error="$(write_thread_summary_reference "$reference_frame" 2>&1)"; then
            reference_ready=1
            break
        fi
    fi
    rm -f -- "$reference_frame"
    if ((reference_attempt < 100)); then
        sleep 0.1
    fi
done
if ((reference_ready != 1)); then
    sed -n '1,160p' "$temp_root/reference-ui.log" >&2 || true
    [[ -n "$reference_error" ]] && printf '%s\n' "$reference_error" >&2
    fail 'reference thread-summary components did not render exactly'
fi
rm -- "$reference_frame"
terminate_owned "$reference_ui_pid" reference-UI "$reference_ui_starttime" "$binary" \
    || fail 'reference UI did not stop cleanly'
reference_ui_pid=''
reference_ui_starttime=''
reference_window_id=''

env -u CODEX_INFO_PREVIEW -u CODEX_INFO_PREVIEW_SIZE "${common_env[@]}" "$binary" --ui --port "$port" \
    >"$temp_root/ui.log" 2>&1 &
ui_pid="$!"
ui_starttime="$(proc_starttime "$ui_pid")"
[[ "$ui_starttime" =~ ^[0-9]+$ ]] || fail 'UI starttime could not be recorded'
for _ in $(seq 1 100); do
    kill -0 "$ui_pid" 2>/dev/null || {
        sed -n '1,160p' "$temp_root/ui.log" >&2 || true
        fail 'real-service UI exited before rendering'
    }
    while read -r candidate; do
        candidate_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
        if [[ "$candidate_pid" == "$ui_pid" ]]; then
            window_id="$candidate"
            break
        fi
    done < <(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+/ { print $1 }')
    [[ -n "$window_id" ]] && break
    sleep 0.1
done
[[ -n "$window_id" ]] || fail 'real-service UI window did not render'
assert_threads_window_closed "$ui_pid" "$window_id"

capture_state() {
    local expected="$1" baseline="${2:-}"
    xwd -silent -id "$window_id" -out "$frame" 2>/dev/null || return 1
    python3 - "$frame" "$expected" "$baseline" <<'PY'
import struct
import sys
from math import sqrt
data = open(sys.argv[1], "rb").read()
expected = sys.argv[2]
baseline_path = sys.argv[3]
header = struct.unpack(">25I", data[:100])
header_size, width, height, bytes_per_line, colors = header[0], header[4], header[5], header[12], header[19]
if (width, height) != (900, 480):
    raise SystemExit(f"unexpected real-service image size: {width}x{height}")
offset = header_size + colors * 12
stride = bytes_per_line // width
def rgb(x, y):
    index = offset + y * bytes_per_line + x * stride
    return data[index + 2], data[index + 1], data[index]
def near(value, target, tolerance=24):
    return sqrt(sum((value[i] - target[i]) ** 2 for i in range(3))) <= tolerance
# Restrict danger-color detection to the fixed StatusBanner region. ASTRA uses
# the same color in the normal model summary and must not make a ready frame fail.
red = sum(near(rgb(x, y), (239, 106, 106)) for y in range(400, 460) for x in range(20, 880))
# The quota fill is at a fixed y on the authenticated main surface. The
# auth panel's primary button is lower, so this rejects a false-ready
# frame.
blue = sum(near(rgb(x, y), (86, 178, 245), 18) for y in range(150, 170) for x in range(10, width))
model_text = sum(near(rgb(x, y), (245, 247, 251), 48) for y in range(324, 390) for x in range(10, width))
if expected == "error":
    if red < 20 or blue < 500:
        raise SystemExit(f"error frame missing retained payload/status: red={red} blue={blue}")
elif expected == "ready":
    if red >= 20 or blue < 500 or model_text < 50:
        raise SystemExit(f"ready frame still has failure or no payload: red={red} blue={blue} model={model_text}")
else:
    raise SystemExit("unknown expected frame")

if baseline_path:
    baseline = open(baseline_path, "rb").read()
    base_header = struct.unpack(">25I", baseline[:100])
    if (base_header[4], base_header[5]) != (width, height):
        raise SystemExit("baseline image size differs from current frame")
    base_offset = base_header[0] + base_header[19] * 12
    base_stride = base_header[12] // base_header[4]
    def base_rgb(x, y):
        index = base_offset + y * base_header[12] + x * base_stride
        return baseline[index + 2], baseline[index + 1], baseline[index]
    # Compare stable authenticated payload surfaces. The status banner is
    # intentionally excluded because its text/color changes on outage.
    payload_rects = ((10, 66, 890, 162), (10, 294, 890, 394))
    changed = total = 0
    for left, top, right, bottom in payload_rects:
        for y in range(top, bottom):
            for x in range(left, right):
                total += 1
                if sum(abs(rgb(x, y)[i] - base_rgb(x, y)[i]) for i in range(3)) > 24:
                    changed += 1
    if changed > total // 50:
        raise SystemExit(f"last-good payload changed too much: changed={changed} total={total}")
print(f"x11-service-recovery-visual-gate: {expected} frame PASS (red={red}, blue={blue})")
PY
}

ready_capture=0
for _ in $(seq 1 60); do
    if capture_state ready >/dev/null 2>/dev/null; then ready_capture=1; break; fi
    sleep 0.25
done
((ready_capture == 1)) || fail 'real-service UI did not render a ready details generation'
cp -- "$frame" "$ready_frame"
assert_thread_summary_components "$ready_frame" \
    || fail 'Main thread total and per-model components differ from the one-SOL wire bundle'

# Exercise the actual lazy boundary: the authenticated main window has already
# rendered with period metadata, and only this user action may materialize the
# selected history page and graph window.
click_window "$window_id" 750 30
for _ in $(seq 1 100); do
    while read -r candidate; do
        [[ "$candidate" != "$window_id" ]] || continue
        candidate_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
        if [[ "$candidate_pid" == "$ui_pid" ]]; then
            graph_window_id="$candidate"
            break
        fi
    done < <(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+/ { print $1 }')
    [[ -n "$graph_window_id" ]] && break
    sleep 0.1
done
[[ -n "$graph_window_id" ]] || fail 'Graph action did not open the real graph window'

graph_capture=0
for _ in $(seq 1 80); do
    if xwd -silent -id "$graph_window_id" -out "$graph_frame" 2>/dev/null &&
        python3 - "$graph_frame" <<'PY' >/dev/null 2>&1
import struct
import sys
from math import sqrt

data = open(sys.argv[1], "rb").read()
header = struct.unpack(">25I", data[:100])
header_size, width, height, bytes_per_line, colors = header[0], header[4], header[5], header[12], header[19]
if width < 700 or height < 480:
    raise SystemExit(f"unexpected graph image size: {width}x{height}")
offset = header_size + colors * 12
stride = bytes_per_line // width
def rgb(x, y):
    index = offset + y * bytes_per_line + x * stride
    return data[index + 2], data[index + 1], data[index]
def near(value, target, tolerance=38):
    return sqrt(sum((value[i] - target[i]) ** 2 for i in range(3))) <= tolerance

# Exclude the header/toggle legend. These pixels must come from the plotted
# history or its value labels, not from static controls.
targets = {
    "remaining": (86, 178, 245),
    "sol": (168, 140, 245),
    "terra": (93, 201, 138),
    "luna": (230, 162, 60),
}
counts = {
    name: sum(
        near(rgb(x, y), color)
        for y in range(180, height - 20)
        for x in range(20, width - 20)
    )
    for name, color in targets.items()
}
if any(count < 8 for count in counts.values()):
    raise SystemExit(f"real history plot is incomplete: {counts}")
PY
    then
        graph_capture=1
        break
    fi
    sleep 0.25
done
((graph_capture == 1)) || fail 'real graph did not render the selected history resource'

# A live daemon may publish a recoverable top-level error while retaining the
# last complete authenticated generation. This is the field state that must
# never be rendered as a fresh authentication prompt.
# Recorder cycles and the software renderer intentionally continue while the
# graph is inspected. Pin both the current Main frame and the service root
# immediately before injecting the failure, instead of comparing with the
# pre-graph frame.
service_thread_bundle_ready \
    || fail 'one-SOL wire bundle changed before failure injection'
# The graph window overlaps Main under the CI window manager. Raise Main before
# reading its drawable so XGetImage cannot return pixels from the obscuring
# graph window in the exact thread-summary comparison.
click_window "$window_id" 500 30
sleep 0.1
capture_state ready >/dev/null \
    || fail 'Main did not remain ready after the graph acceptance step'
cp -- "$frame" "$ready_frame"
assert_thread_summary_components "$ready_frame" \
    || fail 'Main thread summary changed before failure injection'
curl --fail --silent --show-error --max-time 1 "http://127.0.0.1:$port/v3/current" >"$ready_current"
touch "$temp_root/app-server-failure"
wait_service_last_good_error || fail 'resident service did not publish authenticated last-good error state'
assert_recorder_unchanged || fail 'recorder changed during the app-server failure'
error_frame=0
# A rejected current/threads pair retries unconditionally every ten seconds.
# Keep a finite three-attempt window so one publication race cannot make the
# acceptance gate flaky while still bounding the failure path.
for _ in $(seq 1 120); do
    if capture_state error "$ready_frame" >/dev/null 2>/dev/null; then error_frame=1; break; fi
    sleep 0.25
done
if ((error_frame != 1)); then
    capture_state error "$ready_frame" || true
    fail 'UI replaced authenticated last-good data with the authentication surface'
fi
rm -- "$temp_root/app-server-failure"
wait_service_models_ready || fail 'resident service did not recover after the bounded app-server failure'
assert_recorder_unchanged || fail 'recorder changed during app-server recovery'
ready_capture=0
for _ in $(seq 1 60); do
    if capture_state ready "$ready_frame" >/dev/null 2>/dev/null; then ready_capture=1; break; fi
    sleep 0.25
done
((ready_capture == 1)) || fail 'UI did not recover from authenticated last-good error state'

terminate_owned "$service_pid" REST "$service_starttime" "$rest_binary" \
    || fail 'fixture REST service did not stop cleanly'
service_pid=''
service_starttime=''
assert_recorder_unchanged || fail 'stopping REST also stopped or replaced the recorder'
error_frame=0
for _ in $(seq 1 60); do
    if capture_state error "$ready_frame" >/dev/null 2>/dev/null; then error_frame=1; break; fi
    sleep 0.25
done
((error_frame == 1)) || fail 'UI did not show retained payload with selected-endpoint failure'

launch_service
wait_service_ready || fail 'fixture-backed resident service did not recover'
assert_recorder_unchanged || fail 'restarting REST also replaced the recorder'
ready_capture=0
for _ in $(seq 1 60); do
    if capture_state ready "$ready_frame" >/dev/null 2>/dev/null; then ready_capture=1; break; fi
    sleep 0.25
done
((ready_capture == 1)) || fail 'UI did not clear the failure after same-endpoint recovery'
trap - EXIT
cleanup_resources \
    || hold "cleanup ownership could not be proven; retained $temp_root"
echo 'x11-service-recovery-visual-gate: PASS (exact one-SOL Main summary -> main period metadata -> selected history plot -> ready -> authenticated last-good error exact data -> ready -> transport error -> recovered)'

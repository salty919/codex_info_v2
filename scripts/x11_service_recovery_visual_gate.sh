#!/usr/bin/env bash
set -euo pipefail

# Verify the real (non-preview) Linux UI against the resident REST service.
# The app-server is a bounded local fixture, so no account or network is used.
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"
hold() { echo "x11-service-recovery-visual-gate: HOLD: $*" >&2; exit 2; }
fail() { echo "x11-service-recovery-visual-gate: FAIL: $*" >&2; exit 1; }
[[ -n "${DISPLAY:-}" ]] || hold 'DISPLAY is unavailable'
for command in curl python3 xprop xwd xwininfo; do
    command -v "$command" >/dev/null 2>&1 || hold "$command is unavailable"
done
binary="$root_dir/target/release/codex_info"
[[ -x "$binary" ]] || fail 'build target/release/codex_info first'

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
ui_pid=''
window_id=''
port=''
frame="$temp_root/frame.xwd"

terminate_owned() {
    local pid="$1" label="$2"
    [[ "$pid" =~ ^[0-9]+$ ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return 0
    fi
    [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$binary" ]] || {
        echo "x11-service-recovery-visual-gate: refusing to terminate unowned $label PID $pid" >&2
        return 1
    }
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$binary" ]] || return 1
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    terminate_owned "$ui_pid" UI || true
    terminate_owned "$service_pid" service || true
    case "$temp_root" in
        "$temp_parent"/.codex-info-x11-recovery.*) rm -rf -- "$temp_root" ;;
        *) echo 'x11-service-recovery-visual-gate: refusing unexpected cleanup' >&2 ;;
    esac
}
trap cleanup EXIT

mkdir -p "$temp_root"/{home,config,data,cache,state,runtime,codex}
chmod 700 "$temp_root/runtime"
fake_codex="$temp_root/fake-codex"
cat >"$fake_codex" <<'PY'
#!/usr/bin/env python3
import json
import sys
import time

reset_at = int(time.time()) + 604800
account = {
    "requiresOpenaiAuth": False,
    "account": {"type": "chatgpt", "email": "fixture@example.com", "planType": "pro"},
}
quota = {"rateLimits": {"primary": {
    "usedPercent": 56, "resetsAt": reset_at, "windowDurationMins": 10080
}}}
for line in sys.stdin:
    try:
        request = json.loads(line)
    except json.JSONDecodeError:
        continue
    request_id = request.get("id")
    if not isinstance(request_id, int):
        continue
    method = request.get("method")
    if method == "initialize":
        result = {}
    elif method == "account/read":
        result = account
    elif method == "account/rateLimits/read":
        result = quota
    elif method == "thread/list":
        result = {"data": []}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)
PY
chmod 700 "$fake_codex"

port="$(python3 - <<'PY'
import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"
[[ -n "$port" ]] || fail 'could not find an unused loopback port'
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
    "CODEX_INFO_DAEMON_INTERVAL_SECS=2"
)
run_with_common_env() {
    env -u CODEX_INFO_PREVIEW -u CODEX_INFO_PREVIEW_SIZE "${common_env[@]}" "$@"
}
launch_service() {
    run_with_common_env "$binary" --port "$port" >"$temp_root/service-$RANDOM.log" 2>&1 &
    service_pid="$!"
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

launch_service
wait_service_ready || fail 'fixture-backed resident service did not publish ready details'
run_with_common_env "$binary" --ui --port "$port" >"$temp_root/ui.log" 2>&1 &
ui_pid="$!"
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

capture_state() {
    local expected="$1"
    xwd -silent -id "$window_id" -out "$frame" 2>/dev/null || return 1
    python3 - "$frame" "$expected" <<'PY'
import struct
import sys
from math import sqrt
data = open(sys.argv[1], "rb").read()
expected = sys.argv[2]
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
red = sum(near(rgb(x, y), (239, 106, 106)) for y in range(height) for x in range(width))
# The quota fill is at a fixed y on the authenticated main surface. The
# auth panel's primary button is lower, so this rejects a false-ready
# frame while still accepting a fixture with no model rows.
blue = sum(near(rgb(x, y), (86, 178, 245), 18) for y in range(150, 170) for x in range(10, width))
if expected == "error":
    if red < 20 or blue < 500:
        raise SystemExit(f"error frame missing retained payload/status: red={red} blue={blue}")
elif expected == "ready":
    if red >= 20 or blue < 500:
        raise SystemExit(f"ready frame still has failure or no payload: red={red} blue={blue}")
else:
    raise SystemExit("unknown expected frame")
print(f"x11-service-recovery-visual-gate: {expected} frame PASS (red={red}, blue={blue})")
PY
}

ready_frame=0
for _ in $(seq 1 60); do
    if capture_state ready >/dev/null; then ready_frame=1; break; fi
    sleep 0.25
done
((ready_frame == 1)) || fail 'real-service UI did not render a ready details generation'

terminate_owned "$service_pid" service || fail 'fixture service did not stop cleanly'
service_pid=''
error_frame=0
for _ in $(seq 1 60); do
    if capture_state error >/dev/null; then error_frame=1; break; fi
    sleep 0.25
done
((error_frame == 1)) || fail 'UI did not show retained payload with selected-endpoint failure'

launch_service
wait_service_ready || fail 'fixture-backed resident service did not recover'
ready_frame=0
for _ in $(seq 1 60); do
    if capture_state ready >/dev/null; then ready_frame=1; break; fi
    sleep 0.25
done
((ready_frame == 1)) || fail 'UI did not clear the failure after same-endpoint recovery'
echo 'x11-service-recovery-visual-gate: PASS (real service ready -> stopped/error retained -> same endpoint ready/recovered)'

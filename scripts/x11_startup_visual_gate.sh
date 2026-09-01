#!/usr/bin/env bash
set -euo pipefail
root_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root_dir"
hold() { echo "x11-startup-visual-gate: HOLD: $*" >&2; exit 2; }
fail() { echo "x11-startup-visual-gate: FAIL: $*" >&2; exit 1; }
[[ -n "${DISPLAY:-}" ]] || hold 'DISPLAY is unavailable'
for command in xwininfo xprop xwd python3; do command -v "$command" >/dev/null || hold "$command is unavailable"; done
binary="$root_dir/target/release/codex_info"
[[ -x "$binary" ]] || fail 'build target/release/codex_info first'
temp_parent="${TMPDIR:-/tmp}"
temp_root="$(mktemp -d "$temp_parent/codex-info-x11-startup.XXXXXX")"
preview_pid=""
failure_pid=""
blocker_pid=""
cleanup() {
    if [[ -n "$preview_pid" ]] && kill -0 "$preview_pid" 2>/dev/null; then kill "$preview_pid" 2>/dev/null || true; wait "$preview_pid" 2>/dev/null || true; fi
    if [[ -n "$failure_pid" ]] && kill -0 "$failure_pid" 2>/dev/null; then kill "$failure_pid" 2>/dev/null || true; wait "$failure_pid" 2>/dev/null || true; fi
    if [[ -n "$blocker_pid" ]] && kill -0 "$blocker_pid" 2>/dev/null; then kill "$blocker_pid" 2>/dev/null || true; wait "$blocker_pid" 2>/dev/null || true; fi
    case "$temp_root" in
        "$temp_parent"/codex-info-x11-startup.*) rm -rf -- "$temp_root" ;;
        *) echo 'x11-startup-visual-gate: refusing unexpected cleanup' >&2 ;;
    esac
}
trap cleanup EXIT
mkdir -p "$temp_root"/{home,config,data,cache,state,runtime}
chmod 700 "$temp_root/runtime"
env HOME="$temp_root/home" XDG_CONFIG_HOME="$temp_root/config" XDG_DATA_HOME="$temp_root/data" XDG_CACHE_HOME="$temp_root/cache" XDG_STATE_HOME="$temp_root/state" XDG_RUNTIME_DIR="$temp_root/runtime" CODEX_INFO_PREVIEW=startup-loading CODEX_INFO_PREVIEW_SIZE=900x480 "$binary" --ui >"$temp_root/client.log" 2>&1 &
preview_pid="$!"
window_id=""
for startup_attempt in $(seq 1 80); do
    while read -r candidate; do
        window_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
        if [[ "$window_pid" == "$preview_pid" ]]; then window_id="$candidate"; break; fi
    done < <(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+/ {print $1}')
    [[ -n "$window_id" ]] && break
    sleep 0.125
done
[[ -n "$window_id" ]] || { sed -n '1,120p' "$temp_root/client.log" >&2 || true; fail 'startup preview window did not render'; }
root_geometry="$(xwininfo -root 2>/dev/null)"
window_geometry="$(xwininfo -id "$window_id" 2>/dev/null)"
root_width="$(awk '/Width:/ {print $2; exit}' <<<"$root_geometry")"
root_height="$(awk '/Height:/ {print $2; exit}' <<<"$root_geometry")"
window_x="$(awk '/Absolute upper-left X:/ {print $4; exit}' <<<"$window_geometry")"
window_y="$(awk '/Absolute upper-left Y:/ {print $4; exit}' <<<"$window_geometry")"
window_width="$(awk '/Width:/ {print $2; exit}' <<<"$window_geometry")"
window_height="$(awk '/Height:/ {print $2; exit}' <<<"$window_geometry")"
[[ "$root_width" =~ ^[0-9]+$ && "$root_height" =~ ^[0-9]+$ && "$window_x" =~ ^-?[0-9]+$ && "$window_y" =~ ^-?[0-9]+$ && "$window_width" =~ ^[0-9]+$ && "$window_height" =~ ^[0-9]+$ ]] || fail 'window geometry could not be read'
(( window_x >= 0 && window_y >= 0 && window_x + window_width <= root_width && window_y + window_height <= root_height )) ||
    fail "startup window is outside the visible X11 desktop: ${window_x},${window_y} ${window_width}x${window_height} on ${root_width}x${root_height}"
validate_startup_image() {
python3 - "$1" <<'PY'
import struct, sys
from math import sqrt
data = open(sys.argv[1], 'rb').read()
h = struct.unpack('>25I', data[:100])
header_size, width, height, bytes_per_line, colors = h[0], h[4], h[5], h[12], h[19]
if (width, height) != (900, 480):
    raise SystemExit(f'unexpected startup image size: {width}x{height}')
offset = header_size + colors * 12
stride = bytes_per_line // width
def rgb(x, y):
    i = offset + y * bytes_per_line + x * stride
    return data[i + 2], data[i + 1], data[i]
def near(a, b, tolerance=20):
    return sqrt(sum((a[i] - b[i]) ** 2 for i in range(3))) <= tolerance
header_pixels = [(x, y) for y in range(14, 48) for x in range(12, 420) if min(rgb(x, y)) > 150]
if len(header_pixels) < 80:
    print(f'header/version pixels missing: {len(header_pixels)}', file=sys.stderr)
    raise SystemExit(75)
canvas = rgb(10, 100)
center_pixels = [(x, y) for y in range(190, 300) for x in range(330, 570) if not near(rgb(x, y), canvas, 8)]
if len(center_pixels) < 20:
    print(f'center spinner/status missing: {len(center_pixels)}', file=sys.stderr)
    raise SystemExit(75)
# The chrome and status text legitimately contain a few quota-blue pixels.  A
# rendered payload, however, contains a connected horizontal gauge/series in
# the content area.  Detect that observable shape instead of rejecting
# harmless antialiasing noise (which made this gate flaky across X11 servers).
quota_blue = {(x, y) for y in range(80, height) for x in range(width) if near(rgb(x, y), (86, 178, 245), 16)}
seen = set()
largest = (0, 0, 0, 0)  # area, width, height, pixels
for start in quota_blue:
    if start in seen:
        continue
    stack = [start]
    seen.add(start)
    component = []
    while stack:
        x, y = stack.pop()
        component.append((x, y))
        for nx in range(x - 1, x + 2):
            for ny in range(y - 1, y + 2):
                point = (nx, ny)
                if point in quota_blue and point not in seen:
                    seen.add(point)
                    stack.append(point)
    xs = [x for x, _ in component]
    ys = [y for _, y in component]
    shape = (len(component), max(xs) - min(xs) + 1, max(ys) - min(ys) + 1, len(component))
    if shape[:3] > largest[:3]:
        largest = shape
if largest[1] >= 100 and largest[0] >= 100:
    raise SystemExit(f'partial quota payload leaked: component area={largest[0]} width={largest[1]} height={largest[2]}')
print('x11-startup-visual-gate: PASS (900x480, header/version visible, centered spinner visible, partial payload hidden)')
PY
}
startup_frame_ready=0
for _ in $(seq 1 "$((81 - startup_attempt))"); do
    if xwd -silent -id "$window_id" -out "$temp_root/startup.xwd" 2>/dev/null; then
        if validate_startup_image "$temp_root/startup.xwd" >"$temp_root/startup-check.out" 2>"$temp_root/startup-check.err"; then
            cat "$temp_root/startup-check.out"
            startup_frame_ready=1
            break
        elif (( $? != 75 )); then
            cat "$temp_root/startup-check.err" >&2
            fail 'startup preview violated its visual contract'
        fi
    fi
    sleep 0.125
done
if (( startup_frame_ready != 1 )); then
    [[ ! -s "$temp_root/startup-check.err" ]] || cat "$temp_root/startup-check.err" >&2
    fail 'startup preview did not produce a complete rendered frame'
fi

# X-START-05: the UI and its service must use the same selected endpoint.
# Occupy an ephemeral loopback port with a non-codex HTTP listener. The GUI
# must remain visible in its bounded failure/retry surface and must not fall
# back to a healthy service on the default port.
kill "$preview_pid" 2>/dev/null || true
wait "$preview_pid" 2>/dev/null || true
preview_pid=""
python3 -u -m http.server 0 --bind 127.0.0.1 >"$temp_root/blocker.log" 2>&1 &
blocker_pid="$!"
blocked_port=""
for _ in $(seq 1 40); do
    blocked_port="$(sed -n 's/.* port \([0-9][0-9]*\) .*/\1/p' "$temp_root/blocker.log" | head -n 1)"
    [[ "$blocked_port" =~ ^[0-9]+$ ]] && break
    sleep 0.05
done
[[ "$blocked_port" =~ ^[0-9]+$ ]] || fail 'could not allocate the failure-port fixture'
env HOME="$temp_root/home" XDG_CONFIG_HOME="$temp_root/config" XDG_DATA_HOME="$temp_root/data" XDG_CACHE_HOME="$temp_root/cache" XDG_STATE_HOME="$temp_root/state" XDG_RUNTIME_DIR="$temp_root/runtime" "$binary" --ui --port "$blocked_port" >"$temp_root/failure-client.log" 2>&1 &
failure_pid="$!"
failure_window_id=""
for failure_attempt in $(seq 1 120); do
    while read -r candidate; do
        window_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
        if [[ "$window_pid" == "$failure_pid" ]]; then failure_window_id="$candidate"; break; fi
    done < <(xwininfo -root -tree 2>/dev/null | awk '/^ +0x[0-9a-f]+/ {print $1}')
    [[ -n "$failure_window_id" ]] && break
    sleep 0.125
done
[[ -n "$failure_window_id" ]] || { sed -n '1,120p' "$temp_root/failure-client.log" >&2 || true; fail 'failure-port GUI did not render'; }
validate_failure_image() {
python3 - "$1" <<'PY'
import struct, sys
from math import sqrt
data = open(sys.argv[1], 'rb').read()
h = struct.unpack('>25I', data[:100])
header_size, width, height, bytes_per_line, colors = h[0], h[4], h[5], h[12], h[19]
if (width, height) != (900, 480):
    raise SystemExit(f'unexpected failure image size: {width}x{height}')
offset = header_size + colors * 12
stride = bytes_per_line // width
def rgb(x, y):
    i = offset + y * bytes_per_line + x * stride
    return data[i + 2], data[i + 1], data[i]
def near(a, b, tolerance=24):
    return sqrt(sum((a[i] - b[i]) ** 2 for i in range(3))) <= tolerance
failure_pixels = sum(
    near(rgb(x, y), (239, 106, 106))
    for y in range(180, 280)
    for x in range(35, 720)
)
cta_pixels = sum(
    near(rgb(x, y), (86, 178, 245))
    for y in range(150, 250)
    for x in range(35, 430)
)
if failure_pixels < 20:
    print(f'selected-endpoint failure text is missing: pixels={failure_pixels}', file=sys.stderr)
    raise SystemExit(75)
if cta_pixels < 500:
    print(f'failure recovery action is missing: pixels={cta_pixels}', file=sys.stderr)
    raise SystemExit(75)
print(f'x11-startup-failure-port-gate: PASS (window retained, selected-endpoint failure pixels={failure_pixels}, recovery pixels={cta_pixels})')
PY
}
failure_frame_ready=0
for _ in $(seq 1 "$((121 - failure_attempt))"); do
    kill -0 "$failure_pid" 2>/dev/null || fail 'failure-port GUI exited before rendering'
    if xwd -silent -id "$failure_window_id" -out "$temp_root/failure.xwd" 2>/dev/null; then
        if validate_failure_image "$temp_root/failure.xwd" >"$temp_root/failure-check.out" 2>"$temp_root/failure-check.err"; then
            cat "$temp_root/failure-check.out"
            failure_frame_ready=1
            break
        elif (( $? != 75 )); then
            cat "$temp_root/failure-check.err" >&2
            fail 'failure-port GUI violated its visual contract'
        fi
    fi
    sleep 0.125
done
if (( failure_frame_ready != 1 )); then
    [[ ! -s "$temp_root/failure-check.err" ]] || cat "$temp_root/failure-check.err" >&2
    fail 'failure-port GUI did not produce a complete rendered frame'
fi
kill -0 "$failure_pid" 2>/dev/null || fail 'failure-port GUI exited after rendering'

# X-START-06: exercise the normal Linux UI against a real resident service,
# including the selected endpoint becoming unavailable and recovering. The
# helper owns its isolated service/data fixture and fails closed on every
# missing visual state.
bash "$root_dir/scripts/x11_service_recovery_visual_gate.sh"

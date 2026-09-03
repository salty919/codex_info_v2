#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

# Finite daemon/REST/UI mode acceptance checks. Every case owns a temporary
# HOME, XDG directories, CODEX_HOME, history database, and loopback ports.
# No PID outside the case's binary/data scope is ever terminated.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
    echo "record-daemon-e2e: $*" >&2
    exit 1
}

for command in awk curl date getconf python3 rg sed sqlite3 ss stat tr; do
    command -v "$command" >/dev/null || fail "$command is required"
done
BINARY="$ROOT_DIR/target/release/codex_info"
[[ -x "$BINARY" ]] || fail "build target/release/codex_info first"

for contract in \
    'ExecStart=%h/.local/bin/codex_info --port 8787' \
    'Restart=always' \
    'RestartSec=5s' \
    'StartLimitBurst=2' \
    'NoNewPrivileges=true'; do
    rg -q --fixed-strings -- "$contract" packaging/codex-info.service \
        || fail "service contract missing: $contract"
done
if rg -q '^PrivateTmp=true$' packaging/codex-info.service; then
    fail 'PrivateTmp=true hides live Codex /proc state from the Threads snapshot'
fi

temp_parent="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
temp_parent="$(cd -- "$temp_parent" 2>/dev/null && pwd -P)" \
    || fail 'temporary parent is unavailable'
case "$temp_parent/" in
    "$ROOT_DIR/"*) fail 'temporary acceptance data must stay outside the repository' ;;
esac
tmp_root="$(mktemp -d "$temp_parent/codex-info-daemon-e2e.XXXXXX")"
case_root=""
case_data=""
case_home=""
case_port=""
case_alt_port=""
case_lock=""
case_db=""
case_label=""
common_env=()
service_pid=""
ui_pid=""
hold_count=0
port_seed=$((30000 + ($$ % 10000)))

listener_count() {
    local port="$1"
    ss -ltnH "sport = :$port" 2>/dev/null \
        | awk '$1 == "LISTEN" { count += 1 } END { print count + 0 }'
}

reserve_ports() {
    local candidate
    while ((port_seed < 60000)); do
        candidate="$port_seed"
        port_seed=$((port_seed + 3))
        if [[ "$(listener_count "$candidate")" == 0 \
            && "$(listener_count "$((candidate + 1))")" == 0 ]]; then
            case_port="$candidate"
            case_alt_port="$((candidate + 1))"
            return
        fi
    done
    fail 'could not find two unused loopback ports'
}

process_env_contains() {
    local pid="$1" needle="$2" env_text
    [[ -r "/proc/$pid/environ" ]] || return 1
    env_text="$(tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null || true)"
    rg -Fqx -- "$needle" <<<"$env_text" >/dev/null
}

process_cmdline() {
    local pid="$1"
    tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true
}

process_matches_scope() {
    local pid="$1" kind="$2" cmdline
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [[ -e "/proc/$pid" ]] || return 1
    [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$BINARY" ]] \
        || return 1
    process_env_contains "$pid" "CODEX_INFO_DATA_DIR=$case_data" || return 1
    cmdline="$(process_cmdline "$pid")"
    case "$kind" in
        service)
            [[ "$cmdline" == *"--port $case_port"* \
                && "$cmdline" != *"--ui"* ]]
            ;;
        ui)
            [[ "$cmdline" == *"--ui"* ]]
            ;;
        *)
            return 1
            ;;
    esac
}

find_ui_pids() {
    local proc pid
    for proc in /proc/[0-9]*; do
        [[ -d "$proc" ]] || continue
        pid="${proc##*/}"
        if process_matches_scope "$pid" ui; then
            printf '%s\n' "$pid"
        fi
    done
}

find_service_pids() {
    local proc pid
    for proc in /proc/[0-9]*; do
        [[ -d "$proc" ]] || continue
        pid="${proc##*/}"
        if process_matches_scope "$pid" service; then
            printf '%s\n' "$pid"
        fi
    done
}

terminate_scoped_pid() {
    local pid="$1" kind="$2" label="$3" waited
    [[ -n "$pid" ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return 0
    fi
    if ! process_matches_scope "$pid" "$kind"; then
        echo "record-daemon-e2e: refusing to terminate unverified $label PID $pid" >&2
        return 1
    fi
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 40); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        # The identity/data/executable checks above constrain this to a child
        # created by this case. Never escalate a PID that no longer matches.
        if process_matches_scope "$pid" "$kind"; then
            kill -KILL "$pid" 2>/dev/null || true
        fi
    fi
    for _ in $(seq 1 20); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    waited=0
    wait "$pid" 2>/dev/null || waited=$?
    if kill -0 "$pid" 2>/dev/null; then
        echo "record-daemon-e2e: $label PID $pid did not terminate (status=$waited)" >&2
        return 1
    fi
}

stop_ui() {
    local pid pids=()
    mapfile -t pids < <(find_ui_pids)
    if [[ "${#pids[@]}" -eq 0 && -n "${ui_pid:-}" ]]; then
        pids=("$ui_pid")
    fi
    for pid in "${pids[@]}"; do
        terminate_scoped_pid "$pid" ui 'UI' || true
    done
    ui_pid=""
}

stop_services() {
    local pids=() pid
    mapfile -t pids < <(find_service_pids)
    for pid in "${pids[@]}"; do
        terminate_scoped_pid "$pid" service 'service' || true
    done
    service_pid=""
}

cleanup() {
    stop_ui || true
    stop_services || true
    case "$tmp_root" in
        "$temp_parent"/codex-info-daemon-e2e.*)
            rm -rf -- "$tmp_root"
            ;;
        *)
            echo "record-daemon-e2e: refusing to clean unexpected path $tmp_root" >&2
            ;;
    esac
}
trap cleanup EXIT

write_fixture() {
    local auth_file now initial_time reset_at session_dir session
    session_dir="$case_home/sessions/$(date -u +%Y/%m/%d)"
    mkdir -p "$session_dir" "$case_data/history"
    chmod 700 "$case_home"
    auth_file="$case_home/auth.json"
    printf '%s\n' '{"auth_mode":"chatgpt","tokens":{"account_id":"fixture-account-129"}}' \
        >"$auth_file"
    chmod 600 "$auth_file"
    now="$(date +%s)"
    reset_at=$((now + 604200))
    initial_time=$((now - 600))
    common_env+=("CODEX_INFO_FAKE_RESET_AT=$reset_at")
    session="$session_dir/daemon-e2e.jsonl"
    cat >"$session" <<EOF
{"timestamp":"$(date -u -d "@$initial_time" +%Y-%m-%dT%H:%M:%SZ)","type":"turn_context","model":"gpt-5.6-luna"}
{"timestamp":"$(date -u -d "@$initial_time" +%Y-%m-%dT%H:%M:%SZ)","type":"token_count","payload":{"info":{"total_token_usage":{"total_tokens":120,"input_tokens":100,"cached_input_tokens":80,"output_tokens":20}}}}
EOF
    chmod 600 "$session"
    printf '{"reset_at":%s,"window_seconds":604800}\n' "$reset_at" \
        >"$case_data/history/usage_reset_hint.json"
}

setup_case() {
    case_label="$1"
    case_root="$tmp_root/$case_label"
    case_home="$case_root/codex"
    case_data="$case_root/data"
    case_lock="$case_data/history/usage_record_daemon.lock"
    case_db=""
    service_pid=""
    ui_pid=""
    mkdir -p "$case_root/home" "$case_root/xdg-config" "$case_root/xdg-data" \
        "$case_root/xdg-cache" "$case_root/xdg-state" "$case_root/xdg-runtime"
    chmod 700 "$case_root/xdg-runtime"
    reserve_ports
    common_env=(
        "HOME=$case_root/home"
        "XDG_CONFIG_HOME=$case_root/xdg-config"
        "XDG_DATA_HOME=$case_root/xdg-data"
        "XDG_CACHE_HOME=$case_root/xdg-cache"
        "XDG_STATE_HOME=$case_root/xdg-state"
        "XDG_RUNTIME_DIR=$case_root/xdg-runtime"
        "CODEX_HOME=$case_home"
        "CODEX_INFO_DATA_DIR=$case_data"
        "CODEX_INFO_CODEX_BIN=$ROOT_DIR/scripts/fake_codex_app_server.py"
        "CODEX_INFO_DAEMON_INTERVAL_SECS=5"
        "CODEX_INFO_DEBUG=1"
    )
}

launch_service() {
    local log_name="$1"
    env "${common_env[@]}" "$BINARY" --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    service_pid="$!"
}

launch_managed_service() {
    local log_name="$1"
    env "${common_env[@]}" CODEX_INFO_SYSTEMD_MANAGED=1 "$BINARY" --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    service_pid="$!"
}

launch_ui() {
    local log_name="$1"
    env "${common_env[@]}" \
        CODEX_INFO_PREVIEW=normal "$BINARY" --ui --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    ui_pid="$!"
}

launch_client_only_ui() {
    local log_name="$1"
    env "${common_env[@]}" \
        CODEX_INFO_UI_CLIENT_ONLY=1 CODEX_INFO_PREVIEW=normal \
        "$BINARY" --ui --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    ui_pid="$!"
}

lock_owner() {
    [[ -f "$case_lock" ]] || return 1
    sed -nE 's/.*"pid"[[:space:]]*:[[:space:]]*([0-9]+).*/\1/p' "$case_lock"
}

service_health() {
    curl --fail --silent --show-error --max-time 1 \
        "http://127.0.0.1:$case_port/v1/health" >/dev/null 2>&1
}

wait_for_ready() {
    for _ in $(seq 1 60); do
        if [[ -f "$case_lock" ]] && service_health; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_for_history_value() {
    local query="$1" minimum="$2" observed="" read_ok=0
    for _ in $(seq 1 30); do
        read_ok=0
        if [[ -f "$case_db" ]]; then
            if observed="$(sqlite3 "$case_db" "$query" 2>/dev/null)"; then
                read_ok=1
            else
                observed=""
            fi
        else
            observed=""
        fi
        if ((read_ok == 1)) && [[ "$observed" =~ ^[0-9]+$ ]] \
            && ((10#$observed >= minimum)); then
            printf '%s\n' "$observed"
            return 0
        fi
        sleep 0.5
    done
    if [[ "$observed" =~ ^[0-9]+$ ]]; then
        printf '%s\n' "$observed"
    else
        printf '0\n'
    fi
    return 1
}

wait_for_account_baseline() {
    local checkpoint_count databases=()
    for _ in $(seq 1 80); do
        databases=("$case_data"/history/accounts/v1/*/epoch-*/usage_history.sqlite3)
        if [[ "${#databases[@]}" -eq 1 ]]; then
            checkpoint_count="$(sqlite3 -batch -bail -cmd '.timeout 2000' \
                "${databases[0]}" 'SELECT COUNT(*) FROM session_checkpoints;' 2>/dev/null || true)"
            if [[ "$checkpoint_count" =~ ^[0-9]+$ ]] && ((10#$checkpoint_count >= 1)); then
                case_db="${databases[0]}"
                return 0
            fi
        fi
        sleep 0.25
    done
    return 1
}

require_ready() {
    if ! wait_for_ready; then
        sed -n '1,160p' "$case_root"/*.log >&2 2>/dev/null || true
        fail "$case_label: service did not become ready"
    fi
}

require_one_service() {
    local expected_owner="${1:-}" owner="" pids=() pid
    for _ in $(seq 1 20); do
        mapfile -t pids < <(find_service_pids)
        owner="$(lock_owner 2>/dev/null || true)"
        if [[ "${#pids[@]}" -eq 1 && -n "$owner" \
            && "${pids[0]}" == "$owner" \
            && "$(listener_count "$case_port")" == 1 ]]; then
            break
        fi
        sleep 0.1
    done
    [[ "${#pids[@]}" -eq 1 ]] \
        || fail "$case_label: expected one scoped service process, found ${#pids[@]}"
    [[ -n "$owner" && "$owner" == "${pids[0]}" ]] \
        || fail "$case_label: lock owner does not match the sole service process"
    [[ "$(listener_count "$case_port")" == 1 ]] \
        || fail "$case_label: expected one listener on $case_port"
    if [[ -n "$expected_owner" ]]; then
        [[ "$owner" == "$expected_owner" ]] \
            || fail "$case_label: service owner changed ($expected_owner -> $owner)"
    fi
    service_pid="$owner"
}

assert_recorder_state() {
    local expected_state="$1" expected_pid="$2"
    python3 - "$case_data/history/recorder-state.json" "$expected_state" "$expected_pid" <<'PY'
import json
import os
import stat
import sys

path, expected_state, expected_pid = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)
expected_keys = {
    "schema", "pid", "process_starttime", "owner_nonce", "write_state",
    "partition_id_hash", "data_generation", "collector_epoch", "cycle_seq",
    "last_commit_unix", "updated_at_unix",
}
if set(value) != expected_keys:
    raise SystemExit("recorder-state key set changed")
if value["schema"] != "codex-info-recorder-state-v1":
    raise SystemExit("recorder-state schema changed")
if value["pid"] != int(expected_pid) or value["write_state"] != expected_state:
    raise SystemExit("recorder-state owner/state mismatch")
if not isinstance(value["process_starttime"], int) or value["process_starttime"] <= 0:
    raise SystemExit("recorder-state process identity is invalid")
if not isinstance(value["owner_nonce"], str) or len(value["owner_nonce"]) != 32 \
        or any(char not in "0123456789abcdef" for char in value["owner_nonce"]):
    raise SystemExit("recorder-state owner nonce is invalid")
if not isinstance(value["updated_at_unix"], int) or value["updated_at_unix"] <= 0:
    raise SystemExit("recorder-state updated_at_unix is invalid")
if expected_state == "ready":
    if not isinstance(value["partition_id_hash"], str) or len(value["partition_id_hash"]) != 64:
        raise SystemExit("ready recorder-state partition hash is invalid")
    if not isinstance(value["data_generation"], int) or value["data_generation"] <= 0:
        raise SystemExit("ready recorder-state generation is invalid")
    if not isinstance(value["collector_epoch"], str) or len(value["collector_epoch"]) != 32:
        raise SystemExit("ready recorder-state collector epoch is invalid")
    if not isinstance(value["cycle_seq"], int) or value["cycle_seq"] <= 0:
        raise SystemExit("ready recorder-state cycle is invalid")
    if not isinstance(value["last_commit_unix"], int) or value["last_commit_unix"] <= 0:
        raise SystemExit("ready recorder-state commit time is invalid")
elif expected_state == "idle_no_account":
    if any(value[key] is not None for key in (
        "partition_id_hash", "data_generation", "collector_epoch",
        "cycle_seq", "last_commit_unix",
    )):
        raise SystemExit("idle recorder-state carries commit fields")
else:
    raise SystemExit("unexpected recorder-state test state")
if (stat.S_IMODE(os.stat(path).st_mode) != 0o600):
    raise SystemExit("recorder-state is not owner-private")
PY
}

require_no_service() {
    local pids=()
    [[ ! -e "$case_lock" ]] \
        || fail "$case_label: recorder lock remains unexpectedly"
    mapfile -t pids < <(find_service_pids)
    [[ "${#pids[@]}" -eq 0 ]] \
        || fail "$case_label: scoped service process remains (${pids[*]})"
    [[ "$(listener_count "$case_port")" == 0 \
        && "$(listener_count "$case_alt_port")" == 0 ]] \
        || fail "$case_label: REST listener remains unexpectedly"
}

stop_current_service() {
    local owner=""
    owner="$(lock_owner 2>/dev/null || true)"
    if [[ -n "$owner" ]]; then
        terminate_scoped_pid "$owner" service 'service' || true
    fi
    stop_services
}

wait_for_ui_window() {
    local pid="$1" tree
    command -v xwininfo >/dev/null 2>&1 || return 1
    for _ in $(seq 1 60); do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 1
        fi
        tree="$(xwininfo -root -tree 2>/dev/null || true)"
        if rg -q --fixed-strings -- 'preview@example.com' <<<"$tree"; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_for_two_ui_windows() {
    local first_pid="$1" second_pid="$2" tree window_count
    command -v xwininfo >/dev/null 2>&1 || return 1
    for _ in $(seq 1 60); do
        if ! kill -0 "$first_pid" 2>/dev/null \
            || ! kill -0 "$second_pid" 2>/dev/null \
            || ! process_matches_scope "$first_pid" ui \
            || ! process_matches_scope "$second_pid" ui; then
            return 1
        fi
        tree="$(xwininfo -root -tree 2>/dev/null || true)"
        window_count="$(awk '{ count += gsub(/preview@example[.]com/, "&") } END { print count + 0 }' <<<"$tree")"
        if ((window_count >= 2)); then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

is_descendant_of() {
    local pid="$1" ancestor="$2" ppid
    [[ "$pid" != "$ancestor" ]] || return 1
    for _ in $(seq 1 32); do
        ppid="$(awk '/^PPid:/ { print $2; exit }' "/proc/$pid/status" 2>/dev/null || true)"
        [[ "$ppid" =~ ^[0-9]+$ && "$ppid" != "$pid" ]] || return 1
        [[ "$ppid" == "$ancestor" ]] && return 0
        [[ "$ppid" != 1 ]] || return 1
        pid="$ppid"
    done
    return 1
}

find_service_zombies() {
    local first_pid="$1" second_pid="$2" proc pid state name
    for proc in /proc/[0-9]*; do
        [[ -r "$proc/status" ]] || continue
        pid="${proc##*/}"
        state="$(awk '/^State:/ { print $2; exit }' "$proc/status" 2>/dev/null || true)"
        [[ "$state" == Z ]] || continue
        name="$(awk '/^Name:/ { print $2; exit }' "$proc/status" 2>/dev/null || true)"
        [[ "$name" == "${BINARY##*/}" ]] || continue
        if is_descendant_of "$pid" "$first_pid" \
            || is_descendant_of "$pid" "$second_pid"; then
            printf '%s\n' "$pid"
        fi
    done
}

ui_display_available=0
if [[ -n "${DISPLAY:-}" ]] && command -v xdpyinfo >/dev/null 2>&1 \
    && xdpyinfo >/dev/null 2>&1; then
    ui_display_available=1
fi

mark_ui_hold() {
    local label="$1" reason="$2"
    hold_count=$((hold_count + 1))
    printf 'CASE %s: HOLD (%s)\n' "$label" "$reason"
}

run_service_cold_start() {
    local append_time before after clk_tck cpu_before cpu_after idle_cpu_ticks session now2
    setup_case service-cold-start
    write_fixture
    launch_service service-cold
    require_ready
    require_one_service "$service_pid"
    if ! wait_for_account_baseline; then
        sed -n '1,160p' "$case_root/service-cold.log" >&2 || true
        fail "$case_label: daemon did not persist the account Session baseline"
    fi
    assert_recorder_state ready "$service_pid" \
        || fail "$case_label: recorder-state.json is not an acknowledged ready state"
    before="$(sqlite3 "$case_db" 'SELECT COUNT(*) FROM usage_history;')"
    [[ "$(sqlite3 "$case_db" 'SELECT COUNT(*) FROM usage_history WHERE sol_tokens <> 0 OR terra_tokens <> 0 OR luna_tokens <> 0 OR ABS(sol_dollars) > 0.0000001 OR ABS(terra_dollars) > 0.0000001 OR ABS(luna_dollars) > 0.0000001;')" == 0 ]] \
        || fail "$case_label: pre-boundary Session bytes were attributed"
    [[ "$(sqlite3 "$case_db" 'SELECT COUNT(*) FROM session_ranges;')" == 0 ]] \
        || fail "$case_label: pre-boundary Session bytes produced a committed range"
    [[ "$(sqlite3 "$case_db" 'SELECT COUNT(*) FROM storage_partition;')" == 1 ]] \
        || fail "$case_label: account database partition authority is missing"
    [[ ! -e "$case_data/history/usage_history.sqlite3" ]] \
        || fail "$case_label: legacy unpartitioned history database was created"

    clk_tck="$(getconf CLK_TCK)"
    cpu_before="$(awk '{print $14+$15}' "/proc/$service_pid/stat")"
    sleep 5
    cpu_after="$(awk '{print $14+$15}' "/proc/$service_pid/stat")"
    idle_cpu_ticks=$((cpu_after - cpu_before))
    [[ "$idle_cpu_ticks" -lt $((clk_tck * 5 / 2)) ]] \
        || fail "$case_label: unchanged-input daemon CPU exceeded 50%"

    now2="$(date +%s)"
    session="$case_home/sessions/$(date -u +%Y/%m/%d)/daemon-e2e.jsonl"
    append_time="$(date -u -d "@$now2" +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\n' \
        "{\"timestamp\":\"$append_time\",\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}" \
        "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":240,\"input_tokens\":200,\"cached_input_tokens\":160,\"output_tokens\":40}}}}" \
        "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":360,\"input_tokens\":300,\"cached_input_tokens\":240,\"output_tokens\":60}}}}" \
        >>"$session"
    after=0
    if ! after="$(wait_for_history_value \
        'SELECT COALESCE(MAX(luna_tokens),0) FROM usage_history;' 120)"; then
        sed -n '1,160p' "$case_root/service-cold.log" >&2 || true
        fail "$case_label: daemon did not record changed session input (observed luna_tokens=$after)"
    fi
    if [[ "$(listener_count "$case_port")" != 1 ]] || ! service_health; then
        fail "$case_label: REST became unavailable during recording"
    fi

    stop_current_service
    for _ in $(seq 1 20); do
        [[ ! -e "$case_lock" ]] && break
        sleep 0.25
    done
    [[ ! -e "$case_lock" ]] || fail "$case_label: daemon lock was not released"
    [[ "$(listener_count "$case_port")" == 0 ]] \
        || fail "$case_label: service REST listener remained after shutdown"
    [[ "$(sqlite3 "$case_db" "SELECT COUNT(*) FROM recorder_gap_ledger WHERE state = 'pending';")" -ge 1 ]] \
        || fail "$case_label: clean stop did not leave an explicit pending recorder gap"
    [[ "$(sqlite3 "$case_db" 'PRAGMA quick_check;')" == ok ]] \
        || fail "$case_label: history database quick_check failed"
    printf 'CASE %s: PASS (rows_before=%s, luna_tokens_after=%s, idle_cpu_ticks=%s/%s, one owner/listener and clean stop)\n' \
        "$case_label" "$before" "$after" "$idle_cpu_ticks" "$clk_tck"
}

run_ui_without_service() {
    local owner launcher_pid
    setup_case ui-without-service
    write_fixture
    if [[ "$ui_display_available" != 1 ]]; then
        mark_ui_hold "$case_label" 'X11 display is unavailable; UI was not rendered'
        return
    fi
    launch_ui ui-new-service
    launcher_pid="$ui_pid"
    require_ready
    owner="$(lock_owner)"
    [[ "$owner" != "$launcher_pid" ]] \
        || fail "$case_label: --ui became the resident service instead of adding UI"
    require_one_service
    if ! wait_for_ui_window "$launcher_pid"; then
        if ! xdpyinfo >/dev/null 2>&1; then
            stop_ui
            stop_current_service
            mark_ui_hold "$case_label" 'X11 display became unavailable; UI was not rendered'
            return
        fi
        sed -n '1,120p' "$case_root/ui-new-service.log" >&2 || true
        fail "$case_label: --ui process did not render a preview window"
    fi
    require_one_service "$owner"
    service_health || fail "$case_label: --ui-created service became unavailable"
    stop_ui
    require_one_service "$owner"
    stop_current_service
    require_no_service
    printf 'CASE %s: PASS (rendered --ui, one newly-created service owner/listener)\n' "$case_label"
}

run_ui_with_service() {
    local owner launcher_pid
    setup_case ui-with-service
    write_fixture
    launch_service ui-existing-service
    require_ready
    require_one_service "$service_pid"
    owner="$service_pid"
    if [[ "$ui_display_available" != 1 ]]; then
        stop_current_service
        mark_ui_hold "$case_label" 'X11 display is unavailable; UI was not rendered'
        return
    fi
    launch_ui ui-existing-service-window
    launcher_pid="$ui_pid"
    if ! wait_for_ui_window "$launcher_pid"; then
        if ! xdpyinfo >/dev/null 2>&1; then
            stop_ui
            stop_current_service
            mark_ui_hold "$case_label" 'X11 display became unavailable; UI was not rendered'
            return
        fi
        sed -n '1,120p' "$case_root/ui-existing-service-window.log" >&2 || true
        fail "$case_label: --ui process did not render a preview window"
    fi
    require_one_service "$owner"
    [[ "$launcher_pid" != "$owner" ]] \
        || fail "$case_label: --ui replaced the existing service with its UI process"
    stop_ui
    require_one_service "$owner"
    stop_current_service
    require_no_service
    printf 'CASE %s: PASS (rendered --ui reusing service PID %s, no additional resident)\n' \
        "$case_label" "$owner"
}

run_verified_ui_failure_without_service() {
    local launcher_pid pids=()
    setup_case verified-ui-failure-no-owner
    write_fixture
    if [[ "$ui_display_available" != 1 ]]; then
        mark_ui_hold "$case_label" 'X11 display is unavailable; UI was not rendered'
        return
    fi
    # This is the installed-launcher fallback contract: the verified payload
    # receives only a strict client marker after service startup failed. It
    # must retain the localized connection/retry surface without creating a
    # raw resident owner, recorder, writer, or listener.
    launch_client_only_ui ui-client-only
    launcher_pid="$ui_pid"
    if ! wait_for_ui_window "$launcher_pid"; then
        if ! xdpyinfo >/dev/null 2>&1; then
            stop_ui
            mark_ui_hold "$case_label" 'X11 display became unavailable; UI was not rendered'
            return
        fi
        sed -n '1,120p' "$case_root/ui-client-only.log" >&2 || true
        fail "$case_label: verified client-only UI did not render a failure surface"
    fi
    process_env_contains "$launcher_pid" 'CODEX_INFO_UI_CLIENT_ONLY=1' \
        || fail "$case_label: client-only marker was not preserved"
    sleep 2
    [[ ! -e "$case_lock" ]] \
        || fail "$case_label: client-only UI created a recorder owner after service failure"
    mapfile -t pids < <(find_service_pids)
    [[ "${#pids[@]}" -eq 0 ]] \
        || fail "$case_label: client-only UI created resident service process(es) ${pids[*]}"
    [[ "$(listener_count "$case_port")" == 0 ]] \
        || fail "$case_label: client-only UI created an unmanaged listener"
    terminate_scoped_pid "$launcher_pid" ui 'client-only UI' || true
    ui_pid=""
    require_no_service
    printf 'CASE %s: PASS (verified UI failure surface remained client-only with zero owner/listener)\n' \
        "$case_label"
}

run_simultaneous_ui_without_service() {
    local first_ui second_ui owner pids=() zombies=()
    setup_case simultaneous-ui-without-service
    write_fixture
    if [[ "$ui_display_available" != 1 ]]; then
        mark_ui_hold "$case_label" 'X11 display is unavailable; UI was not rendered'
        return
    fi
    launch_ui ui-concurrent-first
    first_ui="$ui_pid"
    launch_ui ui-concurrent-second
    second_ui="$ui_pid"
    require_ready
    require_one_service
    owner="$service_pid"
    [[ "$owner" != "$first_ui" && "$owner" != "$second_ui" ]] \
        || fail "$case_label: --ui process became the service owner"
    process_matches_scope "$first_ui" ui \
        || fail "$case_label: first --ui launcher is not a scoped UI"
    process_matches_scope "$second_ui" ui \
        || fail "$case_label: second --ui launcher is not a scoped UI"
    if ! wait_for_two_ui_windows "$first_ui" "$second_ui"; then
        if ! xdpyinfo >/dev/null 2>&1; then
            stop_ui
            stop_current_service
            mark_ui_hold "$case_label" 'X11 display became unavailable; UI was not rendered'
            return
        fi
        sed -n '1,120p' "$case_root"/ui-concurrent-*.log >&2 2>/dev/null || true
        fail "$case_label: concurrent --ui launchers did not render two preview windows"
    fi
    require_one_service "$owner"
    mapfile -t pids < <(find_service_pids)
    [[ "${#pids[@]}" -eq 1 && "${pids[0]}" == "$owner" ]] \
        || fail "$case_label: concurrent --ui launchers left extra service processes"
    mapfile -t zombies < <(find_service_zombies "$first_ui" "$second_ui")
    [[ "${#zombies[@]}" -eq 0 ]] \
        || fail "$case_label: losing --ui service zombie remains (${zombies[*]})"
    service_health || fail "$case_label: concurrent --ui service became unavailable"

    terminate_scoped_pid "$first_ui" ui 'first concurrent --ui process'
    terminate_scoped_pid "$second_ui" ui 'second concurrent --ui process'
    ui_pid=""
    [[ ! -e "/proc/$first_ui" && ! -e "/proc/$second_ui" ]] \
        || fail "$case_label: one of the concurrent --ui processes remained resident"
    require_one_service "$owner"
    service_health || fail "$case_label: sole service did not survive both UI exits"
    stop_current_service
    require_no_service
    printf 'CASE %s: PASS (simultaneous --ui processes=%s,%s, sole service owner/listener=%s, clean stop)\n' \
        "$case_label" "$first_ui" "$second_ui" "$owner"
}

run_simultaneous_service_launches() {
    local first second owner loser pids=()
    setup_case simultaneous-service-launches
    write_fixture
    env "${common_env[@]}" "$BINARY" --port "$case_port" \
        >"$case_root/simultaneous-first.log" 2>&1 &
    first="$!"
    env "${common_env[@]}" "$BINARY" --port "$case_port" \
        >"$case_root/simultaneous-second.log" 2>&1 &
    second="$!"
    require_ready
    owner="$(lock_owner)"
    [[ "$owner" == "$first" || "$owner" == "$second" ]] \
        || fail "$case_label: lock owner is not one of the simultaneous launchers"
    require_one_service "$owner"
    mapfile -t pids < <(find_service_pids)
    [[ "${#pids[@]}" -eq 1 ]] \
        || fail "$case_label: concurrent launch created multiple service owners"
    if [[ "$owner" == "$first" ]]; then
        loser="$second"
    else
        loser="$first"
    fi
    for _ in $(seq 1 20); do
        if ! kill -0 "$loser" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if [[ -e "/proc/$loser" ]] && ! process_matches_scope "$loser" service; then
        # A failed launcher can be a zombie until its shell parent reaps it.
        wait "$loser" 2>/dev/null || true
    fi
    [[ ! -e "/proc/$loser" ]] \
        || fail "$case_label: losing service launcher remained resident"
    if [[ "$(listener_count "$case_port")" != 1 ]] || ! service_health; then
        fail "$case_label: concurrent launch did not leave one healthy listener"
    fi
    terminate_scoped_pid "$owner" service 'concurrent service' || true
    stop_services
    require_no_service
    printf 'CASE %s: PASS (simultaneous launchers=%s,%s, sole owner/listener=%s)\n' \
        "$case_label" "$first" "$second" "$owner"
}

launch_failure_service() {
    local mode="$1" log_name="$2"
    env "${common_env[@]}" CODEX_INFO_RECORDER_FAILURE="$mode" "$BINARY" --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    service_pid="$!"
}

wait_for_expected_failure() {
    local pid="$1" label="$2" status
    for _ in $(seq 1 120); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.25
    done
    if kill -0 "$pid" 2>/dev/null; then
        terminate_scoped_pid "$pid" service "$label" || true
        fail "$case_label: $label did not exit within the finite failure budget"
    fi
    if wait "$pid"; then
        fail "$case_label: $label exited cleanly after an injected failure"
    else
        status="$?"
    fi
    [[ "$status" != 0 ]] || fail "$case_label: $label returned status zero"
    service_pid=""
}

run_managed_activation_retires_unmanaged_owner() {
    local unmanaged managed
    setup_case managed-activation-retires-unmanaged
    write_fixture
    launch_service managed-unmanaged-owner
    require_ready
    unmanaged="$service_pid"
    require_one_service "$unmanaged"
    launch_managed_service managed-retire
    managed="$service_pid"
    for _ in $(seq 1 80); do
        if [[ "$(lock_owner 2>/dev/null || true)" == "$managed" ]] \
            && service_health; then
            break
        fi
        sleep 0.25
    done
    require_one_service "$managed"
    process_env_contains "$managed" 'CODEX_INFO_SYSTEMD_MANAGED=1' \
        || fail "$case_label: managed activation owner lacks its marker"
    if [[ -e "/proc/$unmanaged" ]]; then
        wait "$unmanaged" 2>/dev/null || true
    fi
    [[ ! -e "/proc/$unmanaged" ]] \
        || fail "$case_label: managed activation left the old unmanaged owner alive"
    stop_current_service
    require_no_service
    printf 'CASE %s: PASS (managed activation retired exact unmanaged owner %s and adopted %s)\n' \
        "$case_label" "$unmanaged" "$managed"
}

run_managed_activation_reuses_managed_owner() {
    local owner second
    setup_case managed-activation-reuses-managed
    write_fixture
    launch_managed_service managed-owner
    require_ready
    owner="$service_pid"
    require_one_service "$owner"
    launch_managed_service managed-reuse
    second="$service_pid"
    if ! wait "$second"; then
        fail "$case_label: managed activation failed while reusing a healthy managed owner"
    fi
    require_one_service "$owner"
    [[ "$(listener_count "$case_port")" == 1 ]] \
        || fail "$case_label: managed owner listener count changed during reuse"
    stop_current_service
    require_no_service
    printf 'CASE %s: PASS (healthy managed owner %s was reused; contender %s exited cleanly)\n' \
        "$case_label" "$owner" "$second"
}

run_managed_activation_rejects_malformed_owner() {
    local payload='{"pid":1}'
    setup_case managed-malformed-owner
    write_fixture
    printf '%s\n' "$payload" >"$case_lock"
    chmod 600 "$case_lock"
    launch_managed_service managed-malformed
    wait_for_expected_failure "$service_pid" 'malformed-owner activation'
    [[ -f "$case_lock" ]] || fail "$case_label: malformed owner lock was removed"
    rg -q --fixed-strings -- "$payload" "$case_lock" \
        || fail "$case_label: malformed owner lock was rewritten"
    [[ "$(listener_count "$case_port")" == 0 ]] \
        || fail "$case_label: malformed owner activation bound an unknown listener"
    printf 'CASE %s: PASS (malformed owner stayed untouched and activation failed closed)\n' \
        "$case_label"
}

run_recorder_failure_budget() {
    local mode
    setup_case recorder-busy-retry
    write_fixture
    launch_failure_service busy recorder-busy
    require_ready
    if ! wait_for_account_baseline; then
        sed -n '1,160p' "$case_root/recorder-busy.log" >&2 || true
        fail "$case_label: busy injection did not reach the next-cycle commit"
    fi
    assert_recorder_state ready "$service_pid" \
        || fail "$case_label: busy retry did not return to ready state"
    process_matches_scope "$service_pid" service \
        || fail "$case_label: busy retry service exited unexpectedly"
    stop_current_service
    require_no_service
    printf 'CASE %s: PASS (one 2s busy failure, one next-cycle retry, ready state restored)\n' \
        "$case_label"

    for mode in worker-death fatal full readonly; do
        setup_case "recorder-failure-$mode"
        write_fixture
        launch_failure_service "$mode" "recorder-$mode"
        wait_for_expected_failure "$service_pid" "$mode failure"
        require_no_service
        printf 'CASE %s: PASS (injected %s failure exited nonzero and released owner/listener)\n' \
            "$case_label" "$mode"
    done
}

run_service_cold_start
run_ui_without_service
run_ui_with_service
run_verified_ui_failure_without_service
run_simultaneous_ui_without_service
run_simultaneous_service_launches
run_managed_activation_retires_unmanaged_owner
run_managed_activation_reuses_managed_owner
run_managed_activation_rejects_malformed_owner
run_recorder_failure_budget

if ((hold_count > 0)); then
    printf 'record-daemon-e2e: HOLD (%s UI case(s) could not be rendered; no HOLD was reported as PASS)\n' \
        "$hold_count"
    exit 2
fi
printf 'record-daemon-e2e: PASS (finite service/ui/concurrent cases verified with isolated HOME/XDG/data/ports)\n'

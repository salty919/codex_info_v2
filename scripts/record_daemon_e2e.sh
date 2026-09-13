#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

# Finite split-process acceptance.  The recorder is the only process that
# reads Session files or writes SQLite; REST reads that same database and the
# UI is an HTTP client.  Every process and mutable input is owned by one
# temporary case, so cleanup can be identity-checked before sending a signal.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT_DIR"
ROOT_VERSION="$(awk '
    /^\[package\]$/ { in_package=1; next }
    /^\[/ { in_package=0 }
    in_package && $1 == "version" && $2 == "=" { gsub(/"/, "", $3); print $3; exit }
' Cargo.toml)"
[[ "$ROOT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
    echo 'record-daemon-e2e: root Cargo.toml package version is unavailable' >&2
    exit 1
}

fail() {
    echo "record-daemon-e2e: $*" >&2
    if [[ -n "${case_root:-}" && -d "${case_root:-}" ]]; then
        sed -n '1,120p' "$case_root"/*.log >&2 2>/dev/null || true
    fi
    exit 1
}

for command in awk curl date python3 rg sed sqlite3 ss stat tail tr xprop xwininfo xdpyinfo; do
    command -v "$command" >/dev/null || fail "$command is required"
done

RECORDER_BINARY="$ROOT_DIR/target/release/codex_info_recorder"
REST_BINARY="$ROOT_DIR/target/release/codex_info_rest"
UI_BINARY="$ROOT_DIR/target/release/codex_info"
for binary in "$RECORDER_BINARY" "$REST_BINARY" "$UI_BINARY"; do
    [[ -f "$binary" && -x "$binary" ]] || fail "build executable first: $binary"
done

[[ -n "${DISPLAY:-}" ]] || fail 'DISPLAY is required for the client-only UI case'
xdpyinfo >/dev/null 2>&1 || fail 'X11 display is unavailable for the client-only UI case'

RECORDER_UNIT="$ROOT_DIR/packaging/codex-info-recorder.service"
REST_UNIT="$ROOT_DIR/packaging/codex-info-rest.service"
[[ -f "$RECORDER_UNIT" ]] || fail "missing unit: $RECORDER_UNIT"
[[ -f "$REST_UNIT" ]] || fail "missing unit: $REST_UNIT"
for contract in \
    'ExecStart=%h/.local/bin/codex_info_recorder' \
    'Restart=always' \
    'RestartSec=5s' \
    'StartLimitIntervalSec=0' \
    'NoNewPrivileges=true'; do
    rg -q --fixed-strings -- "$contract" "$RECORDER_UNIT" \
        || fail "recorder unit contract missing: $contract"
done
for contract in \
    'ExecStart=%h/.local/bin/codex_info_rest --port 8787' \
    'Restart=always' \
    'RestartSec=5s' \
    'NoNewPrivileges=true'; do
    rg -q --fixed-strings -- "$contract" "$REST_UNIT" \
        || fail "REST unit contract missing: $contract"
done
rg -q --fixed-strings -- 'After=default.target codex-info-recorder.service' "$REST_UNIT" \
    || fail 'REST unit does not follow the recorder unit'

temp_parent="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
temp_parent="$(cd -- "$temp_parent" 2>/dev/null && pwd -P)" \
    || fail 'temporary parent is unavailable'
case "$temp_parent/" in
    "$ROOT_DIR/"*) fail 'temporary acceptance data must stay outside the repository' ;;
esac
tmp_root="$(mktemp -d "$temp_parent/codex-info-recorder-rest-e2e.XXXXXX")"
case_root=""
case_home=""
case_data=""
case_db=""
sessions_root=""
session_file=""
fixture_event_time=""
case_port=""
common_env=()
recorder_pid=""
rest_pid=""
ui_pid=""
sentinel_pid=""
ui_rest_connection_baseline=""
port_seed=$((35000 + (BASHPID % 10000)))

listener_count() {
    local port="$1"
    ss -ltnH "sport = :$port" 2>/dev/null \
        | awk '$1 == "LISTEN" { count += 1 } END { print count + 0 }'
}

reserve_port() {
    while ((port_seed < 60000)); do
        case_port="$port_seed"
        port_seed=$((port_seed + 1))
        if [[ "$(listener_count "$case_port")" == 0 ]]; then
            return 0
        fi
    done
    fail 'could not find an unused loopback port'
}

process_env_contains() {
    local pid="$1" needle="$2" env_text
    [[ -r "/proc/$pid/environ" ]] || return 1
    env_text="$(tr '\0' '\n' 2>/dev/null <"/proc/$pid/environ" || true)"
    rg -Fqx -- "$needle" <<<"$env_text" >/dev/null
}

process_cmdline() {
    local pid="$1"
    tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null || true
}

process_matches_scope() {
    local pid="$1" kind="$2" cmdline exe
    [[ "$pid" =~ ^[0-9]+$ ]] || return 1
    [[ -e "/proc/$pid" ]] || return 1
    exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
    process_env_contains "$pid" "CODEX_INFO_DATA_DIR=$case_data" || return 1
    cmdline="$(process_cmdline "$pid")"
    case "$kind" in
        recorder)
            [[ "$exe" == "$RECORDER_BINARY" ]] \
                && [[ "$cmdline" == *"--sessions-root $sessions_root"* ]] \
                && [[ "$cmdline" == *"--interval-secs 1"* ]]
            ;;
        rest)
            [[ "$exe" == "$REST_BINARY" ]] \
                && [[ "$cmdline" == *"--port $case_port"* ]]
            ;;
        ui)
            [[ "$exe" == "$UI_BINARY" ]] \
                && [[ "$cmdline" == *"--ui"* ]] \
                && [[ "$cmdline" == *"--port $case_port"* ]] \
                && process_env_contains "$pid" 'CODEX_INFO_UI_CLIENT_ONLY=1'
            ;;
        *)
            return 1
            ;;
    esac
}

find_scoped_pids() {
    local kind="$1" proc pid
    for proc in /proc/[0-9]*; do
        [[ -d "$proc" ]] || continue
        pid="${proc##*/}"
        if process_matches_scope "$pid" "$kind"; then
            printf '%s\n' "$pid"
        fi
    done
}

assert_one_scoped_process() {
    local kind="$1" expected="$2" label="$3" pids=()
    mapfile -t pids < <(find_scoped_pids "$kind")
    [[ "${#pids[@]}" -eq 1 && "${pids[0]}" == "$expected" ]] \
        || fail "$label: expected only PID $expected, found ${pids[*]:-none}"
    process_matches_scope "$expected" "$kind" \
        || fail "$label: PID $expected no longer has its scoped identity"
}

assert_no_scoped_process() {
    local kind="$1" label="$2" pids=()
    mapfile -t pids < <(find_scoped_pids "$kind")
    [[ "${#pids[@]}" -eq 0 ]] \
        || fail "$label: scoped process remains (${pids[*]})"
}

terminate_scoped_pid() {
    local pid="$1" kind="$2" label="$3" waited=0
    [[ -n "$pid" ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return 0
    fi
    process_matches_scope "$pid" "$kind" \
        || fail "refusing to terminate unverified $label PID $pid"
    kill -TERM "$pid" 2>/dev/null || true
    # These bounded waits are the existing daemon E2E lifecycle budget.  A
    # forceful signal is sent only while the exact executable/data identity is
    # still present; a recycled PID is never escalated.
    for _ in $(seq 1 40); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
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
    wait "$pid" 2>/dev/null || waited=$?
    if kill -0 "$pid" 2>/dev/null; then
        fail "$label PID $pid did not terminate (status=$waited)"
    fi
    return 0
}

stop_kind() {
    local kind="$1" label="$2" pid pids=()
    mapfile -t pids < <(find_scoped_pids "$kind")
    for pid in "${pids[@]}"; do
        terminate_scoped_pid "$pid" "$kind" "$label"
    done
}

cleanup() {
    set +e
    if [[ -n "${case_data:-}" ]]; then
        stop_kind ui UI >/dev/null 2>&1 || true
        stop_kind rest REST >/dev/null 2>&1 || true
        stop_kind recorder recorder >/dev/null 2>&1 || true
    fi
    if [[ -n "${sentinel_pid:-}" ]] && kill -0 "$sentinel_pid" 2>/dev/null; then
        kill -TERM "$sentinel_pid" 2>/dev/null || true
        wait "$sentinel_pid" 2>/dev/null || true
    fi
    case "$tmp_root" in
        "$temp_parent"/codex-info-recorder-rest-e2e.*) rm -rf -- "$tmp_root" ;;
        *) echo "record-daemon-e2e: refusing to clean unexpected path $tmp_root" >&2 ;;
    esac
}
trap cleanup EXIT

read_sql() {
    local query="$1"
    [[ -f "$case_db" ]] || return 1
    sqlite3 -batch -readonly -bail -cmd '.timeout 2000' "$case_db" "$query" \
        2>/dev/null
}

is_uint() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

wait_for_quota_observation() {
    local minimum_inclusive="${1:-1}"
    local row="" observed="" reset="" window=""
    is_uint "$minimum_inclusive" || return 1
    for _ in $(seq 1 80); do
        row="$(read_sql "
            SELECT json_extract(d.snapshot_json, '$.source_timestamp')
                   || '|' || json_extract(d.snapshot_json, '$.reset_at')
                   || '|' || c.window_seconds
            FROM durable_state AS d
            JOIN collection_generation AS c
              ON c.singleton = 1
             AND c.reset_at = json_extract(d.snapshot_json, '$.reset_at')
            WHERE d.singleton >= 2
              AND json_valid(d.snapshot_json)
              AND json_type(d.snapshot_json, '$.source_timestamp') = 'integer'
              AND json_type(d.snapshot_json, '$.remaining_percent')
                  IN ('integer', 'real')
            ORDER BY json_extract(d.snapshot_json, '$.source_timestamp') DESC,
                     d.data_generation DESC,
                     d.singleton DESC
            LIMIT 1
        " 2>/dev/null || true)"
        IFS='|' read -r observed reset window <<<"$row"
        if is_uint "$observed" && is_uint "$reset" && is_uint "$window" \
            && ((10#$observed > 0 && 10#$reset > 0 && 10#$window > 0 \
                && 10#$observed >= 10#$minimum_inclusive \
                && 10#$observed >= 10#$reset - 10#$window \
                && 10#$observed <= 10#$reset)); then
            printf '%s\n' "$observed"
            return 0
        fi
        sleep 0.25
    done
    return 1
}

read_account_activation() {
    python3 - "$case_data/history/account_profile_v1.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    document = json.load(source)
accounts = document.get("accounts")
if not isinstance(accounts, dict) or len(accounts) != 1:
    raise SystemExit(1)
entry = next(iter(accounts.values()))
intervals = entry.get("lifecycle_intervals")
if not isinstance(intervals, list) or len(intervals) != 1:
    raise SystemExit(1)
current = intervals[0]
if not isinstance(current, dict) or current.get("end_at") is not None:
    raise SystemExit(1)
activation = current.get("start_at")
if not isinstance(activation, int) or isinstance(activation, bool) or activation <= 0:
    raise SystemExit(1)
if entry.get("activation_timestamp") != activation:
    raise SystemExit(1)
print(activation)
PY
}

write_fixture() {
    local now minute baseline_time auth_file
    now="$(date -u +%s)"
    minute=$((now - now % 60))
    baseline_time=$((minute - 180))
    mkdir -p "$sessions_root" "$case_data/history"
    chmod 700 "$case_home" "$case_data"
    auth_file="$case_home/auth.json"
    printf '%s\n' '{"auth_mode":"chatgpt","tokens":{"account_id":"fixture-account-129"}}' \
        >"$auth_file"
    chmod 600 "$auth_file"
    session_file="$sessions_root/recorder-rest.jsonl"
    printf '%s\n' \
        "{\"type\":\"event_msg\",\"timestamp\":\"$(date -u -d "@$baseline_time" +%Y-%m-%dT%H:%M:%SZ)\",\"payload\":{\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}}" \
        "{\"type\":\"event_msg\",\"timestamp\":\"$(date -u -d "@$baseline_time" +%Y-%m-%dT%H:%M:%SZ)\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":120,\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20}}}}" \
        >"$session_file"
    chmod 600 "$session_file"
}

append_session_usage() {
    local event_time="$1" total="$2" input="$3" cached="$4" output="$5"
    local timestamp
    timestamp="$(date -u -d "@$event_time" +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\n' \
        "{\"type\":\"event_msg\",\"timestamp\":\"$timestamp\",\"payload\":{\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}}" \
        "{\"type\":\"event_msg\",\"timestamp\":\"$timestamp\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":$total,\"input_tokens\":$input,\"cached_input_tokens\":$cached,\"output_tokens\":$output}}}}" \
        >>"$session_file"
}

setup_case() {
    case_root="$tmp_root/split-process"
    case_home="$case_root/codex"
    case_data="$case_root/data"
    case_db=""
    sessions_root="$case_home/sessions"
    recorder_pid=""
    rest_pid=""
    ui_pid=""
    mkdir -p "$case_root/home" "$case_root/xdg-config" "$case_root/xdg-data" \
        "$case_root/xdg-cache" "$case_root/xdg-state" "$case_root/xdg-runtime" \
        "$case_home" "$case_data"
    chmod 700 "$case_root" "$case_home" "$case_data" "$case_root/xdg-runtime"
    reserve_port
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
        "CODEX_INFO_DAEMON_INTERVAL_SECS=1"
        "CODEX_INFO_DEBUG=1"
        "LC_ALL=C"
    )
}

launch_recorder() {
    local log_name="$1"
    env "${common_env[@]}" "$RECORDER_BINARY" \
        --sessions-root "$sessions_root" \
        --interval-secs 1 \
        >"$case_root/$log_name.log" 2>&1 &
    recorder_pid="$!"
}

launch_rest() {
    local log_name="$1"
    env "${common_env[@]}" "$REST_BINARY" \
        --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    rest_pid="$!"
}

launch_ui() {
    local log_name="$1"
    env "${common_env[@]}" CODEX_INFO_UI_CLIENT_ONLY=1 \
        "$UI_BINARY" --ui --port "$case_port" \
        >"$case_root/$log_name.log" 2>&1 &
    ui_pid="$!"
}

locate_account_partition() {
    local checkpoint_count databases=()
    for _ in $(seq 1 200); do
        databases=("$case_data"/history/accounts/v1/*/epoch-*/usage_history.sqlite3)
        if [[ "${#databases[@]}" -eq 1 ]]; then
            checkpoint_count="$(sqlite3 -batch -bail -cmd '.timeout 2000' \
                "${databases[0]}" 'SELECT COUNT(*) FROM session_checkpoints;' 2>/dev/null || true)"
            if [[ "$checkpoint_count" =~ ^[0-9]+$ ]] && ((10#$checkpoint_count >= 1)); then
                case_db="${databases[0]}"
                break
            fi
        fi
        sleep 0.1
    done
    [[ -n "$case_db" ]] || {
        tail -n 80 "$case_root/recorder.log" >&2 || true
        fail 'recorder did not create a locator-selected account partition'
    }
}

valid_collection_snapshot() {
    local generation="$1" epoch="$2" cycle="$3"
    is_uint "$generation" && is_uint "$cycle" \
        && ((10#$generation > 0 && 10#$cycle > 0)) \
        && [[ "$epoch" =~ ^[0-9a-f]{32}$ ]]
}

collection_generation_state() {
    read_sql 'SELECT data_generation || "|" || COALESCE(collector_epoch, "") || "|" || cycle_seq FROM collection_generation WHERE singleton = 1'
}

canonical_range_count() {
    local epoch="$1"
    [[ "$epoch" =~ ^[0-9a-f]{32}$ ]] || return 1
    read_sql "SELECT COUNT(*) FROM session_ranges WHERE collector_epoch='$epoch'"
}

canonical_checkpoint_offset() {
    local epoch="$1"
    [[ "$epoch" =~ ^[0-9a-f]{32}$ ]] || return 1
    read_sql "SELECT COALESCE(MAX(committed_offset),0) FROM session_checkpoints WHERE collector_epoch='$epoch'"
}

canonical_model_total() {
    read_sql 'SELECT COALESCE(SUM(CAST(total_tokens AS INTEGER)),0) FROM session_model_totals'
}

recorder_projection_snapshot() {
    local state generation epoch cycle checkpoint ranges models
    state="$(collection_generation_state)" || return 1
    IFS='|' read -r generation epoch cycle <<<"$state"
    checkpoint="$(canonical_checkpoint_offset "$epoch")" || return 1
    ranges="$(canonical_range_count "$epoch")" || return 1
    models="$(canonical_model_total)" || return 1
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$generation" "$epoch" "$cycle" "$checkpoint" "$ranges" "$models"
}

wait_for_recorder_generation() {
    local minimum="$1" snapshot="" generation="" epoch="" cycle="" checkpoint="" ranges="" models=""
    for _ in $(seq 1 80); do
        snapshot="$(recorder_projection_snapshot 2>/dev/null || true)"
        IFS=$'\t' read -r generation epoch cycle checkpoint ranges models <<<"$snapshot"
        if valid_collection_snapshot "$generation" "$epoch" "$cycle" \
            && is_uint "$checkpoint" && is_uint "$ranges" && is_uint "$models" \
            && ((10#$generation >= minimum && 10#$checkpoint > 0)); then
            printf '%s\n' "$snapshot"
            return 0
        fi
        sleep 0.25
    done
    printf '%s\n' "${snapshot:-0}"
    return 1
}

health_body() {
    curl --fail --silent --show-error --max-time 1 \
        "http://127.0.0.1:$case_port/v1/health"
}

wait_for_rest() {
    for _ in $(seq 1 60); do
        if process_matches_scope "$rest_pid" rest \
            && [[ "$(listener_count "$case_port")" == 1 ]] \
            && health_body >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

assert_health() {
    local body="$1"
    python3 - "$body" "$ROOT_VERSION" <<'PY'
import json
import sys

document = json.loads(sys.argv[1])
root_version = sys.argv[2]
if set(document) != {"api_version", "service", "product_version"}:
    raise SystemExit("REST health key set changed")
if (document["api_version"] != "v1" or document["service"] != "codex-info"
        or document["product_version"] != root_version):
    raise SystemExit("REST health identity changed")
if not isinstance(document["product_version"], str) or not document["product_version"]:
    raise SystemExit("REST health product version is empty")
PY
}

assert_details_file() {
    local path="$1"
    python3 - "$path" <<'PY'
import json
import sys

document = json.load(open(sys.argv[1], encoding="utf-8"))
expected = {
    "api_version", "state", "observed_at", "authenticated", "plan_label", "quota",
    "models", "active_thread_count", "history_periods", "history_samples",
    "history_gaps", "threads", "estimated_cost_label",
}
if set(document) != expected or document["api_version"] != "v1":
    raise SystemExit("REST details wire contract changed")
if document["state"] != "ready" or document["authenticated"] is not True:
    raise SystemExit("REST details are not an authenticated ready snapshot")
if not isinstance(document["observed_at"], int) or document["observed_at"] <= 0:
    raise SystemExit("REST details observed_at is empty")

quota = document["quota"]
if not isinstance(quota, dict) or set(quota) != {
    "remaining_percent", "reset_at", "window_seconds", "monthly",
}:
    raise SystemExit("REST details quota is missing")
if not isinstance(quota["remaining_percent"], (int, float)) \
        or not 0 <= quota["remaining_percent"] <= 100 \
        or not isinstance(quota["reset_at"], int) or quota["reset_at"] <= 0 \
        or not isinstance(quota["window_seconds"], int) or quota["window_seconds"] <= 0:
    raise SystemExit("REST details quota is invalid")

history = document["history_samples"]
if not isinstance(history, list) or not history:
    raise SystemExit("REST details history_samples is empty")
if not any(sample.get("remaining_percent") is not None for sample in history):
    raise SystemExit("REST details has no quota-backed history sample")

models = document["models"]
if not isinstance(models, list) or not models:
    raise SystemExit("REST details models is empty")
if not any(
    isinstance(model.get("name"), str) and model["name"]
    and (model.get("input_tokens", 0) + model.get("output_tokens", 0)) > 0
    for model in models
):
    raise SystemExit("REST details contains no non-zero model usage")
PY
}

fetch_details() {
    local path="$1"
    curl --fail --silent --show-error --max-time 1 \
        "http://127.0.0.1:$case_port/v1/details" >"$path"
    assert_details_file "$path"
}

assert_details_matches_sqlite() {
    local path="$1" quota_row="" max_luna_tokens="" model_total=""
    quota_row="$(read_sql 'SELECT timestamp || "|" || reset_at || "|" || remaining_percent FROM usage_history WHERE remaining_percent IS NOT NULL ORDER BY timestamp DESC, reset_at DESC LIMIT 1' 2>/dev/null || true)"
    max_luna_tokens="$(read_sql 'SELECT COALESCE(MAX(luna_tokens),0) FROM usage_history' 2>/dev/null || true)"
    model_total="$(canonical_model_total 2>/dev/null || true)"
    is_uint "$max_luna_tokens" || fail 'luna token total is not readable from the same SQLite database'
    is_uint "$model_total" || fail 'recorder model total is not readable from the same SQLite database'
    [[ -n "$quota_row" ]] || fail 'usage_history has no SQLite quota row for REST read-back'
    python3 - "$path" "$quota_row" "$max_luna_tokens" "$model_total" <<'PY'
import json
import sys

details = json.load(open(sys.argv[1], encoding="utf-8"))
raw = sys.argv[2].split("|", 2)
if len(raw) != 3:
    raise SystemExit("SQLite usage_history quota row is malformed")
timestamp, reset_at, remaining = raw
timestamp = int(timestamp)
reset_at = int(reset_at)
remaining = float(remaining)
quota = details["quota"]
if abs(float(quota["remaining_percent"]) - remaining) >= 1e-9:
    raise SystemExit("REST quota lost the SQLite quota observation")
if abs(int(quota["reset_at"]) - reset_at) > 60:
    raise SystemExit("REST quota lost the SQLite reset boundary")
max_luna_tokens = int(sys.argv[3])
if max_luna_tokens > 0 and max(
    int(row["luna_tokens"]) for row in details["history_samples"]
) < max_luna_tokens:
    raise SystemExit("REST history lost SQLite token usage")

expected_model_total = int(sys.argv[4])
observed_model_total = sum(
    int(model["input_tokens"]) + int(model.get("cached_input_tokens", 0))
    + int(model["output_tokens"])
    for model in details["models"]
)
if observed_model_total < expected_model_total:
    raise SystemExit("REST models lost recorder SQLite token usage")
PY
}

assert_model_growth() {
    local before="$1" after="$2"
    python3 - "$before" "$after" <<'PY'
import json
import sys

before = json.load(open(sys.argv[1], encoding="utf-8"))
after = json.load(open(sys.argv[2], encoding="utf-8"))
old = {
    model["name"]: int(model["input_tokens"]) + int(model["output_tokens"])
    for model in before["models"]
}
new = {
    model["name"]: int(model["input_tokens"]) + int(model["output_tokens"])
    for model in after["models"]
}
if not any(name in old and total > old[name] for name, total in new.items()):
    raise SystemExit("REST model details did not advance after Session append")
PY
}

wait_for_recorder_advance() {
    local before="$1" model_before="$2" ranges_before="$3" checkpoint_before="$4" require_quota="${5:-0}"
    local before_generation="" before_epoch="" before_cycle="" generation="" epoch="" cycle=""
    local checkpoint="" ranges="" models="" history="" quota="" snapshot=""
    IFS='|' read -r before_generation before_epoch before_cycle <<<"$before"
    valid_collection_snapshot "$before_generation" "$before_epoch" "$before_cycle" || return 1
    # collection_generation advances on every acknowledged heartbeat, while
    # ranges and checkpoints retain the cycle that admitted their source
    # bytes. Prove the append from durable growth relative to the baseline;
    # equality with the moving current cycle would make success timing-only.
    for _ in $(seq 1 80); do
        process_matches_scope "$recorder_pid" recorder || return 1
        snapshot="$(recorder_projection_snapshot 2>/dev/null || true)"
        IFS=$'\t' read -r generation epoch cycle checkpoint ranges models <<<"$snapshot"
        if valid_collection_snapshot "$generation" "$epoch" "$cycle" \
            && [[ "$epoch" == "$before_epoch" ]] \
            && is_uint "$checkpoint" && is_uint "$ranges" && is_uint "$models" \
            && ((10#$generation > 10#$before_generation \
                && 10#$cycle > 10#$before_cycle \
                && 10#$ranges > 10#$ranges_before \
                && 10#$checkpoint > 10#$checkpoint_before \
                && 10#$models > 10#$model_before)); then
            if ((require_quota)); then
                history="$(read_sql 'SELECT COUNT(*) FROM usage_history' 2>/dev/null || true)"
                quota="$(read_sql 'SELECT COUNT(*) FROM usage_history WHERE remaining_percent IS NOT NULL' 2>/dev/null || true)"
                is_uint "$history" && is_uint "$quota" \
                    && ((10#$history > 0 && 10#$quota > 0)) || {
                        sleep 0.25
                        continue
                    }
            fi
            printf '%s\n' "$snapshot"
            return 0
        fi
        sleep 0.25
    done
    return 1
}

wait_for_ui_window() {
    local pid="$1" candidate window_pid
    for _ in $(seq 1 60); do
        if ! process_matches_scope "$pid" ui; then
            return 1
        fi
        while read -r candidate; do
            window_pid="$(xprop -id "$candidate" _NET_WM_PID 2>/dev/null \
                | awk -F'= ' '{print $2}' | tr -d '[:space:]')"
            [[ "$window_pid" == "$pid" ]] && return 0
        done < <(xwininfo -root -tree 2>/dev/null \
            | awk '/^ +0x[0-9a-f]+/ {print $1}')
        sleep 0.25
    done
    return 1
}

wait_for_ui_rest_connection() {
    local pid="$1" tuple
    for _ in $(seq 1 60); do
        if ! process_matches_scope "$pid" ui; then
            return 1
        fi
        # The client uses short-lived HTTP connections that normally reach
        # TIME_WAIT before a 250 ms process-table sample. Either endpoint can
        # own TIME_WAIT, so compare both directions with the pre-launch
        # baseline instead of relying on a chance ESTABLISHED observation.
        while IFS= read -r tuple; do
            [[ -n "$tuple" ]] || continue
            if ! rg -Fqx -- "$tuple" <<<"$ui_rest_connection_baseline"; then
                return 0
            fi
        done < <(rest_connection_tuples)
        sleep 0.25
    done
    return 1
}

rest_connection_tuples() {
    local endpoint
    printf -v endpoint '0100007F:%04X' "$case_port"
    awk -v endpoint="$endpoint" '
        NR > 1 && (toupper($2) == endpoint || toupper($3) == endpoint) {
            print toupper($2 "|" $3)
        }
    ' /proc/net/tcp 2>/dev/null
}

setup_case
write_fixture

# The sentinel is deliberately outside the product scope.  It must survive
# every product termination below, proving that PID cleanup is not a broad
# process-group or name-based kill.
tail -f /dev/null &
sentinel_pid="$!"
kill -0 "$sentinel_pid" 2>/dev/null || fail 'scope sentinel did not start'

launch_recorder recorder
locate_account_partition
for _ in $(seq 1 20); do
    process_matches_scope "$recorder_pid" recorder && break
    sleep 0.1
done
assert_one_scoped_process recorder "$recorder_pid" 'recorder startup'
baseline_snapshot="$(wait_for_recorder_generation 1 2>/dev/null || true)"
[[ "$baseline_snapshot" != 0 ]] \
    || fail 'recorder did not produce the first durable generation/ack'
IFS=$'\t' read -r baseline_generation baseline_epoch baseline_cycle baseline_checkpoint baseline_ranges baseline_model <<<"$baseline_snapshot"
valid_collection_snapshot "$baseline_generation" "$baseline_epoch" "$baseline_cycle" \
    || fail 'first recorder acknowledgement has invalid collection_generation identity'
is_uint "$baseline_checkpoint" && is_uint "$baseline_ranges" && is_uint "$baseline_model" \
    || fail 'first recorder acknowledgement has invalid range/checkpoint/model readback'

# The registry owns the inclusive account boundary. The quota observation's
# exact source second (not usage_history's graph-minute bucket) proves that the
# Session bytes belong to both the account lifecycle and the admitted period.
# JSONL byte order owns counter order, so the baseline and delta may correctly
# share this observed second without inventing timestamps or SQLite rows.
fixture_activation_time="$(read_account_activation 2>/dev/null || true)"
is_uint "$fixture_activation_time" && ((10#$fixture_activation_time > 0)) \
    || fail 'recorder did not persist the account activation boundary'
fixture_event_time="$(wait_for_quota_observation \
    "$fixture_activation_time" 2>/dev/null || true)"
is_uint "$fixture_event_time" && ((10#$fixture_event_time >= 10#$fixture_activation_time)) \
    || fail 'recorder did not publish an account-owned quota observation before Session input'
append_session_usage "$fixture_event_time" 240 200 160 40
append_session_usage "$fixture_event_time" 360 300 240 60
first_snapshot="$(wait_for_recorder_advance \
    "$baseline_generation|$baseline_epoch|$baseline_cycle" "$baseline_model" \
    "$baseline_ranges" "$baseline_checkpoint" 1 2>/dev/null || true)"
[[ -n "$first_snapshot" && "$first_snapshot" != 0 ]] \
    || fail 'recorder did not produce non-empty quota/history/models from Session input'
IFS=$'\t' read -r first_generation first_epoch first_cycle first_checkpoint first_ranges first_model <<<"$first_snapshot"
valid_collection_snapshot "$first_generation" "$first_epoch" "$first_cycle" \
    || fail 'first append acknowledgement has invalid collection_generation identity'
assert_one_scoped_process recorder "$recorder_pid" 'recorder projection'

launch_rest rest-initial
assert_one_scoped_process rest "$rest_pid" 'REST startup'
[[ "$rest_pid" != "$recorder_pid" ]] || fail 'recorder and REST share a PID'
wait_for_rest || fail 'REST did not become healthy over the recorder SQLite database'
assert_health "$(health_body)" || fail 'REST health contract failed'
details_before="$case_root/details-before.json"
fetch_details "$details_before" \
    || fail 'REST did not publish non-empty details after recorder append'
assert_details_matches_sqlite "$details_before" \
    || fail 'REST details did not read the recorder SQLite database'

generation_before="$first_generation"
epoch_before="$first_epoch"
cycle_before="$first_cycle"
model_before="$first_model"
ranges_before="$first_ranges"
checkpoint_before="$first_checkpoint"
is_uint "$generation_before" && is_uint "$model_before" \
    && is_uint "$ranges_before" && is_uint "$checkpoint_before" \
    || fail 'recorder canonical state is not numeric before REST outage'
process_matches_scope "$recorder_pid" recorder \
    || fail 'recorder identity changed before REST outage'
recorder_pid_before="$recorder_pid"

# REST is stopped first. The next Session append must still be committed by
# the same recorder PID, with both a fresh durable acknowledgement and a
# strictly larger model total.
rest_pid_before="$rest_pid"
terminate_scoped_pid "$rest_pid" rest REST
rest_pid=""
assert_no_scoped_process rest 'REST shutdown'
[[ "$(listener_count "$case_port")" == 0 ]] \
    || fail 'REST listener remained after its scoped shutdown'
[[ "$recorder_pid" == "$recorder_pid_before" ]] \
    || fail 'recorder PID changed when REST stopped'
process_matches_scope "$recorder_pid" recorder \
    || fail 'recorder stopped or changed executable during REST outage'

# REST ownership ends at the HTTP projection. It does not own quota polling,
# Session byte order, or recorder commits. Reuse the already-admitted event
# second so this phase tests only a later source append during the REST outage.
append_session_usage "$fixture_event_time" 480 400 320 80
outage_snapshot=""
outage_snapshot="$(wait_for_recorder_advance \
    "$generation_before|$epoch_before|$cycle_before" "$model_before" \
    "$ranges_before" "$checkpoint_before" 0 2>/dev/null || true)"
[[ -n "$outage_snapshot" && "$outage_snapshot" != 0 ]] \
    || fail 'recorder did not advance a durable generation while REST was stopped'
IFS=$'\t' read -r outage_generation outage_epoch outage_cycle _ _ _ _ <<<"$outage_snapshot"
valid_collection_snapshot "$outage_generation" "$outage_epoch" "$outage_cycle" \
    || fail 'REST-outage acknowledgement has invalid collection_generation identity'
[[ "$recorder_pid" == "$recorder_pid_before" ]] \
    || fail 'recorder PID changed after Session append during REST outage'
assert_one_scoped_process recorder "$recorder_pid" 'recorder during REST outage'

# Restart REST against the exact same database path and require the changed
# model/history details to be read back, rather than accepting an empty root.
launch_rest rest-restart
assert_one_scoped_process rest "$rest_pid" 'REST restart'
[[ "$rest_pid" != "$recorder_pid" ]] || fail 'recorder and restarted REST share a PID'
[[ "$rest_pid" != "$rest_pid_before" ]] || fail 'REST restart reused the stopped process instance'
wait_for_rest || fail 'REST did not recover after restart'
details_after="$case_root/details-after.json"
fetch_details "$details_after" \
    || fail 'REST restart did not recover non-empty details'
assert_details_matches_sqlite "$details_after" \
    || fail 'REST restart details did not read the same SQLite database'
assert_model_growth "$details_before" "$details_after" \
    || fail 'REST restart did not expose the recorder model/token generation'

# The UI receives only the REST endpoint in this invocation. Its executable,
# marker, and port are checked independently, while recorder and REST PIDs
# remain untouched and no UI-owned recorder process may appear.
ui_rest_connection_baseline="$(rest_connection_tuples)"
launch_ui ui-client-only
assert_one_scoped_process ui "$ui_pid" 'client-only UI startup'
[[ "$ui_pid" != "$recorder_pid" && "$ui_pid" != "$rest_pid" ]] \
    || fail 'UI shares a product process PID'
wait_for_ui_window "$ui_pid" \
    || fail 'client-only UI did not render a window connected to the REST case'
wait_for_ui_rest_connection "$ui_pid" \
    || fail 'client-only UI did not open a loopback connection to REST'
assert_one_scoped_process recorder "$recorder_pid" 'client-only UI recorder isolation'
assert_one_scoped_process rest "$rest_pid" 'client-only UI REST isolation'
assert_one_scoped_process ui "$ui_pid" 'client-only UI identity'

# Stop exactly the three scoped product instances and verify the unrelated
# sentinel remains alive until the harness deliberately cleans up its own
# fixture. This is the end-of-case process-scope acceptance.
terminate_scoped_pid "$ui_pid" ui 'client-only UI'
ui_pid=""
terminate_scoped_pid "$rest_pid" rest REST
rest_pid=""
terminate_scoped_pid "$recorder_pid" recorder recorder
recorder_pid=""
assert_no_scoped_process ui 'final UI cleanup'
assert_no_scoped_process rest 'final REST cleanup'
assert_no_scoped_process recorder 'final recorder cleanup'
kill -0 "$sentinel_pid" 2>/dev/null \
    || fail 'scoped product termination killed the unrelated sentinel'
sentinel_exe="$(readlink "/proc/$sentinel_pid/exe" 2>/dev/null || true)"
[[ "$sentinel_exe" != "$RECORDER_BINARY" \
    && "$sentinel_exe" != "$REST_BINARY" \
    && "$sentinel_exe" != "$UI_BINARY" ]] \
    || fail 'scope sentinel unexpectedly has a product executable'
kill -TERM "$sentinel_pid" 2>/dev/null || true
wait "$sentinel_pid" 2>/dev/null || true
sentinel_pid=""

printf 'CASE split-process: PASS (recorder/rest separate PID+exe, REST outage recovery, same SQLite read, client-only UI, scoped termination)\n'
printf 'record-daemon-e2e: PASS (tests=1, recorder acknowledgements and non-empty quota/history/models verified)\n'

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

for command in awk date python3 rg sed sqlite3 ss stat tail tr xprop xwininfo xdpyinfo; do
    command -v "$command" >/dev/null || fail "$command is required"
done

E2E_BIN_DIR="${CODEX_INFO_E2E_BIN_DIR:-$ROOT_DIR/target/release}"
[[ "$E2E_BIN_DIR" == /* ]] || fail 'absolute candidate bin directory required'
RECORDER_BINARY="$E2E_BIN_DIR/codex_info_recorder"
REST_BINARY="$E2E_BIN_DIR/codex_info_rest"
UI_BINARY="$E2E_BIN_DIR/codex_info"
WIRE_PROBE="$E2E_BIN_DIR/examples/wire_api_probe"
for binary in "$RECORDER_BINARY" "$REST_BINARY" "$UI_BINARY" "$WIRE_PROBE"; do
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
fixture_reset_at=""
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
    python3 - "$case_data/history/account_profile_v1.json" <<'PY' || return 1
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
    local now minute baseline_time auth_file thread_id thread_title
    now="$(date -u +%s)"
    minute=$((now - now % 60))
    baseline_time=$((minute - 180))
    thread_id='record-daemon-e2e-fixture-thread'
    thread_title='record daemon baseline fixture'
    mkdir -p "$sessions_root" "$case_data/history"
    chmod 700 "$case_home" "$case_data"
    auth_file="$case_home/auth.json"
    printf '%s\n' '{"auth_mode":"chatgpt","tokens":{"account_id":"fixture-account-129"}}' \
        >"$auth_file"
    chmod 600 "$auth_file"
    session_file="$sessions_root/recorder-rest.jsonl"
    printf '%s\n' \
        "{\"type\":\"session_meta\",\"timestamp\":\"$(date -u -d "@$baseline_time" +%Y-%m-%dT%H:%M:%SZ)\",\"payload\":{\"id\":\"$thread_id\",\"session_id\":\"session-$thread_id\"}}" \
        "{\"type\":\"event_msg\",\"timestamp\":\"$(date -u -d "@$baseline_time" +%Y-%m-%dT%H:%M:%SZ)\",\"payload\":{\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}}" \
        "{\"type\":\"event_msg\",\"timestamp\":\"$(date -u -d "@$baseline_time" +%Y-%m-%dT%H:%M:%SZ)\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":120,\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20}}}}" \
        >"$session_file"
    chmod 600 "$session_file"
    fixture_reset_at=$((now + 604800 - 3600))
    common_env+=(
        "CODEX_INFO_FAKE_RESET_AT=$fixture_reset_at"
        "CODEX_INFO_FAKE_THREAD_ID=$thread_id"
        "CODEX_INFO_FAKE_THREAD_PATH=$session_file"
        "CODEX_INFO_FAKE_THREAD_TITLE=$thread_title"
    )
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

# The same qualified receiver owns all test resource images. Only actual
# change notifications release a pending observation; there is no HTTP poll.
wire_snapshot() {
    local resource="$1" output="$2"; shift 2
    python3 "$ROOT_DIR/tests/fixtures/wire_e2e_snapshot.py" --probe "$WIRE_PROBE" \
        --port "$case_port" --cache "$case_root/wire-receipts" \
        --resource "$resource" --output "$output" "$@"
}

runtime_body() {
    wire_snapshot runtime "$case_root/runtime-last"
}

wait_for_rest() {
    for _ in $(seq 1 60); do
        if process_matches_scope "$rest_pid" rest \
            && [[ "$(listener_count "$case_port")" == 1 ]]; then
            runtime_body >/dev/null 2>&1
            return $?
        fi
        sleep 0.25
    done
    return 1
}

assert_runtime() {
    local body="$1"
    python3 - "$body" "$ROOT_VERSION" <<'PY_RUNTIME'
import json
import sys

document = json.loads(sys.argv[1])
if set(document) != {"rest_product_version", "recorder_product_version", "recorder_status"}:
    raise SystemExit("runtime key set changed")
if document["rest_product_version"] != sys.argv[2]:
    raise SystemExit("runtime product version changed")
if document["recorder_status"] not in {"available", "mismatch", "unavailable"}:
    raise SystemExit("runtime recorder status is invalid")
if document["recorder_status"] in {"available", "mismatch"}:
    if not isinstance(document["recorder_product_version"], str) or not document["recorder_product_version"]:
        raise SystemExit("available recorder version is empty")
elif document["recorder_product_version"] is not None:
    raise SystemExit("unavailable recorder version was fabricated")
PY_RUNTIME
}

assert_current_file() {
    local body_path="$1" metadata_path="$2" expected_reset="$3" expected_total="$4"
    python3 - "$body_path" "$metadata_path" "$expected_reset" "$expected_total" <<'PY' || return 1
import json
import math
import re
import sys

body_path, metadata_path, reset_text, total_text = sys.argv[1:]

def reject(reason):
    raise ValueError(reason)

def uint(value):
    return type(value) is int and 0 <= value <= 18446744073709551615

try:
    with open(body_path, encoding="utf-8") as handle:
        current = json.load(handle)
    expected_keys = {
        "acquisition_state", "selection", "thread_summary", "observed_at", "authenticated", "plan_label",
        "quota", "models", "models_status",
    }
    if not isinstance(current, dict) or set(current) != expected_keys:
        reject("Main schema is invalid")
    if (current["acquisition_state"] != "ready" or current["authenticated"] is not True):
        reject("Main is not ready and authenticated")
    if not uint(current["observed_at"]) or current["observed_at"] <= 0:
        reject("Main observed_at is invalid")
    if current["plan_label"] is not None and not isinstance(current["plan_label"], str):
        reject("Main plan_label is invalid")
    summary = current["thread_summary"]
    if summary is not None:
        if (not isinstance(summary, dict) or set(summary) != {
                "open_count", "running_count", "unknown_activity_count", "by_model"}
                or any(not uint(summary[k]) for k in ("open_count", "running_count", "unknown_activity_count"))
                or summary["running_count"] + summary["unknown_activity_count"] > summary["open_count"]
                or sum(row["open_count"] for row in summary["by_model"]) != summary["open_count"]):
            reject("Main thread summary is invalid")
    selection = current["selection"]
    if not isinstance(selection, dict) or set(selection) != {"account_id", "login_id", "is_current"}:
        reject("Main confirmed selection is missing")
    if current["models_status"] != "partial":
        reject("Main did not preserve the partial model status")

    expected_reset = int(reset_text)
    if str(expected_reset) != reset_text:
        reject("fixture reset is not canonical decimal")
    expected_total = int(total_text)
    expected_components = {
        120: (100, 80, 20),
        240: (200, 160, 40),
    }
    if expected_total not in expected_components:
        reject("fixture expected model total is not one of the declared observations")
    quota = current["quota"]
    if not isinstance(quota, dict) or set(quota) != {
        "remaining_percent", "reset_at", "window_seconds", "monthly"
    }:
        reject("Main quota schema is invalid")
    remaining = quota["remaining_percent"]
    if (type(remaining) not in (int, float) or not math.isfinite(remaining)
            or abs(float(remaining) - 44.0) >= 1e-9
            or not uint(quota["reset_at"]) or quota["reset_at"] != expected_reset
            or not uint(quota["window_seconds"]) or quota["window_seconds"] != 604800
            or quota["monthly"] is not False):
        reject("Main quota does not match the pinned weekly observation")

    models = current["models"]
    if not isinstance(models, list):
        reject("Main models is not an array")
    pending = False
    if not models:
        pending = True
    elif len(models) != 1:
        reject("fixture current contains an unexpected model set")
    else:
        model = models[0]
        if not isinstance(model, dict) or set(model) != {
            "model", "total_tokens", "input_tokens", "cached_input_tokens",
            "cache_write_input_tokens", "output_tokens", "estimated_cost",
        }:
            reject("Main model schema is invalid")
        if model["model"] != "gpt-5.6-luna":
            reject("fixture current contains an unexpected model identity")
        for key in ("total_tokens", "input_tokens", "cached_input_tokens", "output_tokens"):
            if not uint(model[key]):
                reject("Main token component is invalid")
        if model["cache_write_input_tokens"] is not None or model["estimated_cost"] is not None:
            reject("fixture cache-write or price was fabricated")
        if (model["total_tokens"] != model["input_tokens"] + model["output_tokens"]
                or model["cached_input_tokens"] > model["input_tokens"]):
            reject("Main model token components are inconsistent")
        if model["total_tokens"] > expected_total:
            reject("Main model total exceeded the owned append")
        if model["total_tokens"] < expected_total:
            pending = True
        elif ((model["input_tokens"], model["cached_input_tokens"], model["output_tokens"])
                != expected_components[expected_total]):
            reject("Main model components differ from the exact Session counters")

    with open(metadata_path, encoding="utf-8") as handle:
        metadata = json.load(handle)
    if (set(metadata) != {"view_key", "image", "complete"} or metadata["complete"] is not True
            or metadata["view_key"] != ["/state", {"parts": "main", "account": None}]
            or not isinstance(metadata["image"], str) or not metadata["image"]):
        reject("Main image is not qualified")
    if pending:
        raise SystemExit(75)
except (OSError, UnicodeError, ValueError, TypeError, KeyError, json.JSONDecodeError) as error:
    raise SystemExit(str(error))
PY
}

assert_history_file() {
    local body_path="$1" metadata_path="$2" expected_reset="$3" expected_total="$4"
    python3 - "$body_path" "$metadata_path" "$expected_reset" "$expected_total" <<'PY' || return 1
import json
import math
import re
import sys

body_path, metadata_path, reset_text, total_text = sys.argv[1:]

def reject(reason):
    raise ValueError(reason)

def uint(value):
    return type(value) is int and 0 <= value <= 18446744073709551615

try:
    with open(body_path, encoding="utf-8") as handle:
        page = json.load(handle)
    if not isinstance(page, dict) or set(page) != {"period_id", "samples", "gaps", "local_usage"}:
        reject("history schema is invalid")
    if not isinstance(page["period_id"], str) or not page["period_id"]:
        reject("history period identity is missing")
    samples = page["samples"]
    if any(not isinstance(page[k], list) for k in ("samples", "gaps", "local_usage")):
        reject("history collections are invalid")

    expected_reset = int(reset_text)
    if str(expected_reset) != reset_text:
        reject("history period is not canonical decimal")
    expected_total = int(total_text)
    if expected_total not in {120, 240}:
        reject("fixture expected model total is not one of the declared observations")
    saw_quota = False
    confirmed_target = False
    max_luna_total = None
    for sample in samples:
        if not isinstance(sample, dict) or set(sample) != {
            "timestamp", "reset_at", "remaining_percent", "task_active_since_previous",
            "models", "models_complete", "model_source",
        }:
            reject("history sample schema is invalid")
        if (not uint(sample["timestamp"]) or not uint(sample["reset_at"])
                or sample["reset_at"] != expected_reset):
            reject("history sample does not belong to the exact requested reset")
        remaining = sample["remaining_percent"]
        if remaining is not None and (
                type(remaining) not in (int, float) or not math.isfinite(remaining)
                or not 0 <= remaining <= 100):
            reject("history quota value is invalid")
        if remaining is not None and abs(float(remaining) - 44.0) < 1e-9:
            saw_quota = True
        task_state = sample["task_active_since_previous"]
        if task_state is not None and type(task_state) is not bool:
            reject("history task state is invalid")
        if type(sample["models_complete"]) is not bool:
            reject("history models_complete is invalid")
        source = sample["model_source"]
        if not isinstance(source, str) or source not in {
            "confirmed", "legacy-unknown", "reconstructed-from-session", "unknown", "unavailable"
        }:
            reject("history model source is invalid")
        models = sample["models"]
        if models is None:
            if sample["models_complete"] or source not in {
                "reconstructed-from-session", "unknown", "unavailable"
            }:
                reject("nullable history models are not marked incomplete")
            continue
        if not isinstance(models, list):
            reject("history models is not an array or null")
        if source in {"reconstructed-from-session", "unknown", "unavailable"} and models is not None:
            reject("unqualified history source did not preserve nullable model data")
        if sample["models_complete"] and source != "confirmed":
            reject("complete history model set lacks confirmed source")
        for model in models:
            allowed_model_keys = {
                "model", "total_tokens", "input_tokens", "cached_input_tokens",
                "cache_write_input_tokens", "output_tokens", "total_dollars",
            }
            if (not isinstance(model, dict) or not {"model", "total_tokens"}.issubset(model)
                    or not set(model).issubset(allowed_model_keys)):
                reject("history model schema is invalid")
            if not isinstance(model["model"], str) or not model["model"] or not uint(model["total_tokens"]):
                reject("history model identity or total is invalid")
            for key in allowed_model_keys - {"model", "total_tokens"}:
                if key not in model:
                    continue
                value = model[key]
                if key == "total_dollars":
                    valid = type(value) in (int, float) and math.isfinite(value) and value >= 0
                else:
                    valid = uint(value)
                if not valid:
                    reject("history optional model component is invalid")
            if ("input_tokens" in model and "output_tokens" in model
                    and model["total_tokens"] != model["input_tokens"] + model["output_tokens"]):
                reject("history model token components are inconsistent")
            if ("cached_input_tokens" in model and "input_tokens" in model
                    and model["cached_input_tokens"] > model["input_tokens"]):
                reject("history cached tokens exceed input tokens")
            if ("cache_write_input_tokens" in model and "cached_input_tokens" in model
                    and "input_tokens" in model
                    and model["cached_input_tokens"] + model["cache_write_input_tokens"]
                    > model["input_tokens"]):
                reject("history cache components exceed input tokens")
            if model["model"] == "gpt-5.6-luna":
                max_luna_total = max(max_luna_total or 0, model["total_tokens"])
                if source == "confirmed" and model["total_tokens"] == expected_total:
                    confirmed_target = True
                if model["total_tokens"] > expected_total:
                    reject("history model total exceeded the owned append")

    with open(metadata_path, encoding="utf-8") as handle:
        metadata = json.load(handle)
    if (set(metadata) != {"view_key", "image", "complete"} or metadata["complete"] is not True
            or metadata["view_key"][0] != "/history"
            or metadata["view_key"][1].get("period") != page["period_id"]
            or not isinstance(metadata["image"], str) or not metadata["image"]):
        reject("history image is not qualified")
    if (not samples or not saw_quota or max_luna_total is None
            or max_luna_total < expected_total or not confirmed_target):
        raise SystemExit(75)
except (OSError, UnicodeError, ValueError, TypeError, KeyError, json.JSONDecodeError) as error:
    raise SystemExit(str(error))
PY
}

save_rest_failure() {
    local prefix="$1" label="$2" suffix
    for suffix in json snapshot.json meta.json reason; do
        [[ -f "$prefix.$suffix" ]] && cp -- "$prefix.$suffix" "$case_root/$label-failure.$suffix"
    done
    printf 'REST %s failed: %s\n' "$label" "$(cat "$prefix.reason" 2>/dev/null || true)" >&2
    [[ ! -s "$prefix.json" ]] || cat "$prefix.json" >&2
    return 0
}

fetch_current() {
    local path="$1" expected_reset="$2" expected_total="$3" phase="$4"
    local prefix="$case_root/current-$phase-last"
    if ! wire_snapshot current "$prefix" --min-total "$expected_total" >/dev/null 2>"$prefix.reason"; then
        save_rest_failure "$prefix" "current-$phase"
        return 1
    fi
    if ! assert_current_file "$prefix.json" "$prefix.meta.json" "$expected_reset" \
        "$expected_total" 2>"$prefix.reason"; then
        save_rest_failure "$prefix" "current-$phase"
        return 1
    fi
    cp -- "$prefix.json" "$path"
    cp -- "$prefix.meta.json" "$path.meta.json"
}

fetch_history() {
    local path="$1" expected_reset="$2" expected_total="$3" phase="$4"
    local prefix="$case_root/history-$phase-last"
    # This selector is the fixture-declared stored period, not a reset parsed
    # from a server-issued opaque ID. The returned logical keys qualify it.
    if ! wire_snapshot history "$prefix" --period "$expected_reset" \
        --min-total "$expected_total" --sample-source confirmed >/dev/null 2>"$prefix.reason"; then
        save_rest_failure "$prefix" "history-$phase"
        return 1
    fi
    if ! assert_history_file "$prefix.json" "$prefix.meta.json" "$expected_reset" \
        "$expected_total" 2>"$prefix.reason"; then
        save_rest_failure "$prefix" "history-$phase"
        return 1
    fi
    cp -- "$prefix.json" "$path"
    cp -- "$prefix.meta.json" "$path.meta.json"
}

read_projection_snapshot() {
    local expected_reset="$1" history_path="$2" expected_total="$3" history_timestamp
    is_uint "$expected_reset" && is_uint "$expected_total" || return 1
    history_timestamp="$(python3 - "$history_path" "$expected_reset" "$expected_total" <<'PY' || return 1
import json
import sys

history_path, reset_text, total_text = sys.argv[1:]
expected_reset = int(reset_text)
expected_total = int(total_text)
with open(history_path, encoding="utf-8") as handle:
    page = json.load(handle)
samples = page.get("samples") if isinstance(page, dict) else None
if not isinstance(samples, list):
    raise SystemExit("accepted REST history has no sample array")
timestamps = []
for sample in samples:
    if not isinstance(sample, dict):
        continue
    if sample.get("reset_at") != expected_reset or sample.get("model_source") != "confirmed":
        continue
    timestamp = sample.get("timestamp")
    if type(timestamp) is not int or timestamp <= 0:
        continue
    models = sample.get("models")
    if not isinstance(models, list):
        continue
    luna_entries = [
        entry for entry in models
        if isinstance(entry, dict) and entry.get("model") == "gpt-5.6-luna"
    ]
    if len(luna_entries) != 1:
        continue
    if luna_entries[0].get("total_tokens") != expected_total:
        continue
    timestamps.append(timestamp)
if not timestamps:
    raise SystemExit("accepted REST history has no exact confirmed fixture model observation")
print(max(timestamps))
PY
)" || return 1
    is_uint "$history_timestamp" && ((history_timestamp > 0)) || return 1
    read_sql "PRAGMA query_only=ON;
BEGIN;
SELECT 'quota|' || h.timestamp || '|' || h.reset_at || '|' || printf('%.17g', h.remaining_percent) || '|' || g.window_seconds
FROM usage_history AS h
JOIN collection_generation AS g ON g.singleton=1 AND g.reset_at=h.reset_at
WHERE h.reset_at=$expected_reset AND h.remaining_percent IS NOT NULL
ORDER BY h.timestamp DESC LIMIT 1;
SELECT 'session|' || model || '|' || total_tokens || '|' || input_tokens || '|' || cached_input_tokens || '|' || output_tokens || '|' || COALESCE(cache_write_input_tokens, '<NULL>')
FROM session_model_totals WHERE model='gpt-5.6-luna' LIMIT 1;
SELECT 'history|' || timestamp || '|' || reset_at || '|' || model || '|' || total_tokens || '|' || input_tokens || '|' || cached_input_tokens || '|' || output_tokens || '|' || COALESCE(cache_write_input_tokens, '<NULL>')
FROM usage_model_history
WHERE timestamp=$history_timestamp AND reset_at=$expected_reset AND model='gpt-5.6-luna';
COMMIT;"
}

assert_current_matches_sqlite() {
    local path="$1" snapshot="$2" expected_reset="$3"
    python3 - "$path" "$snapshot" "$expected_reset" <<'PY' || return 1
import json
import math
import sys

def fail(message):
    raise SystemExit(message)

rows = {}
for line in sys.argv[2].splitlines():
    fields = line.split("|")
    if fields[0] in rows:
        fail("SQLite snapshot duplicated " + fields[0])
    rows[fields[0]] = fields
if set(rows) != {"quota", "session", "history"}:
    fail("read-only SQLite transaction did not return quota and model rows")
quota_row, session_row = rows["quota"], rows["session"]
if len(quota_row) != 5 or len(session_row) != 7:
    fail("read-only SQLite quota/current model row is malformed")
quota_timestamp, quota_reset = int(quota_row[1]), int(quota_row[2])
quota_remaining, quota_window = float(quota_row[3]), int(quota_row[4])
if (quota_timestamp <= 0 or quota_reset != int(sys.argv[3])
        or not math.isfinite(quota_remaining) or abs(quota_remaining - 44.0) >= 1e-9
        or quota_window != 604800):
    fail("SQLite quota observation differs from the fixed fixture period")
if session_row[1] != "gpt-5.6-luna":
    fail("SQLite current model identity is malformed")
session_values = tuple(int(value) for value in session_row[2:6])
session_cache_write = None if session_row[6] == "<NULL>" else int(session_row[6])
with open(sys.argv[1], encoding="utf-8") as handle:
    current = json.load(handle)
quota = current["quota"]
if (quota["reset_at"] != quota_reset or quota["window_seconds"] != quota_window
        or abs(float(quota["remaining_percent"]) - quota_remaining) >= 1e-9):
    fail("REST Main quota differs from the same SQLite read transaction")
if len(current["models"]) != 1:
    fail("REST Main model row is missing")
model = current["models"][0]
current_values = (
    model["total_tokens"], model["input_tokens"],
    model["cached_input_tokens"], model["output_tokens"],
)
if current_values != session_values or model["cache_write_input_tokens"] != session_cache_write:
    fail("REST Main token components differ from session_model_totals")
PY
}

assert_history_matches_sqlite() {
    local path="$1" snapshot="$2" expected_reset="$3"
    python3 - "$path" "$snapshot" "$expected_reset" <<'PY' || return 1
import json
import sys

def fail(message):
    raise SystemExit(message)

rows = {}
for line in sys.argv[2].splitlines():
    fields = line.split("|")
    if fields[0] in rows:
        fail("SQLite snapshot duplicated " + fields[0])
    rows[fields[0]] = fields
history = rows.get("history")
if history is None or len(history) != 9:
    fail("same-reset SQLite model observation is missing or malformed")
timestamp, reset_at, model_name = int(history[1]), int(history[2]), history[3]
if reset_at != int(sys.argv[3]) or model_name != "gpt-5.6-luna":
    fail("SQLite history observation belongs to a different model or reset")
values = {
    "total_tokens": int(history[4]),
    "input_tokens": int(history[5]),
    "cached_input_tokens": int(history[6]),
    "output_tokens": int(history[7]),
}
cache_write = None if history[8] == "<NULL>" else int(history[8])
with open(sys.argv[1], encoding="utf-8") as handle:
    page = json.load(handle)
matched = []
for sample in page["samples"]:
    if sample["timestamp"] != timestamp or sample["reset_at"] != reset_at:
        continue
    for entry in sample["models"] or []:
        if entry["model"] == model_name and sample["model_source"] == "confirmed":
            matched.append(entry)
if len(matched) != 1:
    fail("REST history does not expose the same confirmed SQLite observation once")
entry = matched[0]
for key, value in values.items():
    if entry.get(key) != value:
        fail("REST history model component differs from same-reset SQLite observation")
if cache_write is None:
    if "cache_write_input_tokens" in entry:
        fail("REST history fabricated an unknown cache-write component")
elif entry.get("cache_write_input_tokens") != cache_write:
    fail("REST history cache-write component differs from SQLite")
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
wait_for_rest || fail 'REST did not become ready over the recorder SQLite database'
assert_runtime "$(runtime_body)" || fail 'REST runtime contract failed'
current_before="$case_root/current-before.json"
history_before="$case_root/history-before.json"
fetch_current "$current_before" "$fixture_reset_at" 120 before \
    || fail 'REST did not publish the exact Main projection after recorder append'
fetch_history "$history_before" "$fixture_reset_at" 120 before \
    || fail 'REST did not publish the exact history projection after recorder append'
projection_before="$(read_projection_snapshot "$fixture_reset_at" "$history_before" 120 2>/dev/null || true)"
[[ -n "$projection_before" ]] \
    || fail 'read-only SQLite projection snapshot is unavailable'
assert_current_matches_sqlite "$current_before" "$projection_before" "$fixture_reset_at" \
    || fail 'REST Main differs from the same SQLite quota/model transaction'
assert_history_matches_sqlite "$history_before" "$projection_before" "$fixture_reset_at" \
    || fail 'REST history differs from the same-reset SQLite model observation'

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
current_after="$case_root/current-after.json"
history_after="$case_root/history-after.json"
fetch_current "$current_after" "$fixture_reset_at" 240 after \
    || fail 'REST restart did not recover the exact Main model generation'
fetch_history "$history_after" "$fixture_reset_at" 240 after \
    || fail 'REST restart did not recover the exact history model generation'
projection_after="$(read_projection_snapshot "$fixture_reset_at" "$history_after" 240 2>/dev/null || true)"
[[ -n "$projection_after" ]] \
    || fail 'read-only SQLite projection snapshot after restart is unavailable'
assert_current_matches_sqlite "$current_after" "$projection_after" "$fixture_reset_at" \
    || fail 'REST Main after restart differs from the same SQLite transaction'
assert_history_matches_sqlite "$history_after" "$projection_after" "$fixture_reset_at" \
    || fail 'REST history after restart differs from the same-reset SQLite observation'

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

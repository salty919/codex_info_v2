#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

# Finite public CLI and daemon lifecycle acceptance.  Every mutable path is
# isolated below one temporary profile; no GitHub workflow or installed
# service is touched.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
UI_BINARY="$ROOT_DIR/target/release/codex_info"
RECORDER_BINARY="$ROOT_DIR/target/release/codex_info_recorder"
REST_BINARY="$ROOT_DIR/target/release/codex_info_rest"
ROOT_VERSION="$(awk '
    /^\[package\]$/ { in_package=1; next }
    /^\[/ { in_package=0 }
    in_package && $1 == "version" && $2 == "=" { gsub(/"/, "", $3); print $3; exit }
' "$ROOT_DIR/Cargo.toml")"

fail() {
    echo "cli-contract-e2e: $*" >&2
    if [[ -n "${tmp_root:-}" && -s "$tmp_root/recorder.log" ]]; then
        echo "cli-contract-e2e: bounded service diagnostics follow" >&2
        tail -n 40 "$tmp_root/recorder.log" >&2
    fi
    if [[ -n "${tmp_root:-}" && -s "$tmp_root/rest.log" ]]; then
        tail -n 40 "$tmp_root/rest.log" >&2
    fi
    if [[ -n "${tmp_root:-}" && -s "$tmp_root/rest-restart.log" ]]; then
        tail -n 40 "$tmp_root/rest-restart.log" >&2
    fi
    exit 1
}

# A listening health endpoint is not a ready account snapshot. Use the split
# current resource for readiness so unrelated legacy history compatibility
# cannot hide a valid current quota/model snapshot.
wait_for_ready_current() {
    local port="$1" minimum_model_total="$2" current_body='' current_status=''
    local current_prefix="$tmp_root/current-last" current_reason="$tmp_root/current-last.reason"
    for _ in $(seq 1 40); do
        capture_http_response "$current_prefix" "http://127.0.0.1:$port/v4/current"
        current_status="$(cat "$current_prefix.status" 2>/dev/null || true)"
        current_body="$(cat "$current_prefix.body" 2>/dev/null || true)"
        if [[ "$current_status" != 200 ]]; then
            printf 'current HTTP status was %s\n' "$current_status" >"$current_reason"
        elif python3 - "$current_body" "$minimum_model_total" "$current_reason" <<'PY_CURRENT' >/dev/null 2>&1
import json
import math
import sys

reason_path = sys.argv[3]

def reject(reason):
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write(reason + "\n")
    raise ValueError(reason)

def uint(value):
    return type(value) is int and 0 <= value <= 18446744073709551615

def finite_nonnegative(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0

try:
    current = json.loads(sys.argv[1])
    expected = {
        "api_version", "state", "observed_at", "authenticated", "plan_label",
        "quota", "models", "active_thread_count", "open_session_thread_count",
        "models_status",
    }
    if not isinstance(current, dict) or set(current) != expected:
        reject("current schema is unknown")
    if current["api_version"] != "v4" or current["state"] != "ready" or current["authenticated"] is not True:
        reject("current is not ready/authenticated")
    if not uint(current["observed_at"]) or current["observed_at"] <= 0:
        reject("current observed_at is invalid")
    if current["plan_label"] is not None and not isinstance(current["plan_label"], str):
        reject("current plan_label is invalid")
    quota = current["quota"]
    if not isinstance(quota, dict) or set(quota) != {
        "remaining_percent", "reset_at", "window_seconds", "monthly"
    }:
        reject("current quota is unavailable or malformed")
    if (not finite_nonnegative(quota["remaining_percent"])
            or quota["remaining_percent"] > 100
            or not uint(quota["reset_at"]) or quota["reset_at"] <= 0
            or not uint(quota["window_seconds"]) or quota["window_seconds"] <= 0
            or type(quota["monthly"]) is not bool):
        reject("current quota values are invalid")
    if (not uint(current["active_thread_count"])
            or not uint(current["open_session_thread_count"])
            or current["active_thread_count"] > current["open_session_thread_count"]):
        reject("current thread counts are invalid")
    if current["models_status"] not in {"complete", "partial", "unavailable"}:
        reject("current models_status is invalid")
    if not isinstance(current["models"], list):
        reject("current models is invalid")
    model_keys = {
        "model", "total_tokens", "input_tokens", "cached_input_tokens",
        "cache_write_input_tokens", "output_tokens", "estimated_cost",
    }
    estimated_cost_keys = {
        "price_version", "ordinary_input_dollars", "cached_input_dollars",
        "cache_write_input_dollars", "output_dollars", "total_dollars",
    }
    for model in current["models"]:
        if not isinstance(model, dict) or set(model) != model_keys:
            reject("current model schema is invalid")
        if not isinstance(model["model"], str) or not model["model"]:
            reject("current model identity is invalid")
        for key in ("total_tokens", "input_tokens", "cached_input_tokens", "output_tokens"):
            if not uint(model[key]):
                reject("current model token value is invalid")
        cache_write = model["cache_write_input_tokens"]
        if cache_write is not None and not uint(cache_write):
            reject("current cache-write value is invalid")
        if (model["total_tokens"] != model["input_tokens"] + model["output_tokens"]
                or model["cached_input_tokens"] > model["input_tokens"]
                or (cache_write is not None
                    and model["cached_input_tokens"] + cache_write > model["input_tokens"])):
            reject("current model token components are inconsistent")
        cost = model["estimated_cost"]
        if cost is not None:
            if (not isinstance(cost, dict) or set(cost) != estimated_cost_keys
                    or not isinstance(cost["price_version"], str)
                    or any(not finite_nonnegative(cost[key]) for key in estimated_cost_keys - {"price_version"})):
                reject("current estimated_cost is invalid")
    model_total = sum(model["total_tokens"] for model in current["models"])
    if model_total < int(sys.argv[2]):
        reject("current model total has not reached the owned append")
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write("validated v4 current observation\n")
except (ValueError, TypeError, KeyError, json.JSONDecodeError) as error:
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write("v4 current validation failed: " + str(error) + "\n")
    raise SystemExit(1)
PY_CURRENT
        then
            printf '%s\n' "$current_body"
            return 0
        fi
        sleep 0.1
    done
    echo "REST current did not reach model total $minimum_model_total (HTTP $current_status): $(cat "$current_reason" 2>/dev/null || true)" >&2
    return 1
}

capture_http_response() {
    local prefix="$1" url="$2" status=''
    : >"$prefix.headers"
    : >"$prefix.body"
    status="$(curl --silent --max-time 1 --dump-header "$prefix.headers" \
        --output "$prefix.body" --write-out '%{http_code}' "$url" 2>/dev/null || true)"
    printf '%s\n' "$status" >"$prefix.status"
}

wait_for_history_snapshot() {
    local port="$1" expected_reset="$2" phase="$3" expectation="$4"
    local expected_total="$5" period_id="" last_reason="$tmp_root/history-last.reason"
    local directory_prefix="$tmp_root/history-last-directory"
    local page_prefix="$tmp_root/history-last-page"
    local directory_status='' page_status='' directory_result='' pair=''
    local page_url='' output_body='' discover_period=0
    if (($# >= 6)); then
        period_id="$6"
    fi
    if [[ -z "$period_id" ]]; then
        discover_period=1
    fi

    for _ in $(seq 1 40); do
        directory_result=''
        pair=''
        if ((discover_period == 1)); then
            capture_http_response "$directory_prefix" \
                "http://127.0.0.1:$port/v5/history/periods"
            directory_status="$(cat "$directory_prefix.status" 2>/dev/null || true)"
            if [[ "$directory_status" != 200 ]]; then
                printf 'directory HTTP status was %s\n' "$directory_status" >"$last_reason"
                sleep 0.1
                continue
            fi
            directory_result="$(python3 - "$directory_prefix.body" "$directory_prefix.headers" \
                "$expected_reset" "$last_reason" 2>/dev/null <<'PY_HISTORY_DIRECTORY' || true
import json
import re
import sys

body_path, headers_path, reset_text, reason_path = sys.argv[1:]

def reject(reason):
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write(reason + "\n")
    raise SystemExit(1)

def uint(value):
    return type(value) is int and 0 <= value <= 18446744073709551615

try:
    with open(body_path, encoding="utf-8") as handle:
        directory = json.load(handle)
    if not isinstance(directory, dict) or set(directory) != {
        "api_version", "history_periods", "next_cursor"
    } or directory["api_version"] != "v5":
        reject("v5 history directory schema is invalid")
    if directory["next_cursor"] is not None:
        reject("v5 history directory unexpectedly has an unread page")
    periods = directory["history_periods"]
    if not isinstance(periods, list):
        reject("v5 history directory periods are invalid")
    expected_reset = int(reset_text)
    matching = []
    for period in periods:
        if not isinstance(period, dict) or set(period) != {
            "id", "start_at", "end_at", "reset_at", "label", "current", "origin_at"
        }:
            reject("v5 history period schema is invalid")
        if (not isinstance(period["id"], str) or not period["id"].isascii()
                or not period["id"].isdigit() or period["id"].startswith("0")
                or not uint(period["start_at"]) or not uint(period["end_at"])
                or not uint(period["reset_at"]) or not isinstance(period["label"], str)
                or type(period["current"]) is not bool
                or (period["origin_at"] is not None and not uint(period["origin_at"]))):
            reject("v5 history period values are invalid")
        if period["reset_at"] == expected_reset:
            matching.append(period)
    if (len(matching) != 1 or matching[0]["id"] != str(expected_reset)
            or matching[0]["current"] is not True):
        reject("v5 directory did not contain one current period for the exact quota reset")

    with open(headers_path, "rb") as handle:
        raw_headers = handle.read().splitlines()
    pairs = []
    for raw in raw_headers:
        name, separator, value = raw.partition(b":")
        if separator and name.strip().lower() == b"codex-info-published-pair":
            pairs.append(value.strip().decode("ascii", "strict"))
    if len(pairs) != 1 or not re.fullmatch(r"v1:[0-9a-f]{64}", pairs[0]):
        reject("v5 directory published-pair header is missing or malformed")
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write("waiting for an exact v5 history period\n")
    print(matching[0]["id"] + "\t" + pairs[0])
except (OSError, UnicodeError, ValueError, TypeError, KeyError, json.JSONDecodeError) as error:
    reject("v5 history directory validation failed: " + str(error))
PY_HISTORY_DIRECTORY
)"
            if [[ -z "$directory_result" ]]; then
                sleep 0.1
                continue
            fi
            IFS=$'\t' read -r period_id pair <<<"$directory_result"
        fi

        page_url="http://127.0.0.1:$port/v3/history?period=$period_id"
        capture_http_response "$page_prefix" "$page_url"
        page_status="$(cat "$page_prefix.status" 2>/dev/null || true)"
        if [[ "$page_status" != 200 ]]; then
            printf 'history page HTTP status was %s\n' "$page_status" >"$last_reason"
            sleep 0.1
            continue
        fi
        if python3 - "$page_prefix.body" "$page_prefix.headers" "$expected_reset" \
            "$expectation" "$expected_total" "$pair" "$last_reason" \
            <<'PY_HISTORY_PAGE' >/dev/null 2>&1
import json
import math
import re
import sys

body_path, headers_path, reset_text, expectation, total_text, expected_pair, reason_path = sys.argv[1:]

def reject(reason):
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write(reason + "\n")
    raise SystemExit(1)

def uint(value):
    return type(value) is int and 0 <= value <= 18446744073709551615

def finite_percent(value):
    return type(value) in (int, float) and math.isfinite(value) and 0 <= value <= 100

try:
    with open(body_path, encoding="utf-8") as handle:
        page = json.load(handle)
    if not isinstance(page, dict) or set(page) != {
        "api_version", "history_samples", "history_gaps", "next_cursor", "resume_cursor"
    } or page["api_version"] != "v3":
        reject("v3 history page schema is invalid")
    if page["next_cursor"] is not None:
        reject("v3 history page unexpectedly has an unread page")
    samples = page["history_samples"]
    if not isinstance(samples, list) or not isinstance(page["history_gaps"], list):
        reject("v3 history collections are invalid")
    if samples and (not isinstance(page["resume_cursor"], str) or not page["resume_cursor"]):
        reject("v3 history resume cursor is missing for a non-empty page")
    if not samples and page["resume_cursor"] is not None:
        reject("empty v3 history page has a resume cursor")
    expected_reset = int(reset_text)
    expected_total = int(total_text) if total_text else None
    saw_quota = False
    saw_unavailable = False
    saw_expected_model = False
    for sample in samples:
        sample_keys = {
            "timestamp", "reset_at", "remaining_percent", "task_active_since_previous",
            "models", "models_complete", "model_source",
        }
        if not isinstance(sample, dict) or set(sample) != sample_keys:
            reject("v3 history sample schema is invalid")
        if (not uint(sample["timestamp"]) or not uint(sample["reset_at"])
                or sample["reset_at"] != expected_reset):
            reject("v3 history sample does not belong to the exact requested reset")
        remaining = sample["remaining_percent"]
        if remaining is not None and not finite_percent(remaining):
            reject("v3 history quota value is invalid")
        if remaining is not None:
            saw_quota = True
        if (sample["task_active_since_previous"] is not None
                and type(sample["task_active_since_previous"]) is not bool):
            reject("v3 history task state is invalid")
        if type(sample["models_complete"]) is not bool or not isinstance(sample["model_source"], str):
            reject("v3 history model metadata is invalid")
        models = sample["models"]
        if models is None:
            if sample["models_complete"] or sample["model_source"] not in {
                "reconstructed-from-session", "unknown", "unavailable"
            }:
                reject("v3 nullable history model source is invalid")
            if (remaining is not None and sample["model_source"] == "unavailable"
                    and sample["models_complete"] is False):
                saw_unavailable = True
            continue
        if not isinstance(models, list):
            reject("v3 history models are invalid")
        for model in models:
            if not isinstance(model, dict) or not {"model", "total_tokens"}.issubset(model):
                reject("v3 history model schema is invalid")
            if (not isinstance(model["model"], str) or not model["model"]
                    or not uint(model["total_tokens"])):
                reject("v3 history model value is invalid")
            if (expectation == "confirmed"
                    and sample["model_source"] == "confirmed"
                    and model["model"] == "gpt-5.6-luna"
                    and model["total_tokens"] == expected_total):
                saw_expected_model = True

    if not samples:
        reject("v3 history page has no retained observation for the requested reset")
    if expectation == "unavailable" and (not saw_quota or not saw_unavailable):
        reject("pre-append history did not retain quota with nullable unavailable model data")
    if expectation == "confirmed" and not saw_expected_model:
        reject("v3 history did not expose the exact confirmed gpt-5.6-luna total")

    with open(headers_path, "rb") as handle:
        raw_headers = handle.read().splitlines()
    pairs = []
    for raw in raw_headers:
        name, separator, value = raw.partition(b":")
        if separator and name.strip().lower() == b"codex-info-published-pair":
            pairs.append(value.strip().decode("ascii", "strict"))
    if len(pairs) != 1 or not re.fullmatch(r"v1:[0-9a-f]{64}", pairs[0]):
        reject("v3 history published-pair header is missing or malformed")
    if expected_pair and pairs[0] != expected_pair:
        reject("v5 directory and v3 history published pairs changed during acquisition")
    with open(reason_path, "w", encoding="utf-8") as handle:
        handle.write("validated v3 history observation\n")
except (OSError, UnicodeError, ValueError, TypeError, KeyError, json.JSONDecodeError) as error:
    reject("v3 history validation failed: " + str(error))
PY_HISTORY_PAGE
        then
            cp -- "$page_prefix.body" "$tmp_root/history-$phase.json"
            cp -- "$page_prefix.headers" "$tmp_root/history-$phase.headers"
            if ((discover_period == 1)); then
                printf '%s\n' "$period_id" >"$tmp_root/history-period-id.txt"
            fi
            output_body="$(cat "$page_prefix.body")"
            printf '%s\n' "$output_body"
            return 0
        fi
        sleep 0.1
    done
    echo "REST v3 history did not satisfy $expectation for reset $expected_reset: $(cat "$last_reason" 2>/dev/null || true)" >&2
    return 1
}

assert_v1_history_unavailable() {
    local port="$1" response='' status='' body='' prefix="$tmp_root/v1-last"
    capture_http_response "$prefix" "http://127.0.0.1:$port/v1/details"
    response="$(cat "$prefix.body"; printf '\n%s' "$(cat "$prefix.status")")"
    status="${response##*$'\n'}"
    body="${response%$'\n'*}"
    if [[ "$status" != '503' ]]; then
        printf 'v1 details returned HTTP %s; expected 503\n' "$status" >"$prefix.reason"
        fail "v1 details should reject unrepresentable history with 503 (got $status)"
    fi
    if python3 - "$body" <<'PY_V1_UNAVAILABLE' >/dev/null 2>&1
import json
import sys

body = json.loads(sys.argv[1])
if body != {"api_version": "v1", "error": "history_unavailable"}:
    raise SystemExit("v1 history error wire changed")
PY_V1_UNAVAILABLE
    then
        printf 'validated v1 history_unavailable response\n' >"$prefix.reason"
    else
        printf 'v1 details body did not match history_unavailable\n' >"$prefix.reason"
        fail 'v1 details returned an unexpected error body'
    fi
}

[[ "$ROOT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || fail 'root Cargo.toml package version is unavailable'

for command in curl python3 rg sha256sum sqlite3 ss stat; do
    command -v "$command" >/dev/null || fail "$command is required"
done
if [[ ! -x "$UI_BINARY" || ! -x "$RECORDER_BINARY" || ! -x "$REST_BINARY" ]]; then
    command -v cargo >/dev/null || fail 'cargo is required to build missing release binaries'
    (cd -- "$ROOT_DIR" && cargo build --release --locked \
        -p codex_info -p codex-info-recorder -p codex-info-rest)
fi
for binary in "$UI_BINARY" "$RECORDER_BINARY" "$REST_BINARY"; do
    [[ -f "$binary" && -x "$binary" && ! -L "$binary" ]] \
        || fail "release binary is not an executable regular file: $binary"
done

temp_parent="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
temp_parent="$(cd -- "$temp_parent" && pwd -P)"
tmp_root="$(mktemp -d "$temp_parent/codex-info-cli-e2e.XXXXXX")"
recorder_pid=""
rest_pid=""
ui_pid=""
sentinel_pid=""

cleanup() {
    terminate_owned "$ui_pid" "$UI_BINARY" UI >/dev/null 2>&1 || true
    terminate_owned "$rest_pid" "$REST_BINARY" REST >/dev/null 2>&1 || true
    terminate_owned "$recorder_pid" "$RECORDER_BINARY" recorder >/dev/null 2>&1 || true
    if [[ -n "$sentinel_pid" ]] && kill -0 "$sentinel_pid" 2>/dev/null; then
        kill -TERM "$sentinel_pid" 2>/dev/null || true
        wait "$sentinel_pid" 2>/dev/null || true
    fi
    case "$tmp_root" in
        "$temp_parent"/codex-info-cli-e2e.*) rm -rf -- "$tmp_root" ;;
        *) fail "refusing to clean unexpected path: $tmp_root" ;;
    esac
}
trap cleanup EXIT

port=$((41000 + (BASHPID % 10000)))
while ss -ltnH "sport = :$port" 2>/dev/null | rg -q '^LISTEN'; do
    port=$((port + 1))
    ((port <= 60000)) || fail "no unused test port"
done

terminate_owned() {
    local pid="$1" expected_exe="$2" label="$3"
    [[ -n "$pid" ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid" 2>/dev/null || true
        return 0
    fi
    [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$expected_exe" ]] || {
        echo "cli-contract-e2e: refusing to terminate unowned $label PID $pid" >&2
        return 1
    }
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 80); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.1
    done
    if kill -0 "$pid" 2>/dev/null; then
        [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$expected_exe" ]] || return 1
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
    ! kill -0 "$pid" 2>/dev/null
}

process_matches() {
    local pid="$1" expected_exe="$2"
    [[ "$pid" =~ ^[0-9]+$ && -e "/proc/$pid" ]] || return 1
    [[ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" == "$expected_exe" ]] || return 1
    tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null \
        | rg -Fqx -- "CODEX_INFO_DATA_DIR=$data_root"
}

wait_for_process_matches() {
    local pid="$1" expected_exe="$2" label="$3"
    for _ in $(seq 1 20); do
        if process_matches "$pid" "$expected_exe"; then
            return 0
        fi
        sleep 0.1
    done
    fail "$label did not reach its exact executable/environment identity"
}

profile_root="$tmp_root/profile"
data_root="$profile_root/data"
codex_root="$profile_root/codex"
runtime_root="$profile_root/runtime"
mkdir -p "$profile_root/home" "$profile_root/config" "$profile_root/cache" \
    "$profile_root/state" "$runtime_root" "$codex_root/sessions/2026/08/27" \
    "$data_root/history"
chmod 700 "$runtime_root" "$codex_root"
auth_file="$codex_root/auth.json"
printf '%s\n' '{"auth_mode":"chatgpt","tokens":{"account_id":"fixture-account-129"}}' \
    >"$auth_file"
chmod 600 "$auth_file"
sqlite3 -batch -bail "$codex_root/state_5.sqlite" 'VACUUM;'
chmod 600 "$codex_root/state_5.sqlite"

common_env=(
    "HOME=$profile_root/home"
    "XDG_CONFIG_HOME=$profile_root/config"
    "XDG_DATA_HOME=$profile_root/xdg-data"
    "XDG_CACHE_HOME=$profile_root/cache"
    "XDG_STATE_HOME=$profile_root/state"
    "XDG_RUNTIME_DIR=$runtime_root"
    "CODEX_HOME=$codex_root"
    "CODEX_INFO_DATA_DIR=$data_root"
    "CODEX_INFO_CODEX_BIN=$ROOT_DIR/scripts/fake_codex_app_server.py"
    "CODEX_INFO_DAEMON_INTERVAL_SECS=1"
)

sessions_root="$codex_root/sessions"
session_file="$sessions_root/2026/08/27/cli-contract.jsonl"
common_env+=(
    "CODEX_INFO_FAKE_THREAD_ID=cli-contract-thread"
    "CODEX_INFO_FAKE_THREAD_PATH=$session_file"
    "CODEX_INFO_FAKE_THREAD_TITLE=CLI contract fixture"
)
reset_hint="$data_root/history/usage_reset_hint.json"
fixture_now="$(date -u +%s)"
fixture_event_epoch=$((fixture_now - 600))
fixture_reset_at=$((fixture_now + 3600))
fixture_event_time="$(date -u -d "@$fixture_event_epoch" '+%Y-%m-%dT%H:%M:%SZ')"
common_env+=("CODEX_INFO_FAKE_RESET_AT=$fixture_reset_at")
printf '%s\n' \
    "{\"timestamp\":\"$fixture_event_time\",\"type\":\"session_meta\",\"payload\":{\"id\":\"cli-contract-thread\",\"session_id\":\"session-cli-contract-thread\"}}" \
    "{\"timestamp\":\"$fixture_event_time\",\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}" \
    "{\"timestamp\":\"$fixture_event_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":10,\"input_tokens\":8,\"cached_input_tokens\":4,\"output_tokens\":2}}}}" \
    >"$session_file"
chmod 600 "$session_file"
printf '{"reset_at":%s,"window_seconds":604800}\n' "$fixture_reset_at" >"$reset_hint"

# The sentinel is deliberately outside the product scope. It must survive
# every product termination below, proving that PID cleanup is not a broad
# process-group or name-based kill.
tail -f /dev/null &
sentinel_pid="$!"
kill -0 "$sentinel_pid" 2>/dev/null || fail 'scope sentinel did not start'

# Help aliases are successful, localized product output and have no startup
# side effects.  The Japanese and fallback-English catalogs are both executed.
for alias in --help --h -h; do
    help_output="$(env "${common_env[@]}" LC_ALL=C "$UI_BINARY" "$alias")"
    rg -q --fixed-strings -- '--ui --port PORT' <<<"$help_output" \
        || fail "$alias omitted --ui --port"
    if rg -q --fixed-strings -- '--stop' <<<"$help_output"; then
        fail "$alias exposed launcher-only --stop"
    fi
done
ja_help="$(env "${common_env[@]}" LC_ALL=ja_JP.UTF-8 "$UI_BINARY" --help)"
rg -q --fixed-strings '使用法:' <<<"$ja_help" || fail 'Japanese help was not selected'
[[ ! -e "$data_root/history/usage_record_daemon.lock" ]] \
    || fail 'help created a daemon lock'

# The installed launcher selects its own catalog through the verified payload;
# it must not expose the raw service/development-only --port operation.
launcher_home="$tmp_root/launcher-home"
launcher_path="$tmp_root/run.sh"
mkdir -p "$launcher_home/.local/bin" \
    "$launcher_home/.local/share/codex-info/current"
cp -- "$UI_BINARY" "$launcher_home/.local/share/codex-info/current/codex_info"
ln -s -- '../share/codex-info/current/codex_info' \
    "$launcher_home/.local/bin/codex_info"
cp -- "$ROOT_DIR/run.sh" "$launcher_path"
chmod 0755 "$launcher_path"
launcher_help="$(HOME="$launcher_home" LC_ALL=C "$launcher_path" --help)"
for option in --start --ui --stop --disable-autostart --remove --status --update --help; do
    rg -q --fixed-strings -- "$option" <<<"$launcher_help" \
        || fail "installed launcher help omitted $option"
done
if rg -q --fixed-strings -- '--port' <<<"$launcher_help"; then
    fail 'installed launcher help exposed payload-only --port'
fi
launcher_ja_help="$(HOME="$launcher_home" LC_ALL=ja_JP.UTF-8 "$launcher_path" --help)"
rg -q --fixed-strings '使用法:' <<<"$launcher_ja_help" \
    || fail 'installed launcher Japanese help was not selected'
[[ ! -e "$launcher_home/.local/share/codex-info/control-state.json" ]] \
    || fail 'installed launcher help mutated control state'

# Every rejected form must fail before creating its own profile data root.
reject_root="$tmp_root/rejected-data"
run_rejected() {
    if env "${common_env[@]}" "CODEX_INFO_DATA_DIR=$reject_root" "$UI_BINARY" "$@" \
        >"$tmp_root/rejected.out" 2>"$tmp_root/rejected.err"; then
        fail "rejected argv succeeded: $*"
    fi
    [[ ! -e "$reject_root" ]] || fail "rejected argv created data: $*"
}
for legacy in --service --ui-only --all --listen --record-daemon --once --ui-onlry --unknown; do
    run_rejected "$legacy"
done
run_rejected --port
run_rejected --stop
run_rejected --ui --port
run_rejected --port "$port" --ui
run_rejected --ui --ui
run_rejected --stop --port "$port"
run_rejected --help --ui
run_rejected "--port=$port"
for invalid_port in 0 65536 -1 abc '127.0.0.1:8787'; do
    run_rejected --port "$invalid_port"
done

# The recorder owns fresh account-partition allocation. It is started before
# REST so the locator can select the same initialized database without any
# combined-service bootstrap or manual SQLite writes.
env "${common_env[@]}" "$RECORDER_BINARY" \
    --sessions-root "$sessions_root" --interval-secs 1 \
    >"$tmp_root/recorder.log" 2>&1 &
recorder_pid="$!"
wait_for_process_matches "$recorder_pid" "$RECORDER_BINARY" recorder
lock_path="$data_root/history/usage_record_daemon.lock"
database=''
for _ in $(seq 1 200); do
    databases=("$data_root"/history/accounts/v1/*/epoch-*/usage_history.sqlite3)
    if [[ "${#databases[@]}" -eq 1 ]]; then
        checkpoint_count="$(sqlite3 -batch -bail -cmd '.timeout 2000' \
            "${databases[0]}" 'SELECT COUNT(*) FROM session_checkpoints;' 2>/dev/null || true)"
        if [[ "$checkpoint_count" =~ ^[0-9]+$ ]] && ((10#$checkpoint_count >= 1)); then
            database="${databases[0]}"
            break
        fi
    fi
    sleep 0.1
done
[[ -n "$database" ]] || fail 'recorder did not allocate an account Session partition'
[[ -f "$lock_path" ]] || fail 'recorder lock was not created'

env "${common_env[@]}" "$REST_BINARY" --port "$port" \
    >"$tmp_root/rest.log" 2>&1 &
rest_pid="$!"
wait_for_process_matches "$rest_pid" "$REST_BINARY" 'initial REST'
for _ in $(seq 1 80); do
    if curl --fail --silent --max-time 1 \
        "http://127.0.0.1:$port/v1/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health" >/dev/null \
    || fail 'REST health did not become ready'
health_body="$(curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health")"
current_body="$(wait_for_ready_current "$port" 0)" \
    || fail 'initial REST current did not expose a ready authenticated quota'
python3 - "$health_body" "$current_body" "$ROOT_VERSION" <<'PY'
import json
import sys

health = json.loads(sys.argv[1])
if set(health) != {"api_version", "service", "product_version"}:
    raise SystemExit("health wire key set changed")
if (health["api_version"] != "v1" or health["service"] != "codex-info"
        or health["product_version"] != sys.argv[3]):
    raise SystemExit("health wire identity changed")
current = json.loads(sys.argv[2])
expected = {
    "api_version", "state", "observed_at", "authenticated", "plan_label", "quota",
    "models", "active_thread_count", "open_session_thread_count", "models_status",
}
if set(current) != expected or current["api_version"] != "v4":
    raise SystemExit("current wire key set or identity changed")
if current["models_status"] != "unavailable" or current["models"] != []:
    raise SystemExit("unowned pre-boundary Session counters became current model usage")
PY
ss -ltnH "sport = :$port" | rg -q "127[.]0[.]0[.]1:$port" \
    || fail 'listener is not bound to 127.0.0.1'

# Health proves lock/listener readiness, not completion of the account-bound
# Session baseline. Wait for the one physical account DB and its checkpoint,
# then prove that pre-boundary bytes produced no usage before appending a
# verified range.
database=''
for _ in $(seq 1 200); do
    databases=("$data_root"/history/accounts/v1/*/epoch-*/usage_history.sqlite3)
    if [[ "${#databases[@]}" -eq 1 ]]; then
        checkpoint_count="$(sqlite3 -batch -bail -cmd '.timeout 2000' \
            "${databases[0]}" 'SELECT COUNT(*) FROM session_checkpoints;' 2>/dev/null || true)"
        if [[ "$checkpoint_count" =~ ^[0-9]+$ ]] && ((10#$checkpoint_count >= 1)); then
            database="${databases[0]}"
            break
        fi
    fi
    sleep 0.1
done
[[ -n "$database" ]] || fail 'account Session baseline did not complete'
state_path="$data_root/history/recorder-state.json"
[[ -f "$state_path" ]] || fail 'owner recorder-state.json was not created'
[[ "$(stat -c '%a' "$state_path")" == 600 ]] \
    || fail 'recorder-state.json is not owner-private'
python3 - "$state_path" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
expected = {
    "schema", "pid", "process_starttime", "owner_nonce", "write_state",
    "partition_id_hash", "data_generation", "collector_epoch", "cycle_seq",
    "last_commit_unix", "updated_at_unix", "recorder_version",
}
if set(state) != expected or state["schema"] != "codex-info-recorder-state-v1":
    raise SystemExit("recorder-state schema/key set changed")
if state["write_state"] != "degraded" or state["data_generation"] <= 0:
    raise SystemExit("recorder-state did not retain pending Session evidence as degraded")
PY
[[ "$(sqlite3 "$database" 'SELECT COUNT(*) FROM storage_partition;')" == 1 ]] \
    || fail 'account database partition authority is missing'
[[ "$(sqlite3 "$database" 'SELECT COUNT(*) FROM usage_history WHERE sol_tokens <> 0 OR terra_tokens <> 0 OR luna_tokens <> 0 OR ABS(sol_dollars) > 0.0000001 OR ABS(terra_dollars) > 0.0000001 OR ABS(luna_dollars) > 0.0000001;')" == 0 ]] \
    || fail 'pre-boundary Session bytes were attributed'
python3 - "$database" "$session_file" <<'PY'
import hashlib
import json
import sqlite3
import sys

database, session_path = sys.argv[1:]
def range_fingerprint(raw):
    return hashlib.sha256(hashlib.sha256(raw).digest()).hexdigest()

with open(session_path, "rb") as handle:
    content = handle.read()
lines = []
offset = 0
for line in content.splitlines(keepends=True):
    event = json.loads(line)
    usage = event.get("payload", {}).get("info", {}).get("total_token_usage", {})
    if usage.get("total_tokens") == 10:
        lines.append((offset, offset + len(line), range_fingerprint(line)))
    offset += len(line)
if len(lines) != 1:
    raise SystemExit("initial raw counter fixture is not unique")
pending_start, pending_end, pending_hash = lines[0]
prefix_hash = range_fingerprint(content)
with sqlite3.connect(database) as connection:
    checkpoints = connection.execute(
        "SELECT relative_path, committed_offset, prefix_sha256, prefix_generation, "
        "previous_total, previous_input, previous_cached_input, previous_output "
        "FROM session_checkpoints"
    ).fetchall()
    ranges = connection.execute(
        "SELECT start_offset, end_offset, record_sha256, prefix_generation "
        "FROM session_ranges WHERE relative_path=?",
        ("2026/08/27/cli-contract.jsonl",),
    ).fetchall()
    pending = connection.execute(
        "SELECT start_offset, end_offset, record_sha256, reason, complete "
        "FROM session_pending_ranges WHERE relative_path=? "
        "AND reason='counter-outside-account-ownership'",
        ("2026/08/27/cli-contract.jsonl",),
    ).fetchall()
    events = connection.execute("SELECT COUNT(*) FROM session_events").fetchone()[0]
    models = connection.execute("SELECT COUNT(*) FROM session_model_totals").fetchone()[0]
if len(checkpoints) != 1:
    raise SystemExit("initial fixture did not create exactly one Session checkpoint")
checkpoint = checkpoints[0]
if checkpoint[:3] != ("2026/08/27/cli-contract.jsonl", len(content), prefix_hash):
    raise SystemExit("Session checkpoint does not match the complete fixture prefix")
if checkpoint[4:] != ("10", "8", "4", "2"):
    raise SystemExit("raw cumulative counter baseline was not retained exactly")
if len(ranges) != 1 or ranges[0] != (0, len(content), prefix_hash, checkpoint[3]):
    raise SystemExit("accepted source range does not match the exact parsed prefix")
if len(pending) != 1 or pending[0] != (
    pending_start, pending_end, pending_hash, "counter-outside-account-ownership", 1
):
    raise SystemExit("outside-ownership counter evidence is missing or not byte-exact")
if events != 0 or models != 0:
    raise SystemExit("outside-ownership counter was attributed before an owned delta")
PY
history_reset_at="$(python3 - "$current_body" <<'PY_CURRENT_RESET'
import json
import sys

current = json.loads(sys.argv[1])
print(current["quota"]["reset_at"])
PY_CURRENT_RESET
)" || fail 'initial current quota reset could not be read'
wait_for_history_snapshot "$port" "$history_reset_at" before-append unavailable 0 '' \
    >/dev/null || fail 'pre-append v3 history did not retain the nullable unavailable observation'
history_period_id="$(cat "$tmp_root/history-period-id.txt" 2>/dev/null || true)"
[[ "$history_period_id" == "$history_reset_at" ]] \
    || fail 'v5 directory period id did not match the exact current quota reset'
assert_v1_history_unavailable "$port"

pre_append_ranges="$(sqlite3 "$database" 'SELECT COUNT(*) FROM session_ranges;')"
[[ ! -e "$data_root/history/usage_history.sqlite3" ]] \
    || fail 'legacy unpartitioned history database was created'

append_time="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
printf '%s\n' \
    "{\"timestamp\":\"$append_time\",\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}" \
    "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":20,\"input_tokens\":16,\"cached_input_tokens\":8,\"output_tokens\":4}}}}" \
    "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":30,\"input_tokens\":24,\"cached_input_tokens\":12,\"output_tokens\":6}}}}" \
    >>"$session_file"

post_boundary_model_total=0
for _ in $(seq 1 200); do
    post_boundary_model_total="$(sqlite3 -batch -bail -cmd '.timeout 2000' \
        "$database" 'SELECT COALESCE(SUM(CAST(total_tokens AS INTEGER)),0) FROM session_model_totals;' 2>/dev/null || true)"
    if [[ "$post_boundary_model_total" =~ ^[0-9]+$ ]] \
        && ((10#$post_boundary_model_total == 10)); then
        break
    fi
    sleep 0.1
done
if [[ ! "$post_boundary_model_total" =~ ^[0-9]+$ ]] \
    || ((10#$post_boundary_model_total != 10)); then
    fail "post-baseline recorder commit did not preserve exactly the owned 20-to-30 delta (observed total=$post_boundary_model_total)"
fi
python3 - "$database" "$session_file" <<'PY'
import hashlib
import json
import sqlite3
import sys

database, session_path = sys.argv[1:]
def range_fingerprint(raw):
    return hashlib.sha256(hashlib.sha256(raw).digest()).hexdigest()

with open(session_path, "rb") as handle:
    content = handle.read()
expected = {}
offset = 0
for line in content.splitlines(keepends=True):
    event = json.loads(line)
    usage = event.get("payload", {}).get("info", {}).get("total_token_usage", {})
    if usage.get("total_tokens") in (10, 20):
        total = usage["total_tokens"]
        expected[total] = (offset, offset + len(line), range_fingerprint(line))
    offset += len(line)
if set(expected) != {10, 20}:
    raise SystemExit("raw baseline and owned-boundary counters are not unique")
with sqlite3.connect(database) as connection:
    events = connection.execute(
        "SELECT model, total_tokens, input_tokens, cached_input_tokens, output_tokens "
        "FROM session_events ORDER BY timestamp, event_index"
    ).fetchall()
    models = connection.execute(
        "SELECT model, total_tokens, input_tokens, cached_input_tokens, output_tokens "
        "FROM session_model_totals"
    ).fetchall()
    pending = connection.execute(
        "SELECT start_offset, end_offset, record_sha256, reason, complete "
        "FROM session_pending_ranges WHERE relative_path=?",
        ("2026/08/27/cli-contract.jsonl",),
    ).fetchall()
expected_events = [("gpt-5.6-luna", "10", "8", "4", "2")]
if events != expected_events or models != expected_events:
    raise SystemExit("only the owned 20-to-30 model delta may be attributed")
expected_pending = {
    (*expected[10], "counter-outside-account-ownership", 1),
    (*expected[20], "account-counter-prefix-unknown", 1),
}
if set(pending) != expected_pending:
    raise SystemExit("exact outside-ownership and first-owned baseline evidence was not retained")
PY
post_boundary_ranges="$(sqlite3 "$database" 'SELECT COUNT(*) FROM session_ranges;')"
[[ "$post_boundary_ranges" =~ ^[0-9]+$ ]] \
    && ((10#$post_boundary_ranges > 10#$pre_append_ranges)) \
    || fail 'post-boundary Session append did not increase committed ranges'
rg -q --fixed-strings 'codex-info-recorder generation=' "$tmp_root/recorder.log" \
    || fail 'recorder did not report a committed usage sample'

[[ -f "$database" ]] || fail 'history database was not created'

collection_state() {
    sqlite3 -batch -bail -cmd '.timeout 2000' "$database" \
        'SELECT data_generation || "|" || COALESCE(collector_epoch, "") || "|" || cycle_seq FROM collection_generation WHERE singleton = 1'
}
recorder_state() {
    python3 - "$state_path" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
print(f'{state["data_generation"]}|{state["collector_epoch"]}|{state["cycle_seq"]}')
PY
}
range_count() {
    sqlite3 -batch -bail -cmd '.timeout 2000' "$database" \
        'SELECT COUNT(*) FROM session_ranges'
}
checkpoint_offset() {
    sqlite3 -batch -bail -cmd '.timeout 2000' "$database" \
        'SELECT COALESCE(MAX(committed_offset), 0) FROM session_checkpoints'
}
model_total() {
    sqlite3 -batch -bail -cmd '.timeout 2000' "$database" \
        'SELECT COALESCE(SUM(CAST(total_tokens AS INTEGER)), 0) FROM session_model_totals'
}
valid_collection_state() {
    local generation="$1" epoch="$2" cycle="$3"
    [[ "$generation" =~ ^[0-9]+$ && "$cycle" =~ ^[0-9]+$ ]] \
        && ((10#$generation > 0 && 10#$cycle > 0)) \
        && [[ "$epoch" =~ ^[0-9a-f]{32}$ ]]
}

baseline_state="$(collection_state 2>/dev/null || true)"
IFS='|' read -r baseline_generation baseline_epoch baseline_cycle <<<"$baseline_state"
valid_collection_state "$baseline_generation" "$baseline_epoch" "$baseline_cycle" \
    || fail 'recorder did not publish a valid collection_generation acknowledgement'
for _ in $(seq 1 20); do
    rg -q --fixed-strings 'codex-info-recorder acknowledged generation=' \
        "$tmp_root/recorder.log" && break
    sleep 0.1
done
rg -q --fixed-strings 'codex-info-recorder acknowledged generation=' \
    "$tmp_root/recorder.log" \
    || fail 'recorder did not publish a final cycle acknowledgement'
for _ in $(seq 1 20); do
    db_ack="$(collection_state 2>/dev/null || true)"
    file_ack="$(recorder_state 2>/dev/null || true)"
    [[ -n "$db_ack" && "$file_ack" == "$db_ack" ]] && break
    sleep 0.1
done
[[ "$file_ack" == "$db_ack" ]] \
    || fail 'recorder-state does not match the final SQLite generation'
baseline_ranges="$(range_count)"
baseline_checkpoint="$(checkpoint_offset)"
baseline_models="$(model_total)"
[[ "$baseline_ranges" =~ ^[0-9]+$ && "$baseline_checkpoint" =~ ^[0-9]+$ \
    && "$baseline_models" =~ ^[0-9]+$ ]] \
    || fail 'recorder acknowledgement readback is not numeric'

current_before_file="$tmp_root/current-before.json"
current_before="$(wait_for_ready_current "$port" "$post_boundary_model_total")" \
    || fail 'REST current did not expose the owned 20-to-30 model delta'
printf '%s\n' "$current_before" >"$current_before_file"
wait_for_history_snapshot "$port" "$history_reset_at" after-append confirmed 10 \
    "$history_period_id" >/dev/null \
    || fail 'REST v3 history did not expose the exact confirmed model total 10'

# Current quota/model values and nullable history are independent API
# resources. Compare each to the recorder's committed SQLite facts without
# mixing a legacy all-in-one projection into current readiness.
python3 - "$current_before" "$database" <<'PY'
import json
import sqlite3
import sys

current = json.loads(sys.argv[1])
models = current["models"]
if not models or not any(model["total_tokens"] > 0 for model in models):
    raise SystemExit("REST current models are empty after the owned append")
current_model_total = sum(model["total_tokens"] for model in models)
with sqlite3.connect(sys.argv[2]) as connection:
    quota_rows = connection.execute(
        "SELECT COUNT(*) FROM usage_history WHERE remaining_percent IS NOT NULL"
    ).fetchone()[0]
    model_rows = connection.execute(
        "SELECT COALESCE(SUM(CAST(total_tokens AS INTEGER)), 0) FROM session_model_totals"
    ).fetchone()[0]
if quota_rows <= 0 or model_rows <= 0 or current_model_total != model_rows:
    raise SystemExit("REST current quota/models disagree with recorder-owned SQLite values")
PY

source_before="$(sha256sum "$session_file" | awk '{print $1}')"
hint_before="$(sha256sum "$reset_hint" | awk '{print $1}')"
recorder_pid_before="$recorder_pid"
rest_pid_before="$rest_pid"

# Stop only REST. The recorder must retain its exact PID and acknowledge a
# later Session append while the REST listener is absent.
terminate_owned "$rest_pid" "$REST_BINARY" REST || fail 'REST scoped shutdown failed'
rest_pid=""
[[ ! -e "/proc/$rest_pid_before" ]] || fail 'REST process remained after scoped shutdown'
if ss -ltnH "sport = :$port" 2>/dev/null | rg -q '^LISTEN'; then
    fail 'REST listener remained after scoped shutdown'
fi
[[ "$recorder_pid" == "$recorder_pid_before" ]] \
    || fail 'recorder PID changed when REST stopped'
process_matches "$recorder_pid" "$RECORDER_BINARY" \
    || fail 'recorder stopped or changed executable during REST outage'

append_time="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
printf '%s\n' \
    "{\"timestamp\":\"$append_time\",\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}" \
    "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":40,\"input_tokens\":32,\"cached_input_tokens\":16,\"output_tokens\":8}}}}" \
    >>"$session_file"

advanced=0
for _ in $(seq 1 200); do
    current_state="$(collection_state 2>/dev/null || true)"
    IFS='|' read -r current_generation current_epoch current_cycle <<<"$current_state"
    current_ranges="$(range_count 2>/dev/null || true)"
    current_checkpoint="$(checkpoint_offset 2>/dev/null || true)"
    current_models="$(model_total 2>/dev/null || true)"
    if valid_collection_state "$current_generation" "$current_epoch" "$current_cycle" \
        && [[ "$current_ranges" =~ ^[0-9]+$ && "$current_checkpoint" =~ ^[0-9]+$ \
            && "$current_models" =~ ^[0-9]+$ ]] \
        && ((10#$current_generation > 10#$baseline_generation \
            && 10#$current_ranges > 10#$baseline_ranges \
            && 10#$current_checkpoint > 10#$baseline_checkpoint \
            && 10#$current_models > 10#$baseline_models)); then
        advanced=1
        break
    fi
    sleep 0.1
done
((advanced == 1)) \
    || fail 'recorder did not advance generation/range/checkpoint/model during REST outage'
[[ "$current_models" =~ ^[0-9]+$ ]] && ((10#$current_models == 20)) \
    || fail "recorder did not retain the exact expected total 20 during REST outage (observed total=$current_models)"
[[ "$recorder_pid" == "$recorder_pid_before" ]] \
    || fail 'recorder PID changed after Session append during REST outage'
process_matches "$recorder_pid" "$RECORDER_BINARY" \
    || fail 'recorder identity was not retained during REST outage'

# Restart REST without passing a database path: its locator must select the
# same initialized account partition and expose the newly acknowledged data.
env "${common_env[@]}" "$REST_BINARY" --port "$port" \
    >"$tmp_root/rest-restart.log" 2>&1 &
rest_pid="$!"
wait_for_process_matches "$rest_pid" "$REST_BINARY" 'restarted REST'
[[ "$rest_pid" != "$recorder_pid" && "$rest_pid" != "$rest_pid_before" ]] \
    || fail 'REST restart did not create a distinct process'
for _ in $(seq 1 80); do
    if curl --fail --silent --max-time 1 \
        "http://127.0.0.1:$port/v1/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
restart_health_body="$(curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health")" \
    || fail 'REST did not recover after restart'
python3 - "$restart_health_body" "$ROOT_VERSION" <<'PY'
import json
import sys

health = json.loads(sys.argv[1])
if set(health) != {"api_version", "service", "product_version"}:
    raise SystemExit("restart health wire key set changed")
if (health["api_version"], health["service"], health["product_version"]) != (
    "v1", "codex-info", sys.argv[2]
):
    raise SystemExit("restart health wire identity changed")
PY
current_after="$(wait_for_ready_current "$port" "$current_models")" \
    || fail 'restarted REST current did not expose the committed model generation'
wait_for_history_snapshot "$port" "$history_reset_at" after-restart confirmed 20 \
    "$history_period_id" >/dev/null \
    || fail 'restarted REST v3 history did not expose the exact confirmed model total 20'
python3 - "$current_before_file" "$current_after" "$database" <<'PY'
import json
import sqlite3
import sys

current_before = json.load(open(sys.argv[1], encoding="utf-8"))
current_after = json.loads(sys.argv[2])
before_models = {item["model"]: item["total_tokens"] for item in current_before["models"]}
after_models = {item["model"]: item["total_tokens"] for item in current_after["models"]}
if not any(name in before_models and total > before_models[name]
           for name, total in after_models.items()):
    raise SystemExit("REST restart current did not expose the later model generation")

with sqlite3.connect(sys.argv[3]) as connection:
    if connection.execute("SELECT COUNT(*) FROM session_ranges").fetchone()[0] <= 0:
        raise SystemExit("same SQLite database has no committed range")
    model_rows = connection.execute(
        "SELECT COALESCE(SUM(CAST(total_tokens AS INTEGER)), 0) FROM session_model_totals"
    ).fetchone()[0]
if sum(after_models.values()) != model_rows:
    raise SystemExit("REST restart current differs from the same committed SQLite model totals")
PY

[[ "$(sha256sum "$session_file" | awk '{print $1}')" != "$source_before" ]] \
    || fail 'Session append did not change the source fixture'
[[ "$(sha256sum "$reset_hint" | awk '{print $1}')" == "$hint_before" ]] \
    || fail 'recorder changed the reset hint source'

# Stop exactly the two daemon processes and prove the unrelated sentinel is
# still alive; cleanup then terminates only that test-owned sentinel.
terminate_owned "$rest_pid" "$REST_BINARY" REST || fail 'REST final shutdown failed'
rest_pid=""
terminate_owned "$recorder_pid" "$RECORDER_BINARY" recorder \
    || fail 'recorder final shutdown failed'
recorder_pid=""
kill -0 "$sentinel_pid" 2>/dev/null \
    || fail 'scoped daemon termination killed the unrelated sentinel'
sentinel_exe="$(readlink "/proc/$sentinel_pid/exe" 2>/dev/null || true)"
[[ "$sentinel_exe" != "$RECORDER_BINARY" && "$sentinel_exe" != "$REST_BINARY" \
    && "$sentinel_exe" != "$UI_BINARY" ]] \
    || fail 'scope sentinel unexpectedly has a product executable'
kill -TERM "$sentinel_pid" 2>/dev/null || true
wait "$sentinel_pid" 2>/dev/null || true
sentinel_pid=""

printf 'cli-contract-e2e: PASS (help aliases/i18n, finite rejection, split recorder/rest PIDs, REST-outage generation, locator restart, scoped cleanup)\n'

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
    exit 1
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
reset_hint="$data_root/history/usage_reset_hint.json"
fixture_now="$(date -u +%s)"
fixture_event_epoch=$((fixture_now - 60))
fixture_reset_at=$((fixture_now + 3600))
fixture_event_time="$(date -u -d "@$fixture_event_epoch" '+%Y-%m-%dT%H:%M:%SZ')"
common_env+=("CODEX_INFO_FAKE_RESET_AT=$fixture_reset_at")
printf '%s\n' \
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
details_body=''
for _ in $(seq 1 40); do
    details_body="$(curl --fail --silent --max-time 1 \
        "http://127.0.0.1:$port/v1/details" 2>/dev/null || true)"
    if python3 - "$details_body" <<'PY' >/dev/null 2>&1
import json
import sys

details = json.loads(sys.argv[1])
raise SystemExit(0 if details.get("state") == "ready" and details.get("authenticated") is True else 1)
PY
    then
        break
    fi
    sleep 0.1
done
python3 - "$health_body" "$details_body" "$ROOT_VERSION" <<'PY'
import json
import sys

health = json.loads(sys.argv[1])
if set(health) != {"api_version", "service", "product_version"}:
    raise SystemExit("health wire key set changed")
if (health["api_version"] != "v1" or health["service"] != "codex-info"
        or health["product_version"] != sys.argv[3]):
    raise SystemExit("health wire identity changed")
details = json.loads(sys.argv[2])
expected = {
    "api_version", "state", "observed_at", "authenticated", "plan_label", "quota",
    "models", "active_thread_count", "history_periods", "history_samples",
    "history_gaps", "threads", "estimated_cost_label",
}
if set(details) != expected:
    raise SystemExit("details wire key set changed")
if details["api_version"] != "v1":
    raise SystemExit("details wire identity changed")
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
    "last_commit_unix", "updated_at_unix",
}
if set(state) != expected or state["schema"] != "codex-info-recorder-state-v1":
    raise SystemExit("recorder-state schema/key set changed")
if state["write_state"] != "ready" or state["data_generation"] <= 0:
    raise SystemExit("recorder-state is not an acknowledged ready state")
PY
[[ "$(sqlite3 "$database" 'SELECT COUNT(*) FROM storage_partition;')" == 1 ]] \
    || fail 'account database partition authority is missing'
[[ "$(sqlite3 "$database" 'SELECT COUNT(*) FROM usage_history WHERE sol_tokens <> 0 OR terra_tokens <> 0 OR luna_tokens <> 0 OR ABS(sol_dollars) > 0.0000001 OR ABS(terra_dollars) > 0.0000001 OR ABS(luna_dollars) > 0.0000001;')" == 0 ]] \
    || fail 'pre-boundary Session bytes were attributed'
pre_append_ranges="$(sqlite3 "$database" 'SELECT COUNT(*) FROM session_ranges;')"
[[ "$pre_append_ranges" == 0 ]] \
    || fail 'new account EOF baseline unexpectedly accepted a pre-boundary range'
[[ ! -e "$data_root/history/usage_history.sqlite3" ]] \
    || fail 'legacy unpartitioned history database was created'

append_time="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
printf '%s\n' \
    "{\"timestamp\":\"$append_time\",\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}" \
    "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":20,\"input_tokens\":16,\"cached_input_tokens\":8,\"output_tokens\":4}}}}" \
    "{\"timestamp\":\"$append_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":30,\"input_tokens\":24,\"cached_input_tokens\":12,\"output_tokens\":6}}}}" \
    >>"$session_file"

post_boundary_luna_tokens=0
for _ in $(seq 1 200); do
    post_boundary_luna_tokens="$(sqlite3 -batch -bail -cmd '.timeout 2000' \
        "$database" 'SELECT COALESCE(MAX(luna_tokens),0) FROM usage_history;' 2>/dev/null || true)"
    if [[ "$post_boundary_luna_tokens" =~ ^[0-9]+$ ]] \
        && ((10#$post_boundary_luna_tokens >= 10)); then
        break
    fi
    sleep 0.1
done
if [[ ! "$post_boundary_luna_tokens" =~ ^[0-9]+$ ]] \
    || ((10#$post_boundary_luna_tokens < 10)); then
    fail "post-baseline recorder commit did not include the verified append (observed luna_tokens=$post_boundary_luna_tokens)"
fi
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

details_before="$tmp_root/details-before.json"
details_body=''
for _ in $(seq 1 40); do
    details_body="$(curl --fail --silent --max-time 1 \
        "http://127.0.0.1:$port/v1/details" 2>/dev/null || true)"
    if python3 - "$details_body" <<'PY' >/dev/null 2>&1
import json
import sys

details = json.loads(sys.argv[1])
raise SystemExit(0 if details.get("state") == "ready" and details.get("authenticated") is True else 1)
PY
    then
        break
    fi
    sleep 0.1
done
printf '%s\n' "$details_body" >"$details_before"
details_state="$(python3 - "$details_body" <<'PY'
import json
import sys

details = json.loads(sys.argv[1])
print(f'{details.get("state")}|{details.get("authenticated")}')
PY
)"
[[ "$details_state" == 'ready|True' ]] \
    || fail "REST details remained non-ready ($details_state)"

# REST details must be backed by recorder-produced UsageStore values. Empty
# quota/history/models are a failure, never a successful fixture shortcut.
python3 - "$details_body" "$database" <<'PY'
import json
import sqlite3
import sys

details = json.loads(sys.argv[1])
if details["state"] != "ready" or details["authenticated"] is not True:
    raise SystemExit("REST details are not ready/authenticated")
quota = details["quota"]
if not isinstance(quota, dict) or quota["reset_at"] <= 0:
    raise SystemExit("REST details quota is empty")
history = details["history_samples"]
if not history or not any(sample.get("remaining_percent") is not None for sample in history):
    raise SystemExit("REST details history is empty or has no quota sample")
models = details["models"]
if not models or not any(
    model.get("name") and int(model.get("input_tokens", 0)) + int(model.get("output_tokens", 0)) > 0
    for model in models
):
    raise SystemExit("REST details models are empty")
with sqlite3.connect(sys.argv[2]) as connection:
    quota_rows = connection.execute(
        "SELECT COUNT(*) FROM usage_history WHERE remaining_percent IS NOT NULL"
    ).fetchone()[0]
    model_rows = connection.execute(
        "SELECT COALESCE(SUM(CAST(total_tokens AS INTEGER)), 0) FROM session_model_totals"
    ).fetchone()[0]
if quota_rows <= 0 or model_rows <= 0:
    raise SystemExit("SQLite recorder projection is empty")
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
details_after="$(curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/details")"
python3 - "$details_before" "$details_after" "$database" <<'PY'
import json
import sqlite3
import sys

before = json.load(open(sys.argv[1], encoding="utf-8"))
after = json.loads(sys.argv[2])
if after["state"] != "ready" or after["authenticated"] is not True:
    raise SystemExit("REST restart details are not ready/authenticated")
if not after["history_samples"] or not after["models"]:
    raise SystemExit("REST restart details lost history/models")
old_models = {
    item["name"]: int(item.get("input_tokens", 0)) + int(item.get("output_tokens", 0))
    for item in before["models"]
}
new_models = {
    item["name"]: int(item.get("input_tokens", 0)) + int(item.get("output_tokens", 0))
    for item in after["models"]
}
if not any(name in old_models and total > old_models[name] for name, total in new_models.items()):
    raise SystemExit("REST restart did not expose model generation")
with sqlite3.connect(sys.argv[3]) as connection:
    if connection.execute("SELECT COUNT(*) FROM session_ranges").fetchone()[0] <= 0:
        raise SystemExit("same SQLite database has no committed range")
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

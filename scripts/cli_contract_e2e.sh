#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

# Finite public CLI and daemon lifecycle acceptance.  Every mutable path is
# isolated below one temporary profile; no GitHub workflow or installed
# service is touched.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BINARY="$ROOT_DIR/target/release/codex_info"

fail() {
    echo "cli-contract-e2e: $*" >&2
    if [[ -n "${tmp_root:-}" && -s "$tmp_root/service.log" ]]; then
        echo "cli-contract-e2e: bounded service diagnostics follow" >&2
        tail -n 40 "$tmp_root/service.log" >&2
    fi
    exit 1
}

for command in curl python3 rg sha256sum sqlite3 ss stat; do
    command -v "$command" >/dev/null || fail "$command is required"
done
[[ -x "$BINARY" ]] || fail "build target/release/codex_info first"

temp_parent="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
temp_parent="$(cd -- "$temp_parent" && pwd -P)"
tmp_root="$(mktemp -d "$temp_parent/codex-info-cli-e2e.XXXXXX")"
service_pid=""
sentinel_pid=""

cleanup() {
    if [[ -n "$service_pid" ]] && kill -0 "$service_pid" 2>/dev/null; then
        kill -TERM "$service_pid" 2>/dev/null || true
        wait "$service_pid" 2>/dev/null || true
    fi
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

session_file="$codex_root/sessions/2026/08/27/cli-contract.jsonl"
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

# Help aliases are successful, localized product output and have no startup
# side effects.  The Japanese and fallback-English catalogs are both executed.
for alias in --help --h -h; do
    help_output="$(env "${common_env[@]}" LC_ALL=C "$BINARY" "$alias")"
    rg -q --fixed-strings -- '--ui --port PORT' <<<"$help_output" \
        || fail "$alias omitted --ui --port"
    rg -q --fixed-strings -- '--stop' <<<"$help_output" \
        || fail "$alias omitted --stop"
done
ja_help="$(env "${common_env[@]}" LC_ALL=ja_JP.UTF-8 "$BINARY" --help)"
rg -q --fixed-strings '使用法:' <<<"$ja_help" || fail 'Japanese help was not selected'
[[ ! -e "$data_root/history/usage_record_daemon.lock" ]] \
    || fail 'help created a daemon lock'

# The installed launcher selects its own catalog through the verified payload;
# it must not expose the raw service/development-only --port operation.
launcher_home="$tmp_root/launcher-home"
launcher_path="$tmp_root/run.sh"
mkdir -p "$launcher_home/.local/bin" \
    "$launcher_home/.local/share/codex-info/current"
cp -- "$BINARY" "$launcher_home/.local/share/codex-info/current/codex_info"
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
    if env "${common_env[@]}" "CODEX_INFO_DATA_DIR=$reject_root" "$BINARY" "$@" \
        >"$tmp_root/rejected.out" 2>"$tmp_root/rejected.err"; then
        fail "rejected argv succeeded: $*"
    fi
    [[ ! -e "$reject_root" ]] || fail "rejected argv created data: $*"
}
for legacy in --service --ui-only --all --listen --record-daemon --once --ui-onlry --unknown; do
    run_rejected "$legacy"
done
run_rejected --port
run_rejected --ui --port
run_rejected --port "$port" --ui
run_rejected --ui --ui
run_rejected --stop --port "$port"
run_rejected --help --ui
run_rejected "--port=$port"
for invalid_port in 0 65536 -1 abc '127.0.0.1:8787'; do
    run_rejected --port "$invalid_port"
done

# Start one real daemon+REST owner on the requested loopback port.  A hostile
# legacy environment value must not alter the explicit public CLI contract.
env "${common_env[@]}" CODEX_INFO_API_LISTEN=0.0.0.0:1 \
    "$BINARY" --port "$port" >"$tmp_root/service.log" 2>&1 &
service_pid="$!"
lock_path="$data_root/history/usage_record_daemon.lock"
for _ in $(seq 1 80); do
    if [[ -f "$lock_path" ]] && curl --fail --silent --max-time 1 \
        "http://127.0.0.1:$port/v1/health" >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done
[[ -f "$lock_path" ]] || fail 'service lock was not created'
curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health" >/dev/null \
    || fail 'service health did not become ready'
health_body="$(curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health")"
details_body="$(curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/details")"
python3 - "$health_body" "$details_body" <<'PY'
import json
import sys

health = json.loads(sys.argv[1])
if set(health) != {"api_version", "service", "product_version"}:
    raise SystemExit("health wire key set changed")
if health["api_version"] != "v1" or health["service"] != "codex-info":
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
[[ "$(sqlite3 "$database" 'SELECT COUNT(*) FROM session_ranges;')" == 0 ]] \
    || fail 'pre-boundary Session bytes produced a committed range'
[[ ! -e "$data_root/history/usage_history.sqlite3" ]] \
    || fail 'legacy unpartitioned history database was created'

initial_commit='codex-info: recorder committed 1 samples'
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
[[ "$(sqlite3 "$database" 'SELECT COUNT(*) FROM session_ranges;')" -ge 1 ]] \
    || fail 'post-boundary Session append did not produce a committed range'
rg -q --fixed-strings "$initial_commit" "$tmp_root/service.log" \
    || fail 'recorder did not report a committed usage sample'

[[ -f "$database" ]] || fail 'history database was not created'
logical_database_sha256() {
    sqlite3 -batch -bail -cmd '.timeout 2000' "$1" .dump \
        | sha256sum \
        | awk '{print $1}'
}
logical_database_without_gap_ledger_sha256() {
    # A controlled resident stop records a pending gap in the ledger. Compare
    # all other SQLite schema/data, then assert the new ledger row separately.
    sqlite3 -batch -bail -cmd '.timeout 2000' "$1" .dump \
        | rg -v 'recorder_gap_ledger' \
        | sha256sum \
        | awk '{print $1}'
}
db_before="$(logical_database_without_gap_ledger_sha256 "$database")"
source_before="$(sha256sum "$session_file" | awk '{print $1}')"
hint_before="$(sha256sum "$reset_hint" | awk '{print $1}')"

# Public stop owns the complete sequence: verified owner -> one TERM -> lock
# release. It is idempotent, preserves durable/source data, and starts one
# pending production gap for the interval that has not yet been source-proven.
env "${common_env[@]}" "$BINARY" --stop
wait "$service_pid"
service_pid=""
[[ ! -e "$lock_path" ]] || fail '--stop returned before lock release'
if curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health" >/dev/null 2>&1; then
    fail '--stop left the REST listener healthy'
fi
[[ "$(logical_database_without_gap_ledger_sha256 "$database")" == "$db_before" ]] \
    || fail '--stop changed non-ledger logical database content'
pending_gap_count="$(sqlite3 -batch -bail -cmd '.timeout 2000' "$database" \
    "SELECT COUNT(*) FROM recorder_gap_ledger WHERE state='pending' AND reason='daemon_stop_unrecoverable';")"
if [[ ! "$pending_gap_count" =~ ^[0-9]+$ ]] || ((10#$pending_gap_count < 1)); then
    fail '--stop did not record a pending production gap'
fi
[[ "$(sha256sum "$session_file" | awk '{print $1}')" == "$source_before" ]] \
    || fail '--stop changed source JSONL'
[[ "$(sha256sum "$reset_hint" | awk '{print $1}')" == "$hint_before" ]] \
    || fail '--stop changed reset hint'
env "${common_env[@]}" "$BINARY" --stop \
    || fail 'stopping an already-stopped profile was not successful'

# A present but unverifiable lock fails closed. Use a child owned by this test,
# not the shell running the gate, so both accidental TERM delivery and the
# child's eventual controlled exit are observable without risking the gate.
invalid_root="$tmp_root/invalid-lock-data"
mkdir -p "$invalid_root/history"
invalid_lock="$invalid_root/history/usage_record_daemon.lock"
sentinel_term="$tmp_root/sentinel.term"
sentinel_ready="$tmp_root/sentinel.ready"
(
    trap 'printf "term\n" >"$sentinel_term"; exit 0' TERM
    printf 'ready\n' >"$sentinel_ready"
    while :; do
        sleep 0.05
    done
) &
sentinel_pid="$!"
for _ in $(seq 1 40); do
    [[ -f "$sentinel_ready" ]] && break
    sleep 0.025
done
[[ -f "$sentinel_ready" ]] || fail 'sentinel child did not become ready'
printf '{"pid":%s}\n' "$sentinel_pid" >"$invalid_lock"
invalid_before="$(sha256sum "$invalid_lock" | awk '{print $1}')"
if env "${common_env[@]}" "CODEX_INFO_DATA_DIR=$invalid_root" "$BINARY" --stop \
    >"$tmp_root/invalid-stop.out" 2>"$tmp_root/invalid-stop.err"; then
    fail 'unverifiable lock was accepted by --stop'
fi
kill -0 "$sentinel_pid" 2>/dev/null || fail 'unverifiable lock terminated the sentinel child'
[[ ! -e "$sentinel_term" ]] || fail 'unverifiable lock sent TERM to the sentinel child'
[[ "$(sha256sum "$invalid_lock" | awk '{print $1}')" == "$invalid_before" ]] \
    || fail 'unverifiable lock was modified'

# Prove the sentinel itself reports both sides of its TERM contract before the
# trap exits it; this keeps the no-signal assertion above independent of the
# shell's own process state.
kill -TERM "$sentinel_pid"
for _ in $(seq 1 40); do
    [[ -f "$sentinel_term" ]] && break
    sleep 0.025
done
[[ -f "$sentinel_term" ]] || fail 'sentinel TERM handler was not observed'
wait "$sentinel_pid"
sentinel_pid=""

printf 'cli-contract-e2e: PASS (help aliases/i18n, finite rejection, loopback port, verified stop, idempotence, data preservation, invalid-lock fail-closed)\n'

#!/usr/bin/env bash
set -euo pipefail

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

for command in curl rg sha256sum sqlite3 ss; do
    command -v "$command" >/dev/null || fail "$command is required"
done
[[ -x "$BINARY" ]] || fail "build target/release/codex_info first"

temp_parent="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
temp_parent="$(cd -- "$temp_parent" && pwd -P)"
tmp_root="$(mktemp -d "$temp_parent/codex-info-cli-e2e.XXXXXX")"
service_pid=""
sentinel_pid=""
lock_holder_pid=""

cleanup() {
    if [[ -n "$service_pid" ]] && kill -0 "$service_pid" 2>/dev/null; then
        kill -TERM "$service_pid" 2>/dev/null || true
        wait "$service_pid" 2>/dev/null || true
    fi
    if [[ -n "$sentinel_pid" ]] && kill -0 "$sentinel_pid" 2>/dev/null; then
        kill -TERM "$sentinel_pid" 2>/dev/null || true
        wait "$sentinel_pid" 2>/dev/null || true
    fi
    if [[ -n "$lock_holder_pid" ]] && kill -0 "$lock_holder_pid" 2>/dev/null; then
        kill -TERM "$lock_holder_pid" 2>/dev/null || true
        wait "$lock_holder_pid" 2>/dev/null || true
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
chmod 700 "$runtime_root"

common_env=(
    "HOME=$profile_root/home"
    "XDG_CONFIG_HOME=$profile_root/config"
    "XDG_DATA_HOME=$profile_root/xdg-data"
    "XDG_CACHE_HOME=$profile_root/cache"
    "XDG_STATE_HOME=$profile_root/state"
    "XDG_RUNTIME_DIR=$runtime_root"
    "CODEX_HOME=$codex_root"
    "CODEX_INFO_DATA_DIR=$data_root"
    # Exercise the production retry path without making a transient initial
    # SQLite/input race wait for the 60-second default interval.
    "CODEX_INFO_DAEMON_INTERVAL_SECS=5"
)

session_file="$codex_root/sessions/2026/08/27/cli-contract.jsonl"
reset_hint="$data_root/history/usage_reset_hint.json"
fixture_now="$(date -u +%s)"
fixture_event_epoch=$((fixture_now - 60))
fixture_reset_at=$((fixture_now + 3600))
fixture_event_time="$(date -u -d "@$fixture_event_epoch" '+%Y-%m-%dT%H:%M:%SZ')"
printf '%s\n' \
    "{\"timestamp\":\"$fixture_event_time\",\"type\":\"turn_context\",\"model\":\"gpt-5.6-luna\"}" \
    "{\"timestamp\":\"$fixture_event_time\",\"type\":\"token_count\",\"payload\":{\"info\":{\"total_token_usage\":{\"total_tokens\":10,\"input_tokens\":8,\"cached_input_tokens\":4,\"output_tokens\":2}}}}" \
    >"$session_file"
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

# Hold a fresh database under an exclusive transaction long enough to exceed
# UsageStore's two-second busy timeout. The first recorder cycle must fail
# safely, and the same resident worker must recover on its bounded retry.
database="$data_root/history/usage_history.sqlite3"
lock_ready="$tmp_root/sqlite-lock.ready"
[[ "$lock_ready" != *"'"* ]] || fail 'temporary path cannot be shell-quoted safely'
sqlite3 "$database" <<SQL >"$tmp_root/sqlite-lock.log" 2>&1 &
BEGIN EXCLUSIVE;
.shell touch '$lock_ready'
.shell sleep 3
COMMIT;
SQL
lock_holder_pid="$!"
for _ in $(seq 1 40); do
    [[ -f "$lock_ready" ]] && break
    sleep 0.025
done
[[ -f "$lock_ready" ]] || fail 'SQLite contention fixture did not acquire its lock'

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
ss -ltnH "sport = :$port" | rg -q "127[.]0[.]0[.]1:$port" \
    || fail 'listener is not bound to 127.0.0.1'

# Health proves lock/listener readiness, not completion of the recorder's
# initial backfill. Wait for this fixture's one deterministic commit, including
# the bounded production retry path after a transient input/SQLite conflict,
# before reading SQLite or exercising the successful stop path. Racing the
# writer would test the documented fail-closed timeout path instead.
initial_commit='codex-info: recorder committed 1 samples'
for _ in $(seq 1 300); do
    rg -q --fixed-strings "$initial_commit" "$tmp_root/service.log" && break
    sleep 0.1
done
rg -q --fixed-strings "$initial_commit" "$tmp_root/service.log" \
    || fail 'initial recorder backfill did not complete'
rg -q --fixed-strings 'codex-info: recorder skipped an unsafe input cycle' \
    "$tmp_root/service.log" || fail 'forced transient recorder failure was not observed'
wait "$lock_holder_pid"
lock_holder_pid=""

# Health alone accepted the old installed daemon while the Windows client
# correctly rejected status/details without an atomic generation identity.
# Exercise the actual public wire boundary and require one matching canonical
# pair header on both documents.
status_headers="$tmp_root/status.headers"
details_headers="$tmp_root/details.headers"
curl --fail --silent --max-time 3 -D "$status_headers" -o "$tmp_root/status.json" \
    "http://127.0.0.1:$port/v1/status" \
    || fail 'status document was not available'
curl --fail --silent --max-time 3 -D "$details_headers" -o "$tmp_root/details.json" \
    "http://127.0.0.1:$port/v1/details" \
    || fail 'details document was not available'
read_pair_header() {
    local headers="$1" values
    values="$(awk 'BEGIN { IGNORECASE=1 } /^Codex-Info-Published-Pair:[[:space:]]*/ {
        sub(/^[^:]*:[[:space:]]*/, ""); sub(/\r$/, ""); print
    }' "$headers")"
    [[ "$(wc -l <<<"$values")" -eq 1 ]] \
        || fail "published pair header must appear exactly once: $headers"
    rg -q '^v1:[0-9a-f]{64}$' <<<"$values" \
        || fail "published pair header is not canonical: $headers"
    printf '%s' "$values"
}
status_pair="$(read_pair_header "$status_headers")"
details_pair="$(read_pair_header "$details_headers")"
[[ "$status_pair" == "$details_pair" ]] \
    || fail 'status/details published pair headers differ'
jq -e '.api_version == "v1"' "$tmp_root/status.json" >/dev/null \
    || fail 'status body is not REST v1'
jq -e '.api_version == "v1"' "$tmp_root/details.json" >/dev/null \
    || fail 'details body is not REST v1'

database="$data_root/history/usage_history.sqlite3"
[[ -f "$database" ]] || fail 'history database was not created'
logical_database_sha256() {
    sqlite3 -batch -bail -cmd '.timeout 2000' "$1" .dump \
        | sha256sum \
        | awk '{print $1}'
}
db_before="$(logical_database_sha256 "$database")"
source_before="$(sha256sum "$session_file" | awk '{print $1}')"
hint_before="$(sha256sum "$reset_hint" | awk '{print $1}')"

# Public stop owns the complete sequence: verified owner -> one TERM -> lock
# release.  It is idempotent and preserves all durable/source data.
env "${common_env[@]}" "$BINARY" --stop
wait "$service_pid"
service_pid=""
[[ ! -e "$lock_path" ]] || fail '--stop returned before lock release'
if curl --fail --silent --max-time 1 "http://127.0.0.1:$port/v1/health" >/dev/null 2>&1; then
    fail '--stop left the REST listener healthy'
fi
[[ "$(logical_database_sha256 "$database")" == "$db_before" ]] \
    || fail '--stop changed logical database content'
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

printf 'cli-contract-e2e: PASS (help aliases/i18n, finite rejection, loopback port, atomic REST pair, verified stop, idempotence, data preservation, invalid-lock fail-closed)\n'

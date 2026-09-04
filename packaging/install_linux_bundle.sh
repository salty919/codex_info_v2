#!/usr/bin/env bash
set -euo pipefail

# Persistent publication/control authority. The runtime wrapper reaches this
# file from the installed generation; it never reads Cargo or target/.
TARGET="x86_64-unknown-linux-gnu"
COMPATIBILITY="glibc"
PRODUCT="codex_info"
SCHEMA="codex-info-linux-bundle-v1"
CONTROL_SCHEMA="codex-info-control-state-v1"
REPOSITORY="salty919/codex_info_v2"
RELEASES_URL="https://api.github.com/repos/salty919/codex_info_v2/releases?per_page=100"
HEALTH_URL="http://127.0.0.1:8787/v1/health"
DETAILS_URL="http://127.0.0.1:8787/v1/details"
SYSTEMCTL_BIN="${SYSTEMCTL_BIN:-systemctl}"
CURL_BIN="${CURL_BIN:-curl}"
GETCONF_BIN="${GETCONF_BIN:-getconf}"
LDD_BIN="${LDD_BIN:-ldd}"
ACTION=install
ARCHIVE=
MANIFEST=
CHECKSUM=
QUIET=0
TRIGGER=manual
CONTROL_TIMEOUT=30
HEALTH_TIMEOUT=30
ROLLBACK_TIMEOUT=60
VALIDATE_TIMEOUT=60
STOP_TIMEOUT=20
MANUAL_TIMEOUT=1230
TIMER_TIMEOUT=4831
candidate_stage=
update_root=
candidate_quarantine=
candidate_created=0
lock_bypassed=0
journal_owner_pid=
journal_owner_starttime=
journal_boot_id=
previous_flat=0
operation_deadline=0
readiness_deadline=0
requested_deadline="${CODEX_INFO_DEADLINE:-}"

home_dir="$HOME"
unit_dir="$home_dir/.config/systemd/user"
local_bin="$home_dir/.local/bin"
local_libexec="$home_dir/.local/libexec"
share_dir="$home_dir/.local/share/codex-info"
generations_dir="$share_dir/generations"
backup_dir="$share_dir/legacy-backups"
binary_destination="$local_bin/codex_info"
launcher_destination="$local_bin/codex-info"
installer_destination="$local_libexec/codex-info-install.sh"
manifest_destination="$share_dir/manifest.json"
unit_destination="$unit_dir/codex-info.service"
update_service_destination="$unit_dir/codex-info-update.service"
update_timer_destination="$unit_dir/codex-info-update.timer"
current_link="$share_dir/current"
transaction="$share_dir/install-transaction.json"
control_state="$share_dir/control-state.json"
install_lock="$share_dir/.install.lock"
proc_root="$(printenv CODEX_INFO_PROC_ROOT || printf '/proc')"

usage() {
    cat <<'EOF'
usage: install.sh --bundle ARCHIVE [--manifest FILE] [--sha256 FILE]
       install.sh --update
       install.sh --start
       install.sh --stop
       install.sh --disable-autostart
       install.sh --remove
       install.sh --status
       install.sh --startup-reconcile
       install.sh --verify-runtime [--quiet]
       install.sh --verify-ui [--quiet]
EOF
}
die() { echo "linux-bundle-install: $*" >&2; exit 1; }
safe_blocked() { echo "SAFE_BLOCKED: $*" >&2; exit 1; }

while (($# > 0)); do
    case "$1" in
        --bundle|--archive)
            (($# >= 2)) || die "$1 requires an archive"
            [[ -z "$ARCHIVE" ]] || die 'bundle option supplied twice'
            ARCHIVE="$2"; shift 2 ;;
        --manifest)
            (($# >= 2)) || die '--manifest requires a path'
            [[ -z "$MANIFEST" ]] || die 'manifest option supplied twice'
            MANIFEST="$2"; shift 2 ;;
        --sha256|--checksum)
            (($# >= 2)) || die "$1 requires a path"
            [[ -z "$CHECKSUM" ]] || die 'checksum option supplied twice'
            CHECKSUM="$2"; shift 2 ;;
        --update)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'update cannot be combined with bundle options'
            ACTION=update; shift ;;
        --timer-update)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'timer-update cannot be combined with bundle options'
            ACTION=timer-update; shift ;;
        --start)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'start cannot be combined with bundle options'
            ACTION=start; shift ;;
        --stop)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'stop cannot be combined with bundle options'
            ACTION=stop; shift ;;
        --disable-autostart)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'disable-autostart cannot be combined with bundle options'
            ACTION=disable; shift ;;
        --remove)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'remove cannot be combined with bundle options'
            ACTION=remove; shift ;;
        --status)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'status cannot be combined with bundle options'
            ACTION=status; shift ;;
        --startup-reconcile|--startup)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'startup-reconcile cannot be combined with bundle options'
            ACTION=startup; shift ;;
        --startup-condition)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'startup-condition cannot be combined with bundle options'
            ACTION=startup-condition; shift ;;
        --verify-runtime|--runtime-check)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'verify-runtime cannot be combined with bundle options'
            ACTION=verify; shift ;;
        --verify-ui)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] || die 'verify-ui cannot be combined with bundle options'
            ACTION=verify-ui; shift ;;
        --quiet) QUIET=1; shift ;;
        -h|--help) usage; exit 0 ;;
        --) shift; (($# == 0)) || die 'unexpected positional argument' ;;
        *) die "unknown argument: $1" ;;
    esac
done

case "$ACTION" in
    startup) TRIGGER=startup ;;
    timer-update) TRIGGER=timer ;;
    install)
        # A child bundle install may inherit the already-authenticated trigger
        # only while it also inherits the held descriptor-9 L1 lock. External
        # environment values alone never grant startup/timer authority.
        if [[ "${CODEX_INFO_INSTALL_LOCKED:-}" == 1 && "${CODEX_INFO_INTERNAL_TRIGGER:-}" =~ ^(startup|timer)$ ]]; then
            TRIGGER="${CODEX_INFO_INTERNAL_TRIGGER}"
        fi
        ;;
esac

atomic_text() {
    local destination="$1" mode="$2" content="$3"
    python3 - "$destination" "$mode" "$content" <<'PY'
import os, sys, tempfile
from pathlib import Path
destination, mode, content = sys.argv[1:]
destination = Path(destination); destination.parent.mkdir(parents=True, exist_ok=True)
fd, temporary = tempfile.mkstemp(prefix=".codex-info.", dir=destination.parent)
try:
    with os.fdopen(fd, "w", encoding="utf-8", newline="") as output:
        output.write(content); output.flush(); os.fsync(output.fileno())
    os.chmod(temporary, int(mode, 8)); os.replace(temporary, destination)
    fd = os.open(destination.parent, os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
finally:
    try: os.unlink(temporary)
    except FileNotFoundError: pass
PY
}
atomic_symlink() {
    local target="$1" destination="$2"
    python3 - "$target" "$destination" <<'PY'
import os, sys, tempfile
from pathlib import Path
target, destination = sys.argv[1:]; destination = Path(destination)
destination.parent.mkdir(parents=True, exist_ok=True)
temporary = tempfile.mktemp(prefix=".codex-info.", dir=destination.parent)
try:
    os.symlink(target, temporary); os.replace(temporary, destination)
    fd = os.open(destination.parent, os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
finally:
    try: os.unlink(temporary)
    except FileNotFoundError: pass
PY
}
atomic_unlink() {
    local destination="$1"
    python3 - "$destination" <<'PY'
import os, sys
from pathlib import Path
path = Path(sys.argv[1])
try: path.unlink()
except FileNotFoundError: raise SystemExit(0)
fd = os.open(path.parent, os.O_DIRECTORY)
try: os.fsync(fd)
finally: os.close(fd)
PY
}

initialize_mutating_action() {
    command -v flock >/dev/null 2>&1 || die 'flock is required'
    command -v timeout >/dev/null 2>&1 || die 'timeout is required'
    local startup_preexisting=0
    if [[ "$ACTION" == startup ]]; then
        # ExecStartPre may race a legacy installer holding L1.  Startup must
        # inspect that prestate without mkdir/chmod or an O_CREAT open, and
        # the old empty regular 0644 lock is accepted only as a read-only
        # compatibility shape until L1 is acquired.
        [[ -d "$share_dir" && ! -L "$share_dir" ]] || safe_blocked 'startup installation directory is unavailable'
        [[ -f "$install_lock" && ! -L "$install_lock" ]] || safe_blocked 'startup installation lock is unavailable'
        [[ "$(stat -c '%u' -- "$install_lock" 2>/dev/null || true)" == "$(id -u)" ]] || safe_blocked 'startup installation lock owner is invalid'
        [[ "$(stat -c '%a' -- "$install_lock" 2>/dev/null || true)" == 600 ||
           "$(stat -c '%a' -- "$install_lock" 2>/dev/null || true)" == 644 ]] ||
            safe_blocked 'startup installation lock mode is invalid'
        startup_preexisting=1
    else
        mkdir -p -- "$share_dir" "$generations_dir"
        chmod 700 -- "$share_dir" "$generations_dir"
    fi
    [[ ! -d "$install_lock" ]] || safe_blocked 'install lock is a directory'
    [[ ! -L "$install_lock" ]] || safe_blocked 'install lock is a symlink'
    if [[ -e "$install_lock" ]]; then
        [[ -f "$install_lock" && "$(stat -c '%u' -- "$install_lock" 2>/dev/null || true)" == "$(id -u)" ]] ||
            safe_blocked 'install lock owner is invalid'
    fi
    locked_env="$(printenv CODEX_INFO_INSTALL_LOCKED || true)"
    if [[ "$locked_env" == 1 ]]; then
        [[ -e /proc/self/fd/9 ]] || die 'inherited installer lock is unavailable'
        lock_identity="$(stat -Lc '%d:%i' -- "$install_lock")" || die 'could not identify install lock'
        inherited_lock_identity="$(stat -Lc '%d:%i' -- /proc/self/fd/9)" || die 'could not identify inherited lock'
        [[ "$lock_identity" == "$inherited_lock_identity" ]] || die 'inherited installer lock does not match the installation lock'
        flock --exclusive --nonblock 9 || die 'inherited installer lock is not held'
        if [[ -n "$requested_deadline" ]]; then
            [[ "$requested_deadline" =~ ^[1-9][0-9]*$ ]] || safe_blocked 'inherited operation deadline is invalid'
            local inherited_now; inherited_now="$(now_unix)" || safe_blocked 'inherited operation clock is unavailable'
            (( requested_deadline >= inherited_now )) || safe_blocked 'inherited operation deadline has expired'
            operation_deadline="$requested_deadline"
        fi
    else
        umask 077
        if (( startup_preexisting )); then exec 9<"$install_lock"; else exec 9>"$install_lock"; fi
        if flock --exclusive --nonblock 9; then
            chmod 600 -- "$install_lock"
            export CODEX_INFO_INSTALL_LOCKED=1
            if (( startup_preexisting )); then
                mkdir -p -- "$share_dir" "$generations_dir"
                chmod 700 -- "$share_dir" "$generations_dir"
            fi
        elif [[ "$ACTION" == startup ]]; then
            # systemd may have requested a start while an installer is publishing.
            # Startup is allowed to perform a read-only journal/current check for
            # the live publication owner; it must never wait on or steal L1.
            lock_bypassed=1
            exec 9<&-
        else
            die 'another install, update, or control operation is already running'
        fi
    fi
    if (( operation_deadline == 0 )); then
        local action_now; action_now="$(now_unix)" || safe_blocked 'operation clock is unavailable'
        case "$ACTION" in
            start|update|install) operation_deadline=$((action_now + MANUAL_TIMEOUT)) ;;
            timer-update) operation_deadline=$((action_now + TIMER_TIMEOUT)) ;;
            startup) operation_deadline=$((action_now + MANUAL_TIMEOUT)) ;;
            stop|disable|remove) operation_deadline=$((action_now + CONTROL_TIMEOUT)) ;;
        esac
    fi
    trap cleanup_candidate_stage EXIT
}

cleanup_candidate_stage() {
    if [[ -n "$candidate_stage" && -d "$candidate_stage" && ! -L "$candidate_stage" ]]; then
        rm -r -- "$candidate_stage"
    fi
    if [[ -n "$update_root" && -d "$update_root" && ! -L "$update_root" ]]; then
        rm -r -- "$update_root"
    fi
}

deadline_timeout() {
    local default="$1" now remaining
    if (( operation_deadline > 0 )); then
        now="$(now_unix)" || return 1
        remaining=$((operation_deadline - now))
        (( remaining > 0 )) || return 1
        (( remaining < default )) && default=$remaining
    fi
    printf '%s\n' "$default"
}
systemctl_user() {
    local limit
    limit="$(deadline_timeout "$CONTROL_TIMEOUT")" || return 124
    timeout --foreground "$limit" "$SYSTEMCTL_BIN" --user "$@"
}
systemctl_stop_user() {
    local limit
    limit="$(deadline_timeout "$STOP_TIMEOUT")" || return 124
    timeout --foreground "$limit" "$SYSTEMCTL_BIN" --user "$@"
}
require_user_manager() {
    systemctl_user show-environment >/dev/null 2>&1 || die 'systemd user manager is unavailable'
}
probe_enabled() {
    local unit="$1" status=0
    systemctl_user is-enabled --quiet "$unit" >/dev/null 2>&1 || status="$?"
    case "$status" in 0) return 0 ;; 1) return 1 ;; *) die "could not inspect enabled state for $unit" ;; esac
}
probe_active() {
    local unit="$1" status=0
    systemctl_user is-active --quiet "$unit" >/dev/null 2>&1 || status="$?"
    case "$status" in 0) return 0 ;; 3) return 1 ;; *) die "could not inspect active state for $unit" ;; esac
}
now_unix() {
    local value
    if [[ -n "${CODEX_INFO_CLOCK_BIN:-}" ]]; then
        value="$("$CODEX_INFO_CLOCK_BIN")" || return 1
    else
        value="$(date +%s)"
    fi
    [[ "$value" =~ ^[0-9]+$ ]] || return 1
    printf '%s\n' "$value"
}
sleep_interval() {
    if [[ -n "${CODEX_INFO_SLEEP_BIN:-}" ]]; then
        "$CODEX_INFO_SLEEP_BIN" "$1"
    else
        sleep "$1"
    fi
}
wait_inactive() {
    local unit="$1" now deadline status
    now="$(now_unix)" || return 1
    deadline=$(( now + STOP_TIMEOUT ))
    if (( operation_deadline > 0 && operation_deadline < deadline )); then deadline=$operation_deadline; fi
    while :; do
        status=0
        timeout --foreground "$STOP_TIMEOUT" "$SYSTEMCTL_BIN" --user is-active --quiet "$unit" >/dev/null 2>&1 || status="$?"
        case "$status" in
            3) return 0 ;;
            0) ;;
            *) return 1 ;;
        esac
        now="$(now_unix)" || return 1
        (( now < deadline )) || return 1
        sleep_interval 1
    done
}
wait_runtime_ready() {
    local now deadline previous_readiness_deadline
    now="$(now_unix)" || return 1
    deadline=$(( now + HEALTH_TIMEOUT ))
    if (( operation_deadline > 0 && operation_deadline < deadline )); then deadline=$operation_deadline; fi
    previous_readiness_deadline=$readiness_deadline
    readiness_deadline=$deadline
    while :; do
        if (probe_active codex-info.service); then
            if (verify_runtime >/dev/null 2>&1); then
                readiness_deadline=$previous_readiness_deadline
                return 0
            fi
        fi
        now="$(now_unix)" || return 1
        if (( now >= deadline )); then
            readiness_deadline=$previous_readiness_deadline
            return 1
        fi
        sleep_interval 1
    done
}
systemd_pid() {
    local pid
    pid="$(systemctl_user show --property=MainPID --value codex-info.service 2>/dev/null)" || die 'could not read MainPID'
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || { printf '0\n'; return; }
    printf '%s\n' "$pid"
}
boot_id() {
    [[ -r /proc/sys/kernel/random/boot_id ]] && tr -d '[:space:]' < /proc/sys/kernel/random/boot_id || printf 'unknown\n'
}
new_operation_id() { printf '%s-%s-%s\n' "$(now_unix)" "$$" "$RANDOM"; }
owner_starttime() {
    python3 - "$$" <<'PY'
from pathlib import Path
import sys
text = Path("/proc").joinpath(sys.argv[1], "stat").read_text(encoding="utf-8")
fields = text.rsplit(") ", 1)[1].split()
if len(fields) < 20:
    raise SystemExit("owner process stat is malformed")
print(fields[19])
PY
}

control_defaults() {
    desired_state=running; state_boot_id="$(boot_id)"
}
load_control_state() {
    control_defaults; [[ -e "$control_state" ]] || return
    [[ -f "$control_state" && ! -L "$control_state" ]] || safe_blocked 'control-state.json is not regular'
    [[ "$(stat -c '%u' -- "$control_state" 2>/dev/null || true)" == "$(id -u)" &&
       "$(stat -c '%a' -- "$control_state" 2>/dev/null || true)" == 600 ]] ||
        safe_blocked 'control-state.json owner or mode is invalid'
    state_line="$(python3 - "$control_state" "$CONTROL_SCHEMA" <<'PY'
import json, pathlib, re, sys
path, schema = sys.argv[1:]
def pairs(items):
    result = {}
    for key, value in items:
        if key in result: raise ValueError("duplicate key")
        result[key] = value
    return result
try: document = json.loads(pathlib.Path(path).read_text(encoding="utf-8"), object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
required = {"schema","desired_state","boot_id","operation_id","generation_id","updated_at_unix"}
if not isinstance(document, dict) or set(document) != required:
    raise SystemExit("state keys are not exact")
if document["schema"] != schema or document["desired_state"] not in {"running","stopped","disabled","removed"}:
    raise SystemExit("state identity is invalid")
for key in ("boot_id","operation_id"):
    if not isinstance(document[key], str) or not document[key]: raise SystemExit("state identity is invalid")
generation = document["generation_id"]
if not isinstance(generation, str) or (generation and not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)-[0-9a-f]{40}-[0-9a-f]{64}", generation)):
    raise SystemExit("state generation identity is invalid")
if isinstance(document["updated_at_unix"], bool) or not isinstance(document["updated_at_unix"], int) or document["updated_at_unix"] <= 0:
    raise SystemExit("state timestamp is invalid")
print(document["desired_state"], document["boot_id"], document["operation_id"], document["generation_id"], document["updated_at_unix"], sep="\t")
PY
    )" || safe_blocked 'control-state.json is invalid or ambiguous'
    IFS=$'\t' read -r desired_state state_boot_id _ _ _ <<<"$state_line"
    if [[ "$state_boot_id" != "$(boot_id)" && "$desired_state" == stopped ]]; then desired_state=running; fi
}
write_control_state() {
    local desired="$1" operation timestamp generation content
    operation="$(new_operation_id)"; timestamp="$(now_unix)" || safe_blocked 'control-state clock is unavailable'
    generation="$(current_generation || true)"
    content="$(python3 - "$CONTROL_SCHEMA" "$desired" "$(boot_id)" "$operation" "$generation" "$timestamp" <<'PY'
import json, re, sys
schema, desired, boot, operation, generation, timestamp = sys.argv[1:]
if generation and not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)-[0-9a-f]{40}-[0-9a-f]{64}", generation):
    raise SystemExit("state generation identity is invalid")
print(json.dumps({"schema":schema,"desired_state":desired,"boot_id":boot,"operation_id":operation,"generation_id":generation,"updated_at_unix":int(timestamp)}, separators=(",",":")))
PY
    )"
    atomic_text "$control_state" 600 "$content"
}

write_journal() {
    local phase="$1" timestamp
    if [[ -z "$journal_owner_pid" ]]; then
        journal_owner_pid="$$"
        journal_owner_starttime="$(owner_starttime)" || safe_blocked 'journal owner starttime is unavailable'
        journal_boot_id="$(boot_id)"
    fi
    [[ "$journal_owner_pid" =~ ^[1-9][0-9]*$ && "$journal_owner_starttime" =~ ^[1-9][0-9]*$ && -n "$journal_boot_id" ]] ||
        safe_blocked 'journal owner identity is invalid'
    timestamp="$(now_unix)" || safe_blocked 'transaction journal clock is unavailable'
    local content
    content="$(python3 - "$phase" "$operation_id" "$journal_owner_pid" "$journal_owner_starttime" "$journal_boot_id" "$previous_id" "$candidate_id" "$desired_state" "$timestamp" <<'PY'
import json, sys
phase, operation, owner_pid, owner_starttime, boot, old_generation, new_generation, desired, timestamp = sys.argv[1:]
document = {"schema":"codex-info-install-transaction-v1","operation_id":operation,
            "owner_pid":int(owner_pid),"owner_starttime":int(owner_starttime),"boot_id":boot,
            "phase":phase,"old_generation":old_generation,"new_generation":new_generation,
            "desired_state":desired,"updated_at_unix":int(timestamp)}
print(json.dumps(document, ensure_ascii=False, indent=2) + "\n", end="")
PY
    )"
    atomic_text "$transaction" 600 "$content"
    if [[ "${CODEX_INFO_INTERRUPT_PHASE-}" == "$phase" ]]; then exit 75; fi
}
read_journal() {
    [[ -f "$transaction" && ! -L "$transaction" ]] || safe_blocked 'transaction journal is not regular'
    [[ "$(stat -c '%u' -- "$transaction" 2>/dev/null || true)" == "$(id -u)" &&
       "$(stat -c '%a' -- "$transaction" 2>/dev/null || true)" == 600 ]] ||
        safe_blocked 'transaction journal owner or mode is invalid'
    journal_line="$(python3 - "$transaction" <<'PY'
import json, pathlib, re, sys
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate key")
        result[key]=value
    return result
try: document=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"), object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
required={"schema","operation_id","owner_pid","owner_starttime","boot_id","phase","old_generation","new_generation","desired_state","updated_at_unix"}
if not isinstance(document,dict) or set(document)!=required or document["schema"]!="codex-info-install-transaction-v1":
    raise SystemExit("journal keys are invalid")
if document["phase"] not in {"prepared","legacy_backed_up","entrypoints_linked","candidate_published","current_switched","activation_requested","candidate_verified","rollback_switched","rollback_verified","committed"}:
    raise SystemExit("journal phase is invalid")
if (isinstance(document["owner_pid"],bool) or not isinstance(document["owner_pid"],int) or document["owner_pid"] <= 0 or
        isinstance(document["owner_starttime"],bool) or not isinstance(document["owner_starttime"],int) or document["owner_starttime"] <= 0 or
        not isinstance(document["boot_id"],str) or not document["boot_id"] or not isinstance(document["operation_id"],str) or not document["operation_id"] or
        document["desired_state"] not in {"running","stopped","disabled","removed"}):
    raise SystemExit("journal state is invalid")
generation_pattern=r"(?:|(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)-[0-9a-f]{40}-[0-9a-f]{64})"
if (not isinstance(document["old_generation"],str) or not re.fullmatch(generation_pattern,document["old_generation"]) or
        not isinstance(document["new_generation"],str) or not re.fullmatch(generation_pattern,document["new_generation"]) or
        isinstance(document["updated_at_unix"],bool) or not isinstance(document["updated_at_unix"],int) or document["updated_at_unix"] <= 0):
    raise SystemExit("journal generation or timestamp is invalid")
print(document["phase"],document["operation_id"],document["owner_pid"],document["owner_starttime"],
      document["boot_id"],document["old_generation"],document["new_generation"],document["desired_state"],sep="\x1f")
PY
    )" || safe_blocked 'transaction journal is invalid or ambiguous'
    IFS=$'\x1f' read -r journal_phase journal_operation_id journal_owner_pid journal_owner_starttime journal_boot_id journal_previous_id journal_candidate_id journal_desired <<<"$journal_line"
}
journal_owner_stale() {
    [[ "$journal_boot_id" != "$(boot_id)" ]] && return 0
    [[ "$journal_owner_pid" == "$$" ]] && return 1
    if ! kill -0 "$journal_owner_pid" 2>/dev/null; then return 0; fi
    local observed
    observed="$(python3 - "$journal_owner_pid" <<'PY'
from pathlib import Path
import sys
try:
    text = Path("/proc").joinpath(sys.argv[1], "stat").read_text(encoding="utf-8")
    fields = text.rsplit(") ", 1)[1].split()
    if len(fields) < 20: raise ValueError
    print(fields[19])
except Exception:
    raise SystemExit(1)
PY
    )" || return 0
    [[ "$observed" != "$journal_owner_starttime" ]]
}

# systemd runs ExecCondition in a separate process while an installer may
# still hold the transaction lock.  That condition may admit only the
# generation that the live installer has already switched to.  The journal
# owner identity and its inherited descriptor-9 lock are the authority; an
# environment variable or a merely present journal is never sufficient.
transaction_startup_generation() {
    case "$journal_phase" in
        current_switched|activation_requested|candidate_verified)
            [[ -n "$journal_candidate_id" ]] || return 1
            printf '%s\n' "$journal_candidate_id"
            ;;
        rollback_switched|rollback_verified)
            [[ -n "$journal_previous_id" ]] || return 1
            printf '%s\n' "$journal_previous_id"
            ;;
        *) return 1 ;;
    esac
}
transaction_owner_holds_install_lock() {
    local lock_identity owner_fd owner_identity
    [[ "$journal_boot_id" == "$(boot_id)" ]] || return 1
    journal_owner_stale && return 1
    [[ -f "$install_lock" && ! -L "$install_lock" ]] || return 1
    [[ "$(stat -c '%u' -- "$install_lock" 2>/dev/null || true)" == "$(id -u)" &&
       "$(stat -c '%a' -- "$install_lock" 2>/dev/null || true)" == 600 ]] || return 1
    lock_identity="$(stat -Lc '%d:%i' -- "$install_lock" 2>/dev/null || true)"
    [[ -n "$lock_identity" ]] || return 1
    owner_fd="/proc/$journal_owner_pid/fd/9"
    [[ -e "$owner_fd" ]] || return 1
    owner_identity="$(stat -Lc '%d:%i' -- "$owner_fd" 2>/dev/null || true)"
    [[ "$owner_identity" == "$lock_identity" ]]
}
transaction_startup_authorized() {
    local expected current
    [[ "$journal_desired" == running ]] || return 1
    transaction_owner_holds_install_lock || return 1
    expected="$(transaction_startup_generation)" || return 1
    current="$(current_generation 2>/dev/null || true)"
    [[ "$current" == "$expected" ]] || return 1
    verify_generation_files "$generations_dir/$expected" >/dev/null 2>&1 || return 1
    verify_fixed_links_local || return 1
}

current_generation() {
    [[ -L "$current_link" ]] || return 0
    local target; target="$(readlink -- "$current_link")"
    [[ "$target" == generations/* && "$target" != */*/* ]] || safe_blocked 'current link is invalid'
    printf '%s\n' "${target#generations/}"
}
manifest_record() {
    local path="${1:-$manifest_destination}"
    if [[ "$path" == "$manifest_destination" ]]; then
        [[ -L "$manifest_destination" ]] || safe_blocked 'manifest fixed link is absent'
    else
        [[ -f "$path" && ! -L "$path" ]] || safe_blocked 'generation manifest is absent'
    fi
    python3 - "$path" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib,json,pathlib,re,sys
path,schema,product,target,compatibility=sys.argv[1:]
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate key")
        result[key]=value
    return result
try:
    raw=pathlib.Path(path).read_bytes(); document=json.loads(raw.decode("utf-8"),object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
if not isinstance(document,dict) or document.get("schema")!=schema or document.get("product")!=product or document.get("target")!=target or document.get("compatibility")!=compatibility:
    raise SystemExit("manifest identity is invalid")
version,source,entries=document.get("version"),document.get("source_sha"),document.get("files")
if (not isinstance(version,str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",version)
    or not isinstance(source,str) or not re.fullmatch(r"[0-9a-f]{40}",source) or not isinstance(entries,list)):
    raise SystemExit("manifest fields are invalid")
paths=[]; binary=None
for entry in entries:
    if not isinstance(entry,dict) or set(entry)!={"path","size","sha256","mode"}:
        raise SystemExit("manifest file entry is invalid")
    path_name=entry["path"]
    if (not isinstance(path_name,str) or not path_name or path_name.startswith("/") or "\\" in path_name or
            any(part in {"",".",".."} for part in path_name.split("/")) or path_name in paths):
        raise SystemExit("manifest file path is invalid")
    if (isinstance(entry["size"],bool) or not isinstance(entry["size"],int) or entry["size"] < 0 or
            not isinstance(entry["sha256"],str) or not re.fullmatch(r"[0-9a-f]{64}",entry["sha256"]) or
            isinstance(entry["mode"],bool) or not isinstance(entry["mode"],int) or entry["mode"] not in {0o644,0o755}):
        raise SystemExit("manifest file identity is invalid")
    paths.append(path_name)
    if path_name == "codex_info": binary=entry
if paths != sorted(paths) or binary is None:
    raise SystemExit("manifest file entries are invalid")
print(version,source,hashlib.sha256(raw).hexdigest(),binary["sha256"],sep="\t")
PY
}
legacy_flat_record() {
    [[ -f "$manifest_destination" && ! -L "$manifest_destination" ]] || return 1
    python3 - "$manifest_destination" "$binary_destination" "$installer_destination" \
        "$unit_destination" "$update_service_destination" "$update_timer_destination" \
        "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib,json,os,pathlib,re,stat,sys
(manifest_name,binary_name,installer_name,unit_name,update_service_name,update_timer_name,
 schema,product,target,compatibility)=sys.argv[1:]
manifest_path=pathlib.Path(manifest_name)
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate legacy manifest key")
        result[key]=value
    return result
def regular(path,mode):
    path=pathlib.Path(path)
    if not path.is_file() or path.is_symlink(): raise SystemExit("legacy flat member is not regular")
    metadata=path.stat()
    if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != mode:
        raise SystemExit("legacy flat member owner or mode is not trusted")
    return path
try:
    raw=manifest_path.read_bytes(); document=json.loads(raw.decode("utf-8"),object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
regular(manifest_path,0o644)
required={"schema","product","version","source_sha","run_id","run_attempt","target","compatibility","glibc_minimum","files"}
if not isinstance(document,dict) or set(document)!=required: raise SystemExit("legacy manifest top-level keys are invalid")
if document["schema"]!=schema or document["product"]!=product or document["target"]!=target or document["compatibility"]!=compatibility:
    raise SystemExit("legacy manifest identity is invalid")
version,source=document["version"],document["source_sha"]
if (not isinstance(version,str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",version) or
    not isinstance(source,str) or not re.fullmatch(r"[0-9a-f]{40}",source) or
    not isinstance(document["run_id"],str) or not re.fullmatch(r"[1-9][0-9]*",document["run_id"]) or
    isinstance(document["run_attempt"],bool) or not isinstance(document["run_attempt"],int) or document["run_attempt"]<1 or
    not isinstance(document["glibc_minimum"],str) or not re.fullmatch(r"[0-9]+(?:[.][0-9]+)+",document["glibc_minimum"])):
    raise SystemExit("legacy manifest identity fields are invalid")
entries=document["files"]
if not isinstance(entries,list) or not entries: raise SystemExit("legacy manifest files are invalid")
by_path={}; ordered=[]
for entry in entries:
    if not isinstance(entry,dict) or set(entry)!={"path","size","sha256"}: raise SystemExit("legacy manifest entry schema is invalid")
    name=entry["path"]
    if (not isinstance(name,str) or not name or name.startswith("/") or "\\" in name or
        any(part in {"",".",".."} for part in name.split("/")) or name in by_path):
        raise SystemExit("legacy manifest path is invalid")
    if (isinstance(entry["size"],bool) or not isinstance(entry["size"],int) or entry["size"]<0 or
        not isinstance(entry["sha256"],str) or not re.fullmatch(r"[0-9a-f]{64}",entry["sha256"])):
        raise SystemExit("legacy manifest file identity is invalid")
    by_path[name]=entry; ordered.append(name)
if ordered != sorted(ordered): raise SystemExit("legacy manifest files are not sorted")
paths={"codex_info":(binary_name,0o755),"install.sh":(installer_name,0o755),
       "codex-info.service":(unit_name,0o644),"codex-info-update.service":(update_service_name,0o644),
       "codex-info-update.timer":(update_timer_name,0o644)}
if set(paths)-set(by_path): raise SystemExit("legacy manifest omits required flat member")
for name,(actual_name,mode) in paths.items():
    actual=regular(actual_name,mode); entry=by_path[name]
    if actual.stat().st_size != entry["size"] or hashlib.sha256(actual.read_bytes()).hexdigest()!=entry["sha256"]:
        raise SystemExit("legacy flat member does not match manifest")
print(version,source,hashlib.sha256(raw).hexdigest(),by_path["codex_info"]["sha256"],manifest_path.stat().st_size,sep="\t")
PY
}
legacy_flat_present() {
    local path
    for path in "$manifest_destination" "$binary_destination" "$installer_destination" \
        "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ -e "$path" || -L "$path" ]] && return 0
    done
    return 1
}
verify_generation_files() {
    local generation_dir_name="$1"
    python3 - "$generation_dir_name" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib, json, os, pathlib, re, stat, sys
root = pathlib.Path(sys.argv[1]); schema, product, target, compatibility = sys.argv[2:]
if (not root.is_dir() or root.is_symlink() or root.stat().st_uid != os.getuid() or
        stat.S_IMODE(root.stat().st_mode) != 0o700):
    raise SystemExit("generation directory is not owner-only")
manifest_path = root / "manifest.json"
if not manifest_path.is_file() or manifest_path.is_symlink(): raise SystemExit("generation manifest unavailable")
if manifest_path.stat().st_uid != os.getuid() or stat.S_IMODE(manifest_path.stat().st_mode) != 0o644:
    raise SystemExit("generation manifest owner or mode differs")
def pairs(items):
    result = {}
    for key, value in items:
        if key in result: raise ValueError("duplicate manifest key")
        result[key] = value
    return result
try: document = json.loads(manifest_path.read_text(encoding="utf-8"), object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
required = {"schema","product","version","source_sha","run_id","run_attempt","target","compatibility","glibc_minimum","files"}
if not isinstance(document, dict) or set(document) != required: raise SystemExit("generation manifest keys are invalid")
if document["schema"] != schema or document["product"] != product or document["target"] != target or document["compatibility"] != compatibility:
    raise SystemExit("generation manifest identity is invalid")
if (not isinstance(document["version"], str) or
        not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)", document["version"]) or
        not isinstance(document["source_sha"], str) or not re.fullmatch(r"[0-9a-f]{40}", document["source_sha"]) or
        not isinstance(document["run_id"], str) or not re.fullmatch(r"[1-9][0-9]*", document["run_id"]) or
        isinstance(document["run_attempt"], bool) or not isinstance(document["run_attempt"], int) or document["run_attempt"] < 1):
    raise SystemExit("generation version identity is invalid")
if not isinstance(document["glibc_minimum"], str) or not re.fullmatch(r"[0-9]+(?:[.][0-9]+)+", document["glibc_minimum"]):
    raise SystemExit("generation glibc identity is invalid")
manifest_hash = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
if root.name != document["version"] + "-" + document["source_sha"] + "-" + manifest_hash:
    raise SystemExit("generation directory identity is invalid")
entries = document["files"]
if not isinstance(entries, list): raise SystemExit("generation manifest files are invalid")
expected = set()
ordered = []
for entry in entries:
    if not isinstance(entry, dict) or set(entry) != {"path","size","sha256","mode"}: raise SystemExit("generation entry is invalid")
    path = entry["path"]
    if not isinstance(path, str) or not path or path.startswith("/") or "\\" in path or any(part in {"", ".", ".."} for part in path.split("/")):
        raise SystemExit("generation path is unsafe")
    if (path in expected or not isinstance(entry["size"], int) or isinstance(entry["size"], bool) or entry["size"] < 0 or
            not re.fullmatch(r"[0-9a-f]{64}", str(entry["sha256"])) or
            isinstance(entry["mode"], bool) or not isinstance(entry["mode"], int) or entry["mode"] not in {0o644,0o755}):
        raise SystemExit("generation entry identity is invalid")
    expected.add(path)
    ordered.append(path)
if ordered != sorted(ordered): raise SystemExit("generation manifest entries are not sorted")
actual = set()
for path in root.rglob("*"):
    if path.is_symlink(): raise SystemExit("generation contains a symlink")
    if path.is_dir():
        if path.stat().st_uid != os.getuid() or stat.S_IMODE(path.stat().st_mode) != 0o700: raise SystemExit("generation subdirectory is not owner-only")
    elif path.is_file():
        if path.stat().st_uid != os.getuid(): raise SystemExit("generation member owner differs")
        actual.add(path.relative_to(root).as_posix())
if actual != expected | {"manifest.json", "SHA256SUMS"}: raise SystemExit("generation member set differs")
for entry in entries:
    path = root / entry["path"]
    if not path.is_file() or path.is_symlink(): raise SystemExit("generation member is not regular")
    mode = stat.S_IMODE(path.stat().st_mode)
    expected_mode = entry["mode"]
    if mode != expected_mode or path.stat().st_size != entry["size"]: raise SystemExit("generation mode or size differs")
    digest = hashlib.sha256(path.read_bytes()).hexdigest()
    if digest != entry["sha256"]: raise SystemExit("generation digest differs")
sum_path = root / "SHA256SUMS"
if sum_path.stat().st_uid != os.getuid() or stat.S_IMODE(sum_path.stat().st_mode) != 0o644: raise SystemExit("generation checksum owner or mode differs")
records = {}
try:
    lines = sum_path.read_text(encoding="utf-8").splitlines()
except Exception as error:
    raise SystemExit(str(error))
for line in lines:
    fields = line.split()
    if len(fields) != 2 or not re.fullmatch(r"[0-9a-f]{64}", fields[0]):
        raise SystemExit("generation checksum record is invalid")
    name = fields[1].removeprefix("*")
    if name in records: raise SystemExit("generation checksum has duplicate member")
    records[name] = fields[0]
if set(records) != expected | {"manifest.json"}: raise SystemExit("generation checksum coverage differs")
for name, digest in records.items():
    if name == "manifest.json":
        member = manifest_path
    else:
        member = root / name
    if hashlib.sha256(member.read_bytes()).hexdigest() != digest:
        raise SystemExit("generation checksum digest differs")
PY
}
verify_fixed_links() {
    local destination expected
    for destination in "$binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ -L "$destination" ]] || safe_blocked "fixed link missing: $destination"
    done
    [[ "$(readlink -- "$binary_destination")" == '../share/codex-info/current/codex_info' ]] || safe_blocked 'binary link is not canonical'
    [[ "$(readlink -- "$launcher_destination")" == '../share/codex-info/current/run.sh' ]] || safe_blocked 'launcher link is not canonical'
    [[ "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' ]] || safe_blocked 'installer link is not canonical'
    [[ "$(readlink -- "$manifest_destination")" == 'current/manifest.json' ]] || safe_blocked 'manifest link is not canonical'
    for destination in "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        expected="../../../.local/share/codex-info/current/$(basename -- "$destination")"
        [[ "$(readlink -- "$destination")" == "$expected" ]] || safe_blocked "unit link is not canonical"
    done
}
verify_fixed_links_local() {
    local destination expected
    for destination in "$binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ -L "$destination" ]] || return 1
    done
    [[ "$(readlink -- "$binary_destination")" == '../share/codex-info/current/codex_info' ]] || return 1
    [[ "$(readlink -- "$launcher_destination")" == '../share/codex-info/current/run.sh' ]] || return 1
    [[ "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' ]] || return 1
    [[ "$(readlink -- "$manifest_destination")" == 'current/manifest.json' ]] || return 1
    for destination in "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        expected="../../../.local/share/codex-info/current/$(basename -- "$destination")"
        [[ "$(readlink -- "$destination")" == "$expected" ]] || return 1
    done
}
verify_local_generation() {
    [[ -L "$current_link" ]] || return 1
    local target generation
    target="$(readlink -- "$current_link")"
    [[ "$target" == generations/* && "$target" != */*/* ]] || return 1
    generation="${target#generations/}"
    [[ -d "$generations_dir/$generation" && ! -L "$generations_dir/$generation" ]] || return 1
    verify_generation_files "$generations_dir/$generation" || return 1
    if [[ "$desired_state" == removed ]]; then
        [[ -L "$binary_destination" && "$(readlink -- "$binary_destination")" == '../share/codex-info/current/codex_info' ]] || return 1
        [[ -L "$launcher_destination" && "$(readlink -- "$launcher_destination")" == '../share/codex-info/current/run.sh' ]] || return 1
        [[ -L "$installer_destination" && "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' ]] || return 1
        [[ -L "$manifest_destination" && "$(readlink -- "$manifest_destination")" == 'current/manifest.json' ]] || return 1
        [[ ! -e "$unit_destination" && ! -L "$unit_destination" ]] || return 1
        [[ ! -e "$update_service_destination" && ! -L "$update_service_destination" ]] || return 1
        [[ ! -e "$update_timer_destination" && ! -L "$update_timer_destination" ]] || return 1
    else
        verify_fixed_links_local || return 1
    fi
}
validate_external_checksum() {
    local archive="$1" sum="$2" count hash name extra
    [[ -f "$sum" && ! -L "$sum" ]] || die 'external checksum is not regular'
    count="$(awk 'NF && $0 !~ /^[[:space:]]*#/ {n++} END {print n+0}' "$sum")"
    [[ "$count" == 1 ]] || die 'external checksum must contain exactly one record'
    read -r hash name extra < "$sum" || die 'external checksum cannot be read'
    [[ -z "$extra" ]] || die 'external checksum has extra fields'
    name="${name#\*}"
    [[ "$name" == "$(basename -- "$archive")" ]] || die 'external checksum names wrong archive'
    [[ "$hash" =~ ^[0-9a-fA-F]{64}$ ]] || die 'external checksum value is invalid'
    [[ "$(sha256sum -- "$archive" | awk '{print $1}')" == "$(printf '%s' "$hash" | tr '[:upper:]' '[:lower:]')" ]] || die 'bundle SHA-256 does not match external checksum'
}
check_glibc_compatibility() {
    local manifest="$1" host_text host_version
    host_text="$("$GETCONF_BIN" GNU_LIBC_VERSION 2>/dev/null || true)"
    host_version="${host_text#*glibc }"
    if [[ ! "$host_version" =~ ^[0-9]+([.][0-9]+)+$ ]]; then
        host_text="$("$LDD_BIN" --version 2>/dev/null | head -n 1 || true)"
        host_version="$(printf '%s\n' "$host_text" | grep -oE '[0-9]+(\.[0-9]+)+' | head -n 1 || true)"
    fi
    [[ "$host_version" =~ ^[0-9]+([.][0-9]+)+$ ]] || die 'host glibc version is unavailable'
    python3 - "$manifest" "$host_version" <<'PY'
import json,pathlib,re,sys
manifest,host_text=sys.argv[1:]
try:
    document=json.loads(pathlib.Path(manifest).read_text(encoding="utf-8"))
except Exception as error:
    raise SystemExit(f"manifest glibc identity is unreadable: {error}")
minimum=document.get("glibc_minimum") if isinstance(document,dict) else None
if not isinstance(minimum,str) or not re.fullmatch(r"[0-9]+(?:[.][0-9]+)+",minimum):
    raise SystemExit("manifest glibc minimum is invalid")
host=tuple(int(part) for part in host_text.split(".")); required=tuple(int(part) for part in minimum.split("."))
if host < required: raise SystemExit("host glibc is older than candidate minimum")
PY
}

validate_bundle() {
    local archive="$1" external="$2" validation_limit="$VALIDATE_TIMEOUT"
    [[ -f "$archive" && ! -L "$archive" ]] || die 'bundle archive is not regular'
    [[ "$archive" == *.tar.gz ]] || die 'bundle archive has wrong suffix'
    [[ -f "$external" && ! -L "$external" ]] || die 'external manifest is not regular'
    if (( operation_deadline > 0 )); then
        validation_limit="$(deadline_timeout "$VALIDATE_TIMEOUT")" || return 1
    fi
    python3 - "$archive" "$external" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" "$validation_limit" <<'PY'
import hashlib,json,pathlib,re,sys,tarfile
import signal
archive_name,manifest_name,schema,product,target,compatibility,timeout_seconds=sys.argv[1:]
def timeout_handler(signum, frame): raise TimeoutError("bundle validation timed out")
signal.signal(signal.SIGALRM, timeout_handler); signal.alarm(int(timeout_seconds))
def reject(message): raise SystemExit("bundle validation failed: "+message)
def pairs(items):
    result={}
    for key,value in items:
        if key in result: reject("duplicate JSON key")
        result[key]=value
    return result
try:
    raw=pathlib.Path(manifest_name).read_bytes(); manifest=json.loads(raw.decode("utf-8"),object_pairs_hook=pairs)
except Exception as error: reject(str(error))
required={"schema","product","version","source_sha","run_id","run_attempt","target","compatibility","glibc_minimum","files"}
if not isinstance(manifest,dict) or set(manifest)!=required: reject("manifest keys are not exact")
if manifest["schema"]!=schema or manifest["product"]!=product: reject("manifest identity")
if not isinstance(manifest["version"],str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",manifest["version"]): reject("version")
if pathlib.Path(archive_name).name != f"codex-info-{manifest['version']}-{target}.tar.gz": reject("archive name")
if not isinstance(manifest["source_sha"],str) or not re.fullmatch(r"[0-9a-f]{40}",manifest["source_sha"]): reject("source")
if not isinstance(manifest["run_id"],str) or not re.fullmatch(r"[1-9][0-9]*",manifest["run_id"]): reject("run id")
if isinstance(manifest["run_attempt"],bool) or not isinstance(manifest["run_attempt"],int) or manifest["run_attempt"]<1: reject("run attempt")
if manifest["target"]!=target or manifest["compatibility"]!=compatibility: reject("target")
if not isinstance(manifest["glibc_minimum"],str) or not re.fullmatch(r"[0-9]+(?:[.][0-9]+)+",manifest["glibc_minimum"]): reject("glibc")
entries=manifest["files"]
if not isinstance(entries,list) or not entries: reject("files")
paths=[]; by_path={}
for entry in entries:
    if not isinstance(entry,dict) or set(entry)!={"path","size","sha256","mode"}: reject("file entry")
    path=entry["path"]
    if not isinstance(path,str) or not path or path.startswith("/") or "\\" in path or path.startswith("./") or any(part in {"",".",".."} for part in path.split("/")): reject("unsafe path")
    if path in by_path: reject("duplicate path")
    if (isinstance(entry["size"],bool) or not isinstance(entry["size"],int) or entry["size"]<0 or
            not isinstance(entry["sha256"],str) or not re.fullmatch(r"[0-9a-f]{64}",entry["sha256"]) or
            isinstance(entry["mode"],bool) or not isinstance(entry["mode"],int) or entry["mode"] not in {0o644,0o755}): reject("file identity")
    paths.append(path); by_path[path]=entry
if paths!=sorted(paths): reject("files not sorted")
required_files={"codex_info","run.sh","install.sh","codex-info.service","codex-info-update.service","codex-info-update.timer","LICENSE","COPYRIGHT"}
if not required_files.issubset(by_path) or not ({"THIRD_PARTY_NOTICES.md","NOTICE.txt"} & set(by_path)): reject("required member missing")
try:
    with tarfile.open(archive_name,"r:gz") as archive:
        actual=[]
        for member in archive.getmembers():
            path=member.name
            if not path or path.startswith("/") or "\\" in path or path.startswith("./") or any(part in {"",".",".."} for part in path.split("/")) or not member.isfile(): reject("unsafe member")
            if path in actual: reject("duplicate member")
            actual.append(path)
        if actual!=sorted(actual): reject("members not sorted")
        if set(actual)!=set(by_path)|{"manifest.json","SHA256SUMS"}: reject("member set differs")
        internal=archive.extractfile("manifest.json")
        if internal is None or internal.read()!=raw: reject("manifest bytes differ")
        sums=archive.extractfile("SHA256SUMS")
        if sums is None: reject("SHA256SUMS missing")
        records={}
        for line in sums.read().decode("utf-8").splitlines():
            fields=line.split()
            if len(fields)!=2 or not re.fullmatch(r"[0-9a-f]{64}",fields[0]): reject("bad SHA256SUMS")
            name=fields[1].removeprefix("*")
            if name in records: reject("duplicate SHA256SUMS")
            records[name]=fields[0]
        if set(records)!=set(actual)-{"SHA256SUMS"}: reject("SHA256SUMS coverage")
        for path,expected in records.items():
            digest=hashlib.sha256(); stream=archive.extractfile(path)
            while chunk:=stream.read(1024*1024): digest.update(chunk)
            if digest.hexdigest()!=expected: reject("SHA256SUMS digest")
        for path,entry in by_path.items():
            member=archive.getmember(path); mode=member.mode&0o7777
            expected_mode=0o755 if path in {"codex_info","run.sh","install.sh"} else 0o644
            if mode!=expected_mode or mode!=entry["mode"] or member.size!=entry["size"]: reject("mode/size mismatch")
            digest=hashlib.sha256(); stream=archive.extractfile(member)
            while chunk:=stream.read(1024*1024): digest.update(chunk)
            if digest.hexdigest()!=entry["sha256"]: reject("manifest digest")
        for path in ("manifest.json", "SHA256SUMS"):
            if archive.getmember(path).mode&0o7777 != 0o644: reject("metadata mode mismatch")
except (OSError,tarfile.TarError,UnicodeError) as error: reject(str(error))
signal.alarm(0)
print(manifest["version"],manifest["source_sha"],hashlib.sha256(raw).hexdigest(),by_path["codex_info"]["sha256"],sep="\t")
PY
}
extract_candidate() {
    local archive="$1" destination="$2"
    chmod 700 -- "$destination"
    python3 - "$archive" "$destination" <<'PY'
import os,pathlib,tarfile,tempfile,sys
archive_name,destination_name=sys.argv[1:]; destination=pathlib.Path(destination_name)
with tarfile.open(archive_name,"r:gz") as archive:
    for member in archive.getmembers():
        if not member.isfile(): raise SystemExit("candidate member is not regular")
        target=destination.joinpath(*pathlib.PurePosixPath(member.name).parts); target.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
        source=archive.extractfile(member)
        if source is None: raise SystemExit("candidate member is unreadable")
        fd,temporary=tempfile.mkstemp(prefix=".codex-info.",dir=target.parent)
        try:
            with os.fdopen(fd,"wb") as output:
                while chunk:=source.read(1024*1024): output.write(chunk)
                output.flush(); os.fsync(output.fileno())
            os.chmod(temporary,member.mode&0o7777)
            os.replace(temporary,target); fd=os.open(target.parent,os.O_DIRECTORY)
            try: os.fsync(fd)
            finally: os.close(fd)
        finally:
            try: os.unlink(temporary)
            except FileNotFoundError: pass
fd=os.open(destination,os.O_DIRECTORY)
try: os.fsync(fd)
finally: os.close(fd)
PY
}
publish_candidate() {
    local stage="$1" final="$2"
    if [[ -e "$final" ]]; then
        [[ -d "$final" && ! -L "$final" ]] || safe_blocked 'candidate path is not a directory'
        if verify_generation_files "$final" >/dev/null 2>&1; then
            candidate_created=0; rm -r -- "$stage"; return
        fi
        mkdir -p -- "$backup_dir"; chmod 700 -- "$backup_dir"
        candidate_quarantine="$backup_dir/$operation_id-generation-$candidate_id"
        [[ ! -e "$candidate_quarantine" && ! -L "$candidate_quarantine" ]] || safe_blocked 'candidate quarantine collision'
        python3 - "$final" "$candidate_quarantine" <<'PY'
import os,sys
from pathlib import Path
source,destination=map(Path,sys.argv[1:])
os.replace(source,destination)
for parent in {source.parent,destination.parent}:
    fd=os.open(parent,os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
PY
    fi
    python3 - "$stage" "$final" <<'PY'
import os,sys
from pathlib import Path
stage,final=map(Path,sys.argv[1:]); os.replace(stage,final)
fd=os.open(final.parent,os.O_DIRECTORY)
try: os.fsync(fd)
finally: os.close(fd)
PY
    candidate_created=1
}
backup_legacy_path() {
    local destination="$1"
    if [[ ! -e "$destination" && ! -L "$destination" ]]; then return; fi
    if [[ -L "$destination" ]]; then
        local resolved link_target expected
        link_target="$(readlink -- "$destination" 2>/dev/null || true)"
        case "$destination" in
            "$current_link")
                [[ "$link_target" == generations/* && "$link_target" != */*/* ]] ||
                    safe_blocked "foreign current symlink: $destination"
                ;;
            *)
                resolved="$(readlink -f -- "$destination" 2>/dev/null || true)"
                [[ "$resolved" == "$generations_dir/"* ]] || safe_blocked "foreign symlink: $destination"
                return
        esac
        resolved="$(readlink -f -- "$destination" 2>/dev/null || true)"
        [[ "$resolved" == "$generations_dir/"* ]] || safe_blocked "foreign symlink: $destination"
        return
    fi
    [[ -f "$destination" ]] || safe_blocked "legacy path is not regular: $destination"
    [[ "$(stat -c '%u' -- "$destination" 2>/dev/null || true)" == "$(id -u)" ]] ||
        safe_blocked "legacy path owner is not trusted: $destination"
    local expected_mode
    case "$destination" in
        "$binary_destination"|"$installer_destination") expected_mode=755 ;;
        *) expected_mode=644 ;;
    esac
    [[ "$(stat -c '%a' -- "$destination")" == "$expected_mode" ]] ||
        safe_blocked "legacy path mode is not trusted: $destination"
    local backup
    backup="$backup_dir/$operation_id-$(basename -- "$destination")"
    [[ ! -e "$backup" && ! -L "$backup" ]] || safe_blocked 'legacy backup collision'
    mkdir -p -- "$backup_dir"; chmod 700 -- "$backup_dir"
    python3 - "$destination" "$backup" <<'PY'
import os, sys
from pathlib import Path
source, destination = map(Path, sys.argv[1:])
os.replace(source, destination)
for parent in {source.parent, destination.parent}:
    fd = os.open(parent, os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
PY
    chmod "$expected_mode" -- "$backup"
    python3 - "$backup_dir" <<'PY'
import os,sys
fd=os.open(sys.argv[1],os.O_DIRECTORY)
try: os.fsync(fd)
finally: os.close(fd)
PY
}
link_entrypoints() {
    mkdir -p -- "$local_bin" "$local_libexec" "$unit_dir"
    atomic_symlink '../share/codex-info/current/codex_info' "$binary_destination"
    atomic_symlink '../share/codex-info/current/run.sh' "$launcher_destination"
    atomic_symlink '../share/codex-info/current/install.sh' "$installer_destination"
    atomic_symlink 'current/manifest.json' "$manifest_destination"
    if [[ "${desired_state-}" == removed ]]; then
        local destination expected
        for destination in "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
            [[ -e "$destination" || -L "$destination" ]] || continue
            expected="../../../.local/share/codex-info/current/$(basename -- "$destination")"
            [[ -L "$destination" && "$(readlink -- "$destination")" == "$expected" ]] ||
                safe_blocked "foreign unit link in removed state: $destination"
            atomic_unlink "$destination"
        done
        return 0
    fi
    atomic_symlink '../../../.local/share/codex-info/current/codex-info.service' "$unit_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-update.service' "$update_service_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-update.timer' "$update_timer_destination"
}
restore_backups() {
    python3 - "$operation_id" "$backup_dir" "$current_link" "$binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$update_service_destination" "$update_timer_destination" <<'PY'
import os,sys
from pathlib import Path
operation,backup_root,*destinations=sys.argv[1:]
backup_root=Path(backup_root)
for destination_name in reversed(destinations):
    destination=Path(destination_name)
    backup=backup_root / (operation + "-" + destination.name)
    if backup.exists() or backup.is_symlink():
        if destination.exists() or destination.is_symlink(): destination.unlink()
        expected=0o755 if destination.name in {"codex_info","codex-info-install.sh"} else 0o644
        if not backup.is_symlink() and (backup.stat().st_mode & 0o7777) != expected:
            raise SystemExit("legacy backup mode changed before restore")
        os.replace(backup,destination)
        if not destination.is_symlink(): os.chmod(destination,expected)
        fd=os.open(destination.parent,os.O_DIRECTORY)
        try: os.fsync(fd)
        finally: os.close(fd)
        fd=os.open(backup.parent,os.O_DIRECTORY)
        try: os.fsync(fd)
        finally: os.close(fd)
PY
}
remove_published_entrypoints() {
    local destination link_target expected
    for destination in "$binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ -L "$destination" ]] || continue
        link_target="$(readlink -- "$destination" 2>/dev/null || true)"
        case "$destination" in
            "$binary_destination") expected='../share/codex-info/current/codex_info' ;;
            "$launcher_destination") expected='../share/codex-info/current/run.sh' ;;
            "$installer_destination") expected='../share/codex-info/current/install.sh' ;;
            "$manifest_destination") expected='current/manifest.json' ;;
            *) expected="../../../.local/share/codex-info/current/$(basename -- "$destination")" ;;
        esac
        [[ "$link_target" == "$expected" ]] || safe_blocked "foreign or noncanonical symlink during rollback: $destination"
        atomic_unlink "$destination"
    done
}
ensure_entrypoints_for_generation() {
    [[ -n "${previous_id-}" ]] || return 0
    local destination expected
    mkdir -p -- "$local_bin" "$local_libexec" "$unit_dir"
    for destination in "$binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ "${desired_state-}" == removed && "$destination" == "$unit_destination" || "${desired_state-}" == removed && "$destination" == "$update_service_destination" || "${desired_state-}" == removed && "$destination" == "$update_timer_destination" ]] && continue
        [[ -e "$destination" || -L "$destination" ]] && continue
        case "$destination" in
            "$binary_destination") expected='../share/codex-info/current/codex_info' ;;
            "$launcher_destination") expected='../share/codex-info/current/run.sh' ;;
            "$installer_destination") expected='../share/codex-info/current/install.sh' ;;
            "$manifest_destination") expected='current/manifest.json' ;;
            *) expected="../../../.local/share/codex-info/current/$(basename -- "$destination")" ;;
        esac
        atomic_symlink "$expected" "$destination"
    done
}

proc_starttime() {
    local pid="$1" line rest; local -a fields
    [[ -r "$proc_root/$pid/stat" ]] || return 1
    line="$(<"$proc_root/$pid/stat")"; rest="${line##*) }"; read -r -a fields <<<"$rest"
    [[ ${#fields[@]} -ge 20 ]] || return 1
    printf '%s\n' "${fields[19]}"
}
socket_pid() {
    python3 - "$proc_root" <<'PY'
import os,pathlib,re,sys
root=pathlib.Path(sys.argv[1]); tcp=root/"net/tcp"
if not tcp.exists(): print(""); raise SystemExit(0)
inodes=set()
for line in tcp.read_text(errors="replace").splitlines()[1:]:
    fields=line.split()
    if len(fields)>9 and fields[1].upper()=="0100007F:2253" and fields[3]=="0A": inodes.add(fields[9])
owners=[]
for proc in root.glob("[0-9]*"):
    fd_dir=proc/"fd"
    if not fd_dir.is_dir(): continue
    try: descriptors=list(fd_dir.iterdir())
    except OSError: continue
    for fd in descriptors:
        try: value=os.readlink(fd)
        except OSError: continue
        match=re.fullmatch(r"socket:\[(\d+)\]",value)
        if match and match.group(1) in inodes: owners.append(proc.name); break
owners=sorted(set(owners))
if len(owners)>1: raise SystemExit("multiple listener owners")
if inodes and not owners: raise SystemExit("listener owner is inaccessible")
print(owners[0] if owners else "")
PY
}
retire_known_unmanaged() {
    local managed_pid="$1" listener_pid
    listener_pid="$(socket_pid)" || safe_blocked 'listener ownership is ambiguous'
    if [[ -z "$listener_pid" || "$listener_pid" == "$managed_pid" ]]; then return 0; fi
    local resolved actual expected generation_path
    resolved="$(readlink -f -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)"
    if [[ "$resolved" == "$binary_destination" ]]; then
        local legacy_info legacy_binary
        legacy_info="$(legacy_flat_record)" || safe_blocked 'legacy listener state is not trusted'
        IFS=$'\t' read -r _ _ _ legacy_binary _ <<<"$legacy_info"
        [[ "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$binary_destination" 2>/dev/null || true)" ]] ||
            safe_blocked 'legacy listener executable identity differs'
        actual="$(sha256sum -- "$resolved" 2>/dev/null | awk '{print $1}' || true)"
        [[ "$actual" == "$legacy_binary" ]] || safe_blocked 'legacy listener digest differs'
        kill -TERM "$listener_pid" 2>/dev/null || safe_blocked 'legacy listener could not be retired'
        local legacy_deadline; legacy_deadline=$(( $(now_unix) + HEALTH_TIMEOUT ))
        while kill -0 "$listener_pid" 2>/dev/null; do
            (( $(now_unix) < legacy_deadline )) || safe_blocked 'legacy listener did not stop in 30s'
            sleep_interval 1
        done
        return 0
    fi
    [[ "$resolved" == "$generations_dir/"*"/codex_info" ]] || safe_blocked 'foreign listener owner is present'
    generation_path="${resolved%/codex_info}"
    [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || safe_blocked 'listener generation path is invalid'
    verify_generation_files "$generation_path" || safe_blocked 'known listener generation is incoherent'
    expected="$(python3 - "$resolved" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib, json, pathlib, re, stat, sys

path = pathlib.Path(sys.argv[1])
schema, product, target, compatibility = sys.argv[2:]
if path.name != "codex_info" or not path.is_file() or path.is_symlink():
    raise SystemExit("listener executable is not a regular generation member")
manifest_path = path.parent / "manifest.json"
try:
    raw = manifest_path.read_bytes()
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate manifest key")
            result[key] = value
        return result
    document = json.loads(raw.decode("utf-8"), object_pairs_hook=pairs)
except Exception as error:
    raise SystemExit(str(error))
required = {"schema", "product", "version", "source_sha", "run_id", "run_attempt",
            "target", "compatibility", "glibc_minimum", "files"}
if not isinstance(document, dict) or set(document) != required:
    raise SystemExit("listener generation manifest keys are invalid")
if (document["schema"] != schema or document["product"] != product or
        document["target"] != target or document["compatibility"] != compatibility):
    raise SystemExit("listener generation manifest identity is invalid")
version = document["version"]
source = document["source_sha"]
if (not isinstance(version, str) or
        not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)", version) or
        not isinstance(source, str) or not re.fullmatch(r"[0-9a-f]{40}", source)):
    raise SystemExit("listener generation version identity is invalid")
manifest_hash = hashlib.sha256(raw).hexdigest()
if path.parent.name != version + "-" + source + "-" + manifest_hash:
    raise SystemExit("listener generation directory identity is invalid")
entries = document["files"]
if not isinstance(entries, list):
    raise SystemExit("listener generation entries are invalid")
binary = [entry for entry in entries if isinstance(entry, dict) and entry.get("path") == "codex_info"]
if len(binary) != 1 or set(binary[0]) != {"path", "size", "sha256", "mode"}:
    raise SystemExit("listener binary manifest entry is invalid")
entry = binary[0]
if (not isinstance(entry["sha256"], str) or
        not re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]) or
        entry["size"] != path.stat().st_size or entry["mode"] != stat.S_IMODE(path.stat().st_mode)):
    raise SystemExit("listener binary manifest identity is invalid")
print(entry["sha256"])
PY
    )"
    actual="$(sha256sum -- "$proc_root/$listener_pid/exe" 2>/dev/null | awk '{print $1}' || true)"
    [[ -n "$actual" && "$actual" == "$expected" ]] || safe_blocked 'known listener identity mismatch'
    kill -TERM "$listener_pid" 2>/dev/null || safe_blocked 'known listener could not be retired'
    local deadline=$(( $(date +%s) + HEALTH_TIMEOUT ))
    while kill -0 "$listener_pid" 2>/dev/null; do
        (( $(date +%s) < deadline )) || safe_blocked 'known listener did not stop in 30s'
        sleep 1
    done
}
preflight_listener_owner() {
    local listener_pid managed_pid=0 resolved generation_path info expected actual
    listener_pid="$(socket_pid)" || safe_blocked 'listener ownership is ambiguous'
    [[ -z "$listener_pid" ]] && return 0
    if probe_active codex-info.service; then managed_pid="$(systemd_pid)"; fi
    [[ "$listener_pid" == "$managed_pid" ]] && return 0
    resolved="$(readlink -f -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)"
    if [[ "$resolved" == "$binary_destination" ]]; then
        info="$(legacy_flat_record)" || safe_blocked 'legacy listener state is not trusted'
        IFS=$'\t' read -r _ _ _ expected _ <<<"$info"
        [[ "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$binary_destination" 2>/dev/null || true)" ]] || safe_blocked 'legacy listener executable identity differs'
    elif [[ "$resolved" == "$generations_dir/"*"/codex_info" ]]; then
        generation_path="${resolved%/codex_info}"
        [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || safe_blocked 'listener generation path is invalid'
        verify_generation_files "$generation_path" || safe_blocked 'known listener generation is incoherent'
        info="$(manifest_record "$generation_path/manifest.json")" || safe_blocked 'known listener manifest is invalid'
        IFS=$'\t' read -r _ _ _ expected <<<"$info"
    else
        safe_blocked 'foreign listener owner is present'
    fi
    actual="$(sha256sum -- "$resolved" 2>/dev/null | awk '{print $1}' || true)"
    [[ -n "$expected" && "$actual" == "$expected" ]] || safe_blocked 'known listener digest differs'
}
guard_control_listener() {
    local listener_pid managed_pid=0
    listener_pid="$(socket_pid)" || safe_blocked 'listener ownership is ambiguous'
    [[ -z "$listener_pid" ]] && return 0
    if probe_active codex-info.service; then managed_pid="$(systemd_pid)"; fi
    [[ "$listener_pid" == "$managed_pid" ]] || safe_blocked 'foreign listener blocks control mutation'
}
recorder_identity_check() {
    local pid="$1" version="$2" source="$3" manifest_hash="$4" lock_path recorder_path data_root
    data_root="$(printenv CODEX_INFO_DATA_DIR || true)"
    [[ -n "$data_root" ]] || data_root="${CODEX_HOME:-$home_dir/.codex}"
    lock_path="$(printenv CODEX_INFO_PROFILE_LOCK || printf '%s/history/usage_record_daemon.lock' "$data_root")"
    recorder_path="$(printenv CODEX_INFO_RECORDER_STATE || printf '%s/history/recorder-state.json' "$data_root")"
    python3 - "$pid" "$version" "$source" "$manifest_hash" "$lock_path" "$recorder_path" "$proc_root" <<'PY'
import json, os, pathlib, re, stat, sys, time
pid_text, version, source, manifest_hash, lock_name, recorder_name, proc_root = sys.argv[1:]
pid = int(pid_text)
def pairs(items):
    result = {}
    for key, value in items:
        if key in result: raise ValueError("duplicate recorder key")
        result[key] = value
    return result
def read_object(path, keys):
    path = pathlib.Path(path)
    if not path.is_file() or path.is_symlink(): raise SystemExit("recorder identity file unavailable")
    metadata = path.stat()
    if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o600: raise SystemExit("recorder identity file is not owner-private")
    try: value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=pairs)
    except Exception as error: raise SystemExit(str(error))
    if not isinstance(value, dict) or set(value) != keys: raise SystemExit("recorder identity schema is invalid")
    return value
lock = read_object(lock_name, {"pid","started_at","starttime_ticks","executable_device","executable_inode","owner_nonce"})
if lock["pid"] != pid or any(isinstance(lock[key], bool) or not isinstance(lock[key], int) or lock[key] <= 0 for key in ("pid","started_at","starttime_ticks","executable_device","executable_inode")):
    raise SystemExit("profile lock owner mismatch")
if not isinstance(lock["owner_nonce"], str) or not re.fullmatch(r"[0-9a-f]{32}", lock["owner_nonce"]): raise SystemExit("profile lock nonce is invalid")
stat_path = pathlib.Path(proc_root) / pid_text / "stat"
try: stat_text = stat_path.read_text(encoding="utf-8")
except OSError: raise SystemExit("profile owner process is unavailable")
try: proc_fields = stat_text.rsplit(") ", 1)[1].split()
except Exception: raise SystemExit("profile owner stat is malformed")
if len(proc_fields) < 20 or proc_fields[19] != str(lock["starttime_ticks"]): raise SystemExit("profile owner starttime mismatch")
try: executable = (pathlib.Path(proc_root) / pid_text / "exe").stat()
except OSError: raise SystemExit("profile owner executable is unavailable")
if executable.st_dev != lock["executable_device"] or executable.st_ino != lock["executable_inode"]: raise SystemExit("profile owner executable identity mismatch")
state = read_object(recorder_name, {"schema","pid","process_starttime","owner_nonce","write_state","partition_id_hash","data_generation","collector_epoch","cycle_seq","last_commit_unix","updated_at_unix"})
if state["schema"] != "codex-info-recorder-state-v1" or state["pid"] != pid or state["process_starttime"] != lock["starttime_ticks"] or state["owner_nonce"] != lock["owner_nonce"]:
    raise SystemExit("recorder owner identity mismatch")
if isinstance(state["updated_at_unix"], bool) or not isinstance(state["updated_at_unix"], int) or state["updated_at_unix"] <= 0: raise SystemExit("recorder updated_at_unix is invalid")
now = int(time.time())
if state["updated_at_unix"] > now + 5 or now - state["updated_at_unix"] > 150: raise SystemExit("recorder heartbeat is stale")
write_state = state["write_state"]
if write_state not in {"idle_no_account","ready","degraded"}: raise SystemExit("recorder write state is invalid")
partition = state["partition_id_hash"]
if partition is not None and (not isinstance(partition, str) or not re.fullmatch(r"[0-9a-f]{64}", partition)): raise SystemExit("recorder partition identity is invalid")
if write_state == "idle_no_account" and any(state[key] is not None for key in ("partition_id_hash","data_generation","collector_epoch","cycle_seq","last_commit_unix")):
    raise SystemExit("idle recorder state is inconsistent")
if write_state == "ready":
    if partition is None or any(state[key] is None for key in ("data_generation","collector_epoch","cycle_seq","last_commit_unix")): raise SystemExit("ready recorder state is incomplete")
    if state["last_commit_unix"] > now + 5 or now - state["last_commit_unix"] > 150: raise SystemExit("recorder commit is stale")
if write_state == "degraded": raise SystemExit("recorder is degraded")
if state["data_generation"] is not None and (isinstance(state["data_generation"], bool) or not isinstance(state["data_generation"], int) or state["data_generation"] <= 0): raise SystemExit("recorder data generation is invalid")
if state["collector_epoch"] is not None and (not isinstance(state["collector_epoch"], str) or not re.fullmatch(r"[0-9a-f]{32}", state["collector_epoch"]) or set(state["collector_epoch"]) == {"0"}): raise SystemExit("recorder collector epoch is invalid")
if state["cycle_seq"] is not None and (isinstance(state["cycle_seq"], bool) or not isinstance(state["cycle_seq"], int) or state["cycle_seq"] <= 0): raise SystemExit("recorder cycle is invalid")
if state["last_commit_unix"] is not None and (isinstance(state["last_commit_unix"], bool) or not isinstance(state["last_commit_unix"], int) or state["last_commit_unix"] <= 0): raise SystemExit("recorder commit is invalid")
PY
}
proc_identity_check() {
    local pid="$1" expected_hash="$2" expected_exe="$3" resolved actual owner expected_stat actual_stat
    resolved="$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)"
    [[ "$resolved" == "$expected_exe" ]] || safe_blocked 'MainPID executable is not current generation'
    expected_stat="$(stat -Lc '%d:%i' -- "$expected_exe" 2>/dev/null || true)"
    actual_stat="$(stat -Lc '%d:%i' -- "$proc_root/$pid/exe" 2>/dev/null || true)"
    [[ -n "$expected_stat" && "$actual_stat" == "$expected_stat" ]] || safe_blocked 'MainPID executable identity differs'
    actual="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
    [[ "$actual" == "$expected_hash" ]] || safe_blocked 'MainPID digest mismatch'
    owner="$(socket_pid)" || safe_blocked 'listener ownership is ambiguous'
    [[ "$owner" == "$pid" ]] || safe_blocked 'listener socket is not owned by MainPID'
}
health_readback() {
    local pid="$1" before="$2" version="$3" source="$4" manifest_hash="$5" binary_hash="$6" response details after after_pid expected_exe health_limit health_now health_remaining readback_deadline
    expected_exe="$generations_dir/$version-$source-$manifest_hash/codex_info"
    proc_identity_check "$pid" "$binary_hash" "$expected_exe"
    health_now="$(now_unix)" || safe_blocked 'health clock is unavailable'
    readback_deadline=$((health_now + HEALTH_TIMEOUT))
    if (( readiness_deadline > 0 && readiness_deadline < readback_deadline )); then
        readback_deadline=$readiness_deadline
    fi
    health_remaining=$((readback_deadline - health_now))
    (( health_remaining > 0 )) || safe_blocked 'health readiness deadline expired'
    health_limit=$health_remaining
    response="$("$CURL_BIN" --fail --silent --show-error --proto '=http' --max-time "$health_limit" "$HEALTH_URL")" || safe_blocked 'health request failed'
    after="$(proc_starttime "$pid" 2>/dev/null || true)"
    [[ -n "$before" && "$after" == "$before" ]] || safe_blocked 'MainPID/starttime changed during health'
    after_pid="$(systemd_pid)"
    [[ "$after_pid" == "$pid" ]] || safe_blocked 'systemd MainPID changed during health'
    python3 - "$response" "$version" "$source" "$manifest_hash" <<'PY'
import json,sys
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate health key")
        result[key]=value
    return result
try: document=json.loads(sys.argv[1],object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
if not isinstance(document,dict): raise SystemExit("health is not an object")
if set(document) != {"api_version","service","product_version"}:
    raise SystemExit("health schema is unknown")
if document["api_version"] != "v1" or document["service"] != "codex-info" or document["product_version"] != sys.argv[2]:
    raise SystemExit("health identity mismatch")
PY
    health_now="$(now_unix)" || safe_blocked 'details clock is unavailable'
    health_limit=$((readback_deadline - health_now))
    (( health_limit > 0 )) || safe_blocked 'details readiness deadline expired'
    details="$("$CURL_BIN" --fail --silent --show-error --proto '=http' --max-time "$health_limit" "$DETAILS_URL")" || safe_blocked 'details request failed'
    python3 -c '
import json,sys
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate details key")
        result[key]=value
    return result
try: document=json.load(sys.stdin,object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
if not isinstance(document,dict): raise SystemExit("details is not an object")
if document.get("state") not in {"ready","auth_required"}:
    raise SystemExit("details is not functionally ready")
observed_at=document.get("observed_at")
if isinstance(observed_at,bool) or not isinstance(observed_at,int) or observed_at <= 0:
    raise SystemExit("details observed_at is invalid")
' <<< "$details"
    recorder_identity_check "$pid" "$version" "$source" "$manifest_hash"
    proc_identity_check "$pid" "$binary_hash" "$expected_exe"
}
verify_runtime() {
    verify_fixed_links
    [[ -L "$current_link" ]] || safe_blocked 'current generation is absent'
    local target generation info version source manifest_hash binary_hash pid before
    target="$(readlink -- "$current_link")"
    [[ "$target" == generations/* && "$target" != */*/* ]] || safe_blocked 'current generation link is invalid'
    generation="${target#generations/}"
    [[ -d "$generations_dir/$generation" && ! -L "$generations_dir/$generation" ]] || safe_blocked 'current generation directory is invalid'
    info="$(manifest_record)"; IFS=$'\t' read -r version source manifest_hash binary_hash <<<"$info"
    [[ "$generation" == "$version-$source-$manifest_hash" ]] || safe_blocked 'generation identity mismatch'
    verify_generation_files "$generations_dir/$generation" || safe_blocked 'generation artifact set is incoherent'
    [[ "$(sha256sum -- "$binary_destination" | awk '{print $1}')" == "$binary_hash" ]] || safe_blocked 'installed binary digest mismatch'
    probe_active codex-info.service || safe_blocked 'managed service is inactive'
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || safe_blocked 'managed service has no MainPID'
    before="$(proc_starttime "$pid" 2>/dev/null || true)"; [[ -n "$before" ]] || safe_blocked 'MainPID starttime unavailable'
    health_readback "$pid" "$before" "$version" "$source" "$manifest_hash" "$binary_hash"
    printf 'ready version=%s source=%s generation=%s pid=%s\n' "$version" "$source" "$generation" "$pid"
}
verify_ui_source() {
    local target generation info version source manifest_hash binary_hash pid expected_exe resolved listener_pid
    verify_fixed_links_local || return 1
    [[ -L "$current_link" ]] || return 1
    target="$(readlink -- "$current_link")"
    [[ "$target" == generations/* && "$target" != */*/* ]] || return 1
    generation="${target#generations/}"
    [[ -d "$generations_dir/$generation" && ! -L "$generations_dir/$generation" ]] || return 1
    info="$(manifest_record)" || return 1
    IFS=$'\t' read -r version source manifest_hash binary_hash <<<"$info"
    [[ "$generation" == "$version-$source-$manifest_hash" ]] || return 1
    verify_generation_files "$generations_dir/$generation" || return 1
    if ! probe_active codex-info.service; then
        listener_pid="$(socket_pid 2>/dev/null || true)"
        [[ -z "$listener_pid" ]]
        return
    fi
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || return 1
    expected_exe="$generations_dir/$generation/codex_info"
    resolved="$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)"
    [[ "$resolved" == "$expected_exe" ]] || return 1
    [[ "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$expected_exe" 2>/dev/null || true)" ]] || return 1
    [[ "$(sha256sum -- "$resolved" 2>/dev/null | awk '{print $1}')" == "$binary_hash" ]] || return 1
    listener_pid="$(socket_pid 2>/dev/null || true)"
    [[ -z "$listener_pid" || "$listener_pid" == "$pid" ]]
}
verify_legacy_runtime() {
    local info version source manifest_hash binary_hash pid before after listener_pid
    info="$(legacy_flat_record)" || return 1
    IFS=$'\t' read -r version source manifest_hash binary_hash _ <<<"$info"
    probe_active codex-info.service || return 1
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || return 1
    [[ "$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)" == "$binary_destination" ]] || return 1
    [[ "$(stat -Lc '%d:%i' -- "$proc_root/$pid/exe" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$binary_destination" 2>/dev/null || true)" ]] || return 1
    [[ "$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}')" == "$binary_hash" ]] || return 1
    listener_pid="$(socket_pid 2>/dev/null || true)"; [[ "$listener_pid" == "$pid" ]] || return 1
    before="$(proc_starttime "$pid" 2>/dev/null || true)"; [[ -n "$before" ]] || return 1
    python3 - "$CURL_BIN" "$HEALTH_URL" "$HEALTH_TIMEOUT" "$version" <<'PY'
import json,subprocess,sys
curl,url,timeout,version=sys.argv[1:]
raw=subprocess.check_output([curl,"--fail","--silent","--show-error","--proto","=http","--max-time",timeout,url],text=True)
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate health key")
        result[key]=value
    return result
document=json.loads(raw,object_pairs_hook=pairs)
if not isinstance(document,dict) or set(document)!={"api_version","service","product_version"}:
    raise SystemExit("legacy health schema is invalid")
if document!={"api_version":"v1","service":"codex-info","product_version":version}:
    raise SystemExit("legacy health identity is invalid")
PY
    after="$(proc_starttime "$pid" 2>/dev/null || true)"; [[ "$after" == "$before" ]] || return 1
    [[ "$(systemd_pid)" == "$pid" ]] || return 1
    recorder_identity_check "$pid" "$version" "$source" "$manifest_hash"
}
verify_legacy_terminal() {
    legacy_flat_record >/dev/null || return 1
    [[ ! -L "$current_link" ]] || return 1
    if [[ "$desired_state" == running ]]; then
        verify_legacy_runtime
    else
        ! probe_active codex-info.service || return 1
        [[ -z "$(socket_pid 2>/dev/null || true)" ]]
    fi
}
rearm_update_timer() {
    if probe_active codex-info-update.timer; then
        systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || return 1
        wait_inactive codex-info-update.timer || return 1
    fi
    systemctl_user enable codex-info-update.timer >/dev/null 2>&1 || return 1
    systemctl_user start --no-block codex-info-update.timer >/dev/null 2>&1 || return 1
}
reset_failed_main() {
    systemctl_user reset-failed codex-info.service >/dev/null 2>&1
}
repair_known_managed_runtime() {
    local pid resolved expected generation_path current_path current_info expected_hash actual_hash
    probe_active codex-info.service || return 0
    [[ "$TRIGGER" != startup ]] || return 0
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || safe_blocked 'managed service has no MainPID'
    current_path="$(readlink -f -- "$current_link" 2>/dev/null || true)"
    [[ "$current_path" == "$generations_dir/"* ]] || safe_blocked 'current generation path is unavailable for runtime repair'
    expected="$current_path/codex_info"
    resolved="$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)"
    if [[ "$resolved" == "$expected" ]]; then
        current_info="$(manifest_record "$current_path/manifest.json")" || safe_blocked 'current generation manifest is unavailable for runtime repair'
        IFS=$'\t' read -r _ _ _ expected_hash <<<"$current_info"
        actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        [[ "$actual_hash" == "$expected_hash" ]] || safe_blocked 'managed current executable digest differs'
        return 0
    fi
    if [[ "$resolved" == "$generations_dir/"*"/codex_info" ]]; then
        generation_path="${resolved%/codex_info}"
        [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || safe_blocked 'managed runtime generation path is invalid'
        verify_generation_files "$generation_path" || safe_blocked 'known managed runtime generation is incoherent'
        actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        current_info="$(manifest_record "$generation_path/manifest.json")" || safe_blocked 'known managed runtime manifest is unavailable'
        IFS=$'\t' read -r _ _ _ expected_hash <<<"$current_info"
        [[ "$actual_hash" == "$expected_hash" ]] || safe_blocked 'known managed runtime digest differs'
    elif [[ "$resolved" == "$binary_destination" ]]; then
        legacy_flat_record >/dev/null || safe_blocked 'legacy managed runtime is not trusted'
        actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        expected_hash="$(sha256sum -- "$binary_destination" 2>/dev/null | awk '{print $1}' || true)"
        [[ -n "$actual_hash" && "$actual_hash" == "$expected_hash" ]] || safe_blocked 'legacy managed runtime digest differs'
    else
        safe_blocked 'foreign managed service executable blocks runtime repair'
    fi
    reset_failed_main || safe_blocked 'could not reset failed managed service for runtime repair'
    systemctl_user restart --no-block codex-info.service >/dev/null 2>&1 || safe_blocked 'could not restart managed service for runtime repair'
}
verify_nonrunning_terminal() {
    local desired="$1" listener_pid
    if [[ "$desired" == removed ]]; then
        unit_inactive_or_absent codex-info.service || return 1
    else
        probe_active codex-info.service && return 1
    fi
    listener_pid="$(socket_pid 2>/dev/null || true)"
    [[ -z "$listener_pid" ]] || return 1
    verify_local_generation || return 1
    case "$desired" in
        stopped)
            probe_enabled codex-info.service || return 1
            probe_enabled codex-info-update.timer || return 1
            probe_active codex-info-update.timer || return 1
            ;;
        disabled)
            probe_enabled codex-info.service && return 1
            probe_enabled codex-info-update.timer && return 1
            probe_active codex-info-update.timer && return 1
            verify_fixed_links_local || return 1
            ;;
        removed)
            unit_inactive_or_absent codex-info-update.timer || return 1
            unit_inactive_or_absent codex-info-update.service || return 1
            [[ ! -e "$unit_destination" && ! -L "$unit_destination" ]] || return 1
            [[ ! -e "$update_service_destination" && ! -L "$update_service_destination" ]] || return 1
            [[ ! -e "$update_timer_destination" && ! -L "$update_timer_destination" ]] || return 1
            [[ -L "$binary_destination" && -L "$launcher_destination" && -L "$installer_destination" && -L "$manifest_destination" ]] || return 1
            ;;
        *) return 1 ;;
    esac
}
unit_inactive_or_absent() {
    local unit="$1" status=0
    systemctl_user is-active --quiet "$unit" >/dev/null 2>&1 || status="$?"
    case "$status" in
        3|4|5) return 0 ;;
        0) return 1 ;;
        *) return 1 ;;
    esac
}
capture_runtime_state() {
    main_enabled=0; main_active=0; timer_enabled=0; timer_active=0
    probe_enabled codex-info.service && main_enabled=1 || true; probe_active codex-info.service && main_active=1 || true
    probe_enabled codex-info-update.timer && timer_enabled=1 || true; probe_active codex-info-update.timer && timer_active=1 || true
}
enforce_desired_state() {
    if [[ "$desired_state" != running && "$main_active" == 1 ]]; then
        systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || return 1
        wait_inactive codex-info.service || return 1
        main_active=0
    fi
    if [[ "$desired_state" == disabled ]]; then
        if [[ "$timer_active" == 1 ]]; then
            systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || return 1
            wait_inactive codex-info-update.timer || return 1
            timer_active=0
        fi
        systemctl_user disable codex-info-update.timer >/dev/null 2>&1 || return 1
        timer_enabled=0
    fi
}
restore_runtime_state() {
    local failed=0
    if ((timer_enabled)); then systemctl_user enable codex-info-update.timer >/dev/null 2>&1 || failed=1; else systemctl_user disable codex-info-update.timer >/dev/null 2>&1 || failed=1; fi
    if ((timer_active)); then systemctl_user start --no-block codex-info-update.timer >/dev/null 2>&1 || failed=1; else systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || failed=1; wait_inactive codex-info-update.timer || failed=1; fi
    if ((main_enabled)); then systemctl_user enable codex-info.service >/dev/null 2>&1 || failed=1; else systemctl_user disable codex-info.service >/dev/null 2>&1 || failed=1; fi
    if ((main_active)); then systemctl_user restart --no-block codex-info.service >/dev/null 2>&1 || failed=1; else systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || failed=1; wait_inactive codex-info.service || failed=1; fi
    return "$failed"
}
rollback_transaction() {
    local previous="$1" reason="$2" ok=1 saved_deadline="$operation_deadline" rollback_now rollback_deadline
    rollback_now="$(now_unix)" || safe_blocked 'rollback clock is unavailable'
    rollback_deadline=$((rollback_now + ROLLBACK_TIMEOUT))
    if (( saved_deadline > 0 && saved_deadline < rollback_deadline )); then rollback_deadline=$saved_deadline; fi
    operation_deadline=$rollback_deadline
    if [[ -n "$previous" ]]; then atomic_symlink "generations/$previous" "$current_link" || ok=0; else atomic_unlink "$current_link" || ok=0; fi
    remove_published_entrypoints || ok=0
    restore_backups || ok=0
    previous_id="$previous"
    ensure_entrypoints_for_generation || ok=0
    write_journal rollback_switched "$reason" || ok=0
    systemctl_user daemon-reload >/dev/null 2>&1 || ok=0; restore_runtime_state || ok=0
    if ((ok)) && [[ "$desired_state" != running && "$main_active" == 1 ]]; then
        systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || ok=0
        wait_inactive codex-info.service || ok=0
        main_active=0
    fi
    if ((ok)); then
        if [[ -n "$previous" ]]; then
            if [[ "$desired_state" == running ]]; then
                if ! ((main_active)); then
                    systemctl_user enable codex-info.service >/dev/null 2>&1 || ok=0
                    systemctl_user start --no-block codex-info.service >/dev/null 2>&1 || ok=0
                fi
                ((ok)) && wait_runtime_ready || ok=0
            elif ((main_active)); then
                wait_runtime_ready || ok=0
            else
                [[ "$(current_generation)" == "$previous" ]] || ok=0
            fi
        elif (( previous_flat )) || legacy_flat_present; then
            verify_legacy_terminal || ok=0
        else
            [[ ! -L "$current_link" ]] || ok=0
            [[ -z "$(socket_pid 2>/dev/null || true)" ]] || ok=0
        fi
    fi
    if ((ok)); then write_journal rollback_verified "$reason"; write_journal committed rolled_back; fi
    operation_deadline=$saved_deadline
    ((ok)) || safe_blocked "rollback could not be verified within $ROLLBACK_TIMEOUT seconds"
}
resume_transaction() {
    [[ -f "$transaction" ]] || return 0
    read_journal; [[ "$journal_phase" != committed ]] || return 0
    journal_owner_stale || safe_blocked 'transaction journal owner is still live'
    operation_id="$journal_operation_id"; candidate_id="$journal_candidate_id"; previous_id="$journal_previous_id"; desired_state="$journal_desired"
    candidate_quarantine="$backup_dir/$operation_id-generation-$candidate_id"
    [[ -e "$candidate_quarantine" || -L "$candidate_quarantine" ]] || candidate_quarantine=
    journal_owner_pid="$$"; journal_owner_starttime="$(owner_starttime)" || safe_blocked 'journal resume owner starttime is unavailable'; journal_boot_id="$(boot_id)"
    require_user_manager
    capture_runtime_state
    if [[ "$journal_phase" == current_switched || "$journal_phase" == activation_requested ]]; then
        if [[ "$(current_generation)" == "$candidate_id" ]] && verify_local_generation >/dev/null 2>&1 &&
            { [[ "$desired_state" != running ]] || verify_runtime >/dev/null 2>&1; }; then
            write_journal candidate_verified resumed-live-owner
            write_journal committed resumed
            return 0
        fi
    elif [[ "$journal_phase" == rollback_switched ]]; then
        if [[ -n "$previous_id" ]]; then
            if [[ "$(current_generation)" == "$previous_id" ]] && verify_local_generation >/dev/null 2>&1 &&
                { [[ "$desired_state" != running ]] || verify_runtime >/dev/null 2>&1; }; then
                write_journal rollback_verified resumed-rollback
                write_journal committed resumed
                return 0
            fi
        elif legacy_flat_present && verify_legacy_terminal >/dev/null 2>&1; then
            write_journal rollback_verified resumed-legacy-rollback
            write_journal committed resumed
            return 0
        elif [[ ! -L "$current_link" ]] && [[ -z "$(socket_pid 2>/dev/null || true)" ]]; then
            write_journal rollback_verified resumed-empty-rollback
            write_journal committed resumed
            return 0
        fi
    fi
    rollback_transaction "$previous_id" 'resumed rollback'
}
activate_candidate() {
    systemctl_user daemon-reload >/dev/null 2>&1 || return 1
    [[ "$desired_state" == running ]] || return 0
    reset_failed_main || return 1
    systemctl_user enable codex-info.service >/dev/null 2>&1 || return 1
    rearm_update_timer || return 1
    [[ "$TRIGGER" == startup ]] && return
    if ((main_active)); then systemctl_user restart --no-block codex-info.service >/dev/null 2>&1 || return 1
    else systemctl_user start --no-block codex-info.service >/dev/null 2>&1 || return 1; fi
}
verify_candidate() {
    verify_local_generation; [[ "$(current_generation)" == "$candidate_id" ]] || safe_blocked 'candidate is not current'
    if [[ "$desired_state" == running && "$TRIGGER" != startup ]]; then wait_runtime_ready; fi
}
perform_install() {
    local validation bundle_version source_hash manifest_hash binary_hash
    validation="$(validate_bundle "$ARCHIVE" "$MANIFEST")" || die 'candidate validation failed before mutation'
    check_glibc_compatibility "$MANIFEST" || die 'candidate glibc compatibility check failed'
    IFS=$'\t' read -r bundle_version source_hash manifest_hash binary_hash <<<"$validation"
    candidate_id="$bundle_version-$source_hash-$manifest_hash"; previous_id="$(current_generation)"; operation_id="$(new_operation_id)"
    previous_flat=0
    if [[ -z "$previous_id" ]] && legacy_flat_present; then
        local legacy_info
        legacy_info="$(legacy_flat_record)" || safe_blocked 'flat predecessor is not trusted'
        previous_flat=1
    fi
    journal_owner_pid=""; journal_owner_starttime=""; journal_boot_id=""
    load_control_state; require_user_manager; capture_runtime_state
    local managed_pid=0; probe_active codex-info.service && managed_pid="$(systemd_pid)" || true
    # The durable pre-state marker must exist before the first stop or TERM.
    # This makes a crash after owner retirement resumable instead of leaving a
    # listener-less flat installation with no recovery authority.
    write_journal prepared
    enforce_desired_state || safe_blocked 'could not enforce desired runtime state'
    retire_known_unmanaged "$managed_pid"
    for destination in "$current_link" "$binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$update_service_destination" "$update_timer_destination"; do
        backup_legacy_path "$destination"
        # Persist each legacy move, so a crash between two moves can always
        # replay the same operation without guessing from mtimes.
        write_journal prepared
    done
    write_journal legacy_backed_up; link_entrypoints; write_journal entrypoints_linked
    candidate_stage="$(mktemp -d "$generations_dir/.candidate.XXXXXX")"; extract_candidate "$ARCHIVE" "$candidate_stage"
    candidate_final="$generations_dir/$candidate_id"; publish_candidate "$candidate_stage" "$candidate_final"; candidate_stage=
    if ! verify_generation_files "$candidate_final"; then
        (( candidate_created )) && rm -r -- "$candidate_final"
        safe_blocked 'published candidate artifacts are incoherent'
    fi
    write_journal candidate_published; atomic_symlink "generations/$candidate_id" "$current_link"; write_journal current_switched
    if ! activate_candidate; then rollback_transaction "$previous_id" 'candidate activation failed'; die 'candidate activation failed; previous generation restored'; fi
    write_journal activation_requested
    if ! verify_candidate; then rollback_transaction "$previous_id" 'candidate verification failed'; die 'candidate verification failed; previous generation restored'; fi
    if ! write_control_state "$desired_state"; then
        rollback_transaction "$previous_id" 'control state publication failed'
        die 'control state publication failed; previous generation restored'
    fi
    write_journal candidate_verified; write_journal committed installed
    ((QUIET)) || printf 'installed generation=%s\n' "$candidate_id"
}
update_failure_with_fallback() {
    local reason="$1" current_id fallback_ok=0
    current_id="$(current_generation 2>/dev/null || true)"
    if [[ -n "$update_root" && -d "$update_root" && ! -L "$update_root" ]]; then
        rm -r -- "$update_root"
        update_root=
    fi
    if [[ "$desired_state" == running ]]; then
        if [[ -n "$current_id" ]]; then
            verify_runtime >/dev/null 2>&1 && fallback_ok=1 || true
        elif legacy_flat_present; then
            verify_legacy_runtime >/dev/null 2>&1 && fallback_ok=1 || true
        fi
    elif [[ "$desired_state" == stopped || "$desired_state" == disabled || "$desired_state" == removed ]]; then
        verify_nonrunning_terminal "$desired_state" >/dev/null 2>&1 && fallback_ok=1 || true
    fi
    if (( fallback_ok )); then
        die "$reason; existing installation remains coherent"
    fi
    safe_blocked "$reason; existing installation could not be verified"
}
run_update() {
    local start update_deadline releases selection info local_coherent=0 discovery_limit
    [[ ! -f "$transaction" ]] || resume_transaction
    start="$(now_unix)" || safe_blocked 'update clock is unavailable'; [[ "$TRIGGER" == timer ]] && update_deadline=$((start+TIMER_TIMEOUT)) || update_deadline=$((start+MANUAL_TIMEOUT))
    require_user_manager; load_control_state
    local current_id current_manifest_path
    current_id="$(current_generation)" || safe_blocked 'installed current generation is not coherent'
    if [[ -n "$current_id" ]]; then
        current_manifest_path="$generations_dir/$current_id/manifest.json"
        info="$(manifest_record "$current_manifest_path")" || safe_blocked 'installed manifest is not coherent'
    else
        # The only accepted pre-generation state is the verified predecessor
        # flat layout. It is read-only here; perform_install writes the
        # prepared journal before changing any of its members.
        info="$(legacy_flat_record)" || safe_blocked 'installed generation is absent and legacy flat state is not trusted'
        current_manifest_path="$manifest_destination"
    fi
    # Classify any listener before network discovery. Unknown/foreign owners
    # must terminate SAFE_BLOCKED without download or publication mutation;
    # only an exact known generation/legacy owner may be retired later after a
    # durable prepared journal exists.
    preflight_listener_owner
    IFS=$'\t' read -r installed_version _ installed_manifest_hash _ <<<"$info"
    if [[ -n "$current_id" && "$desired_state" == running ]] && ! verify_fixed_links_local; then
        # --remove retains the verified generation and payload links but
        # removes only the three stable unit links.  An explicit subsequent
        # --start repairs those links from the verified current generation
        # before asking systemd to activate it.
        verify_generation_files "$generations_dir/$current_id" || safe_blocked 'retained generation is incoherent during unit-link repair'
        previous_id="$current_id"
        ensure_entrypoints_for_generation || safe_blocked 'retained generation entrypoints could not be repaired'
        systemctl_user daemon-reload >/dev/null 2>&1 || safe_blocked 'daemon-reload failed during retained unit-link repair'
    fi
    verify_local_generation >/dev/null 2>&1 && local_coherent=1 || true
    update_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-info-update.XXXXXX")"; releases="$update_root/releases.json"; selection="$update_root/selection"
    command -v "$CURL_BIN" >/dev/null 2>&1 || update_failure_with_fallback "$CURL_BIN is required"
    discovery_limit=30
    if (( operation_deadline > 0 )); then discovery_limit="$(deadline_timeout 30)" || update_failure_with_fallback 'update overall timeout exceeded before discovery'; fi
    if ! "$CURL_BIN" --fail --silent --show-error --proto '=https' --max-time "$discovery_limit" --header 'Accept: application/vnd.github+json' --header 'X-GitHub-Api-Version: 2022-11-28' "$RELEASES_URL" >"$releases"; then
        update_failure_with_fallback 'public release discovery failed'
    fi
    local manifest_size; manifest_size="$(stat -c %s -- "$current_manifest_path")"
    if ! select_release "$releases" "$installed_version" "$installed_manifest_hash" "$manifest_size" "$local_coherent" >"$selection"; then
        update_failure_with_fallback 'release selection failed'
    fi
    local state newest; IFS=$'\t' read -r state newest < "$selection"
    if [[ "$state" == no-update ]]; then
        verify_local_generation || safe_blocked 'no-update local generation is incoherent'
        if [[ "$desired_state" == running ]]; then
            if ! probe_active codex-info.service; then
                retire_known_unmanaged 0
                if [[ "$TRIGGER" == startup ]]; then
                    rm -r -- "$update_root"; update_root=; ((QUIET)) || printf 'no update current=%s newest=%s\n' "$installed_version" "$newest"; return
                fi
                reset_failed_main || safe_blocked 'could not reset failed managed service'
                systemctl_user enable codex-info.service >/dev/null 2>&1 || safe_blocked 'could not enable managed service'
                systemctl_user start --no-block codex-info.service >/dev/null 2>&1 || safe_blocked 'could not start managed service'
            else
                repair_known_managed_runtime
            fi
            wait_runtime_ready || safe_blocked 'no-update managed runtime is not healthy'
        fi
        if [[ "$desired_state" != disabled && "$desired_state" != removed ]]; then
            rearm_update_timer || safe_blocked 'update timer could not be rearmed'
        fi
        rm -r -- "$update_root"; update_root=; ((QUIET)) || printf 'no update current=%s newest=%s\n' "$installed_version" "$newest"; return
    fi
    [[ "$state" == update ]] || die 'release selection returned unknown state'
    (( $(now_unix) <= update_deadline )) || update_failure_with_fallback 'update overall timeout exceeded before download'
    local archive_name archive_url archive_size archive_digest checksum_name checksum_url checksum_size checksum_digest manifest_name manifest_url manifest_size manifest_digest
    IFS=$'\t' read -r archive_name archive_url archive_size archive_digest < <(sed -n '2p' "$selection")
    IFS=$'\t' read -r checksum_name checksum_url checksum_size checksum_digest < <(sed -n '3p' "$selection")
    IFS=$'\t' read -r manifest_name manifest_url manifest_size manifest_digest < <(sed -n '4p' "$selection")
    local archive_path="$update_root/$archive_name" checksum_path="$update_root/$checksum_name" manifest_path="$update_root/$manifest_name"
    download_asset "$archive_url" "$archive_path" "$archive_size" "$archive_digest" || update_failure_with_fallback 'release archive download failed'
    download_asset "$checksum_url" "$checksum_path" "$checksum_size" "$checksum_digest" || update_failure_with_fallback 'release checksum download failed'
    download_asset "$manifest_url" "$manifest_path" "$manifest_size" "$manifest_digest" || update_failure_with_fallback 'release manifest download failed'
    (( $(now_unix) <= update_deadline )) || update_failure_with_fallback 'update overall timeout exceeded before candidate installation'
    local child_limit
    child_limit="$(deadline_timeout "$MANUAL_TIMEOUT")" || update_failure_with_fallback 'update overall timeout exceeded before candidate installation'
    if ! CODEX_INFO_INTERNAL_TRIGGER="$TRIGGER" CODEX_INFO_DEADLINE="$update_deadline" CODEX_INFO_INSTALL_LOCKED=1 \
        timeout --foreground "$child_limit" "$0" --bundle "$archive_path" --manifest "$manifest_path" --sha256 "$checksum_path"; then
        update_failure_with_fallback 'candidate installation failed'
    fi
    (( $(now_unix) <= update_deadline )) || update_failure_with_fallback 'update overall timeout exceeded after candidate installation'
    rm -r -- "$update_root"; update_root=; ((QUIET)) || printf 'updated from=%s to=%s\n' "$installed_version" "$newest"
}
select_release() {
    local releases="$1" current="$2" current_hash="$3" current_size="$4" local_coherent="${5:-0}"
    python3 - "$releases" "$current" "$current_hash" "$current_size" "$local_coherent" "$REPOSITORY" "$TARGET" <<'PY'
import json,pathlib,re,sys
release_path,current_text,current_hash,current_size,local_coherent,repository,target=sys.argv[1:]
def reject(message): raise SystemExit("release metadata validation failed: "+message)
def pairs(items):
    result={}
    for key,value in items:
        if key in result: reject("duplicate JSON key")
        result[key]=value
    return result
try: releases=json.loads(pathlib.Path(release_path).read_text(encoding="utf-8"),object_pairs_hook=pairs)
except Exception as error: reject(str(error))
if not isinstance(releases,list): reject("response is not an array")
if len(releases)==100: reject("100-entry response is pagination-ambiguous")
if not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",current_text): reject("installed version invalid")
current=tuple(map(int,current_text.split("."))); stable=[]; tags=set()
for release in releases:
    if not isinstance(release,dict) or type(release.get("draft")) is not bool or type(release.get("prerelease")) is not bool: reject("release identity malformed")
    tag=release.get("tag_name")
    if not isinstance(tag,str) or tag in tags: reject("release tag malformed or duplicate")
    tags.add(tag); match=re.fullmatch(r"windows-v((?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*))",tag)
    if release["draft"] or release["prerelease"]: continue
    if match is None: reject("stable tag malformed")
    text=match.group(1); stable.append((tuple(map(int,text.split("."))),text,release))
if not stable: print("no-update",current_text,sep="\t"); raise SystemExit(0)
stable.sort(key=lambda item:item[0],reverse=True)
if len(stable)>1 and stable[0][0]==stable[1][0]: reject("duplicate stable version")
newest,newest_text,release=stable[0]
if not isinstance(release.get("published_at"),str) or not release["published_at"]: reject("stable release unpublished")
assets=release.get("assets")
if not isinstance(assets,list) or len(assets)!=5: reject("asset count is not exactly five")
by_name={}
for asset in assets:
    if not isinstance(asset,dict): reject("asset is not an object")
    name,url,state,size,digest=(asset.get(key) for key in ("name","browser_download_url","state","size","digest"))
    if (not isinstance(name,str) or name in by_name or not isinstance(url,str) or state!="uploaded" or isinstance(size,bool) or not isinstance(size,int) or size<=0 or not isinstance(digest,str) or not re.fullmatch(r"sha256:[0-9a-f]{64}",digest)): reject("asset identity malformed")
    by_name[name]=(url,size,digest)
archive_name=f"codex-info-{newest_text}-{target}.tar.gz"; linux=(archive_name,archive_name+".sha256",archive_name[:-7]+".manifest.json")
expected=set(linux+("CodexInfo.WindowsClient.Setup.exe","CodexInfo.WindowsClient.update.json"))
if set(by_name)!=expected: reject("asset names are not exact canonical five")
for name in expected:
    if by_name[name][0]!=f"https://github.com/{repository}/releases/download/windows-v{newest_text}/{name}": reject("asset URL is not canonical")
manifest_size,manifest_digest=by_name[linux[2]][1:]
needs=(newest>current or (newest==current and not (manifest_digest=="sha256:"+current_hash and manifest_size==int(current_size))) or local_coherent!="1")
if newest<current or not needs: print("no-update",newest_text,sep="\t"); raise SystemExit(0)
print("update",newest_text,sep="\t")
for name in linux:
    url,size,digest=by_name[name]; print(name,url,size,digest,sep="\t")
PY
}
download_asset() {
    local url="$1" destination="$2" size="$3" digest="$4" effective download_limit
    [[ "$url" == https://github.com/$REPOSITORY/releases/download/*/* ]] || return 1
    download_limit=300
    if (( operation_deadline > 0 )); then
        download_limit="$(deadline_timeout 300)" || return 1
    fi
    effective="$("$CURL_BIN" --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --max-redirs 3 --max-time "$download_limit" --output "$destination" --write-out '%{url_effective}' "$url")" || return 1
    case "$effective" in "$url"|https://release-assets.githubusercontent.com/*) ;; *) return 1 ;; esac
    [[ -f "$destination" && ! -L "$destination" ]] || return 1
    [[ "$size" =~ ^[1-9][0-9]*$ && "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || return 1
    [[ "$(stat -c %s -- "$destination")" == "$size" ]] || return 1
    [[ "sha256:$(sha256sum -- "$destination" | awk '{print $1}')" == "$digest" ]] || return 1
}

readonly_transaction_check() {
    if [[ -e "$transaction" || -L "$transaction" ]]; then
        read_journal
        [[ "$journal_phase" == committed ]] && return 0
        if [[ "$ACTION" == startup-condition ]] && transaction_startup_authorized; then
            return 0
        fi
        safe_blocked 'transaction journal requires a mutating reconcile'
    fi
}

# Read-only actions intentionally run before all mutating initialization: no
# state directories, lock file, journal replay, or control-state write is
# permitted for status/readback.
if [[ "$ACTION" == startup-condition ]]; then
    readonly_transaction_check
    load_control_state
    [[ "$desired_state" == running ]] || exit 1
    verify_local_generation >/dev/null 2>&1 || exit 1
    exit 0
fi
if [[ "$ACTION" == verify ]]; then
    require_user_manager
    readonly_transaction_check
    verify_runtime
    exit
fi
if [[ "$ACTION" == verify-ui ]]; then
    require_user_manager
    readonly_transaction_check
    load_control_state
    [[ "$desired_state" != removed ]] || safe_blocked 'UI is unavailable after removal'
    verify_ui_source || safe_blocked 'verified UI generation or owner is unavailable'
    exit
fi
if [[ "$ACTION" == status ]]; then
    require_user_manager
    readonly_transaction_check
    load_control_state; printf 'desired_state=%s boot_id=%s\n' "$desired_state" "$state_boot_id"
    if [[ -L "$current_link" ]]; then
        info="$(manifest_record)" || safe_blocked 'status found invalid generation'
        verify_local_generation || safe_blocked 'status found incoherent local generation'
        IFS=$'\t' read -r version source manifest_hash binary_hash <<<"$info"
        printf 'generation=%s version=%s source=%s manifest_sha256=%s\n' "$(current_generation)" "$version" "$source" "$manifest_hash"
    else
        safe_blocked 'status found no installed generation'
    fi
    if [[ "$desired_state" == running ]]; then
        verify_runtime
    else
        verify_nonrunning_terminal "$desired_state" || safe_blocked 'status non-running terminal is not coherent'
        printf 'status=not-running-by-request\n'
    fi
    exit
fi

initialize_mutating_action

# Every mutating operation settles an interrupted publication under the same
# nonblocking L1 lock. A committed journal is durable history and requires no
# replay.
if (( ! lock_bypassed )) && [[ -f "$transaction" ]]; then
    read_journal
    if [[ "$journal_phase" != committed ]]; then
        resume_transaction
    fi
fi

if [[ "$ACTION" == start ]]; then
    load_control_state; require_user_manager
    # An explicit start supersedes a same-boot --stop intent.  Persist this
    # desired-state transition before resolving so the shared installer keeps
    # the service running even when the resolver selects an equal generation.
    write_control_state running
    run_update
    systemctl_user enable codex-info.service >/dev/null 2>&1 || safe_blocked 'could not enable managed service'
    rearm_update_timer || safe_blocked 'could not enable update timer'
    if ! probe_active codex-info.service; then
        systemctl_user start --no-block codex-info.service >/dev/null 2>&1 || safe_blocked 'could not start managed service'
    fi
    wait_runtime_ready || safe_blocked 'managed runtime is not healthy after start'
    printf 'started codex-info.service\n'
    exit
fi
if [[ "$ACTION" == stop ]]; then
    load_control_state; require_user_manager; guard_control_listener
    systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || die 'could not stop service'
    wait_inactive codex-info.service || safe_blocked 'managed service did not stop within 20s'
    systemctl_user enable codex-info.service >/dev/null 2>&1 || die 'could not keep service enabled'
    rearm_update_timer || safe_blocked 'update timer could not remain active after stop'
    desired_state=stopped
    verify_nonrunning_terminal stopped || safe_blocked 'stopped terminal could not be verified'
    write_control_state stopped
    load_control_state; verify_nonrunning_terminal stopped || safe_blocked 'stopped control state readback failed'
    printf 'stopped codex-info.service (timer remains enabled)\n'; exit
fi
if [[ "$ACTION" == disable ]]; then
    load_control_state; require_user_manager; guard_control_listener
    systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || die 'could not stop service'
    wait_inactive codex-info.service || safe_blocked 'managed service did not stop within 20s'
    systemctl_user disable codex-info.service >/dev/null 2>&1 || die 'could not disable service'
    systemctl_stop_user disable --now codex-info-update.timer >/dev/null 2>&1 || die 'could not disable timer'
    wait_inactive codex-info-update.timer || safe_blocked 'update timer did not stop within 20s'
    desired_state=disabled
    verify_nonrunning_terminal disabled || safe_blocked 'disabled terminal could not be verified'
    write_control_state disabled
    load_control_state; verify_nonrunning_terminal disabled || safe_blocked 'disabled control state readback failed'
    printf 'disabled autostart (unit files retained)\n'; exit
fi
if [[ "$ACTION" == remove ]]; then
    load_control_state; require_user_manager; guard_control_listener
    systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || die 'could not stop service'
    wait_inactive codex-info.service || safe_blocked 'managed service did not stop within 20s'
    systemctl_user disable codex-info.service >/dev/null 2>&1 || die 'could not disable service'
    systemctl_stop_user disable --now codex-info-update.timer >/dev/null 2>&1 || die 'could not disable timer'
    wait_inactive codex-info-update.timer || safe_blocked 'update timer did not stop within 20s'
    systemctl_stop_user stop --no-block codex-info-update.service >/dev/null 2>&1 || die 'could not stop update service'
    wait_inactive codex-info-update.service || safe_blocked 'update service did not stop within 20s'
    for destination in "$unit_destination" "$update_service_destination" "$update_timer_destination"; do [[ -L "$destination" ]] || safe_blocked "refusing to remove non-symlink unit: $destination"; done
    atomic_unlink "$unit_destination"; atomic_unlink "$update_service_destination"; atomic_unlink "$update_timer_destination"; systemctl_user daemon-reload >/dev/null 2>&1 || die 'daemon-reload failed during remove'
    desired_state=removed
    verify_nonrunning_terminal removed || safe_blocked 'removed terminal could not be verified'
    write_control_state removed
    load_control_state; verify_nonrunning_terminal removed || safe_blocked 'removed control state readback failed'
    printf 'removed stable unit links (payload and profile retained)\n'; exit
fi
if [[ "$ACTION" == startup ]]; then
    TRIGGER=startup
    if (( lock_bypassed )); then
        [[ -f "$transaction" ]] || safe_blocked 'startup reconcile observed a busy installer without a journal'
        read_journal
        [[ "$journal_phase" != committed ]] ||
            safe_blocked 'startup reconcile observed an unsettled publication phase'
        transaction_startup_authorized || safe_blocked 'startup reconcile could not verify live publication owner'
        expected_startup_generation="$(transaction_startup_generation)" ||
            safe_blocked 'startup reconcile observed an unsupported publication phase'
        ((QUIET)) || printf 'startup reconcile observed live publication generation=%s\n' "$expected_startup_generation"
        exit
    fi
    load_control_state
    [[ "$desired_state" == running ]] || { ((QUIET)) || printf 'startup reconcile preserved desired_state=%s\n' "$desired_state"; exit; }
    [[ -L "$current_link" ]] || safe_blocked 'startup reconcile has no installed generation'
    run_update
    if probe_active codex-info.service; then verify_runtime; else ((QUIET)) || printf 'startup reconcile complete; service activation is pending\n'; fi
    exit
fi
if [[ "$ACTION" == update || "$ACTION" == timer-update ]]; then run_update; exit; fi

[[ -n "$ARCHIVE" ]] || die '--bundle ARCHIVE is required'
archive_dir="$(cd -- "$(dirname -- "$ARCHIVE")" && pwd)"
ARCHIVE="$archive_dir/$(basename -- "$ARCHIVE")"
[[ -n "$CHECKSUM" ]] || CHECKSUM="$ARCHIVE.sha256"
[[ -n "$MANIFEST" ]] || MANIFEST="${ARCHIVE%.tar.gz}.manifest.json"
validate_external_checksum "$ARCHIVE" "$CHECKSUM"
perform_install

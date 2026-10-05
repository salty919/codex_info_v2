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
RELEASES_URL="https://api.github.com/repos/salty919/codex_info_v2/releases/latest"
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
previous_combined=0
legacy_combined_enabled=0
legacy_combined_active=0
legacy_combined_generation=0
legacy_combined_prestate=
journal_legacy_combined_prestate=
journal_legacy_recovery_reader_hash=
legacy_recovery_reader_hash=
running_installer_source="$(readlink -f -- "$0" 2>/dev/null || true)"
recorder_reused=0
recorder_override_migrated=0
migrate_recorder_override=0
rollback_recorder_receipt=
transaction_recovered=0
operation_deadline=0
install_deadline=
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
recorder_binary_destination="$local_bin/codex_info_recorder"
rest_binary_destination="$local_bin/codex_info_rest"
launcher_destination="$local_bin/codex-info"
installer_destination="$local_libexec/codex-info-install.sh"
manifest_destination="$share_dir/manifest.json"
unit_destination="$unit_dir/codex-info-recorder.service"
recorder_override_destination="$unit_destination.d/90-issue-134-current-cli.conf"
rest_unit_destination="$unit_dir/codex-info-rest.service"
legacy_combined_unit_destination="$unit_dir/codex-info.service"
legacy_combined_enable_destination="$unit_dir/default.target.wants/codex-info.service"
update_service_destination="$unit_dir/codex-info-update.service"
update_timer_destination="$unit_dir/codex-info-update.timer"
main_enable_destination="$unit_dir/default.target.wants/codex-info-recorder.service"
rest_enable_destination="$unit_dir/default.target.wants/codex-info-rest.service"
timer_enable_destination="$unit_dir/timers.target.wants/codex-info-update.timer"
current_link="$share_dir/current"
transaction="$share_dir/install-transaction.json"
legacy_recovery_reader_destination="$share_dir/.legacy-recovery-install.sh"
legacy_recovery_journal="$share_dir/.legacy-recovery-transaction.json"
control_state="$share_dir/control-state.json"
install_lock="$share_dir/.install.lock"
update_stage=
update_target=
update_failure_file="$share_dir/last-update-failure.txt"
proc_root="$(printenv CODEX_INFO_PROC_ROOT || printf '/proc')"

usage() {
    cat <<'EOF'
usage: install.sh --bundle ARCHIVE [--manifest FILE] [--sha256 FILE]
       install.sh --update [--migrate-recorder-override]
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
update_log() {
    [[ -n "$update_stage" ]] || return 0
    python3 - "$share_dir" "$TRIGGER" "$update_target" "$update_stage" "$1" "${2:-}" <<'PY'
import json, pathlib, sys, time
directory, trigger, target, stage, result, reason = sys.argv[1:]
directory = pathlib.Path(directory)
try:
    record = {"timestamp":int(time.time()), "trigger":trigger, "target":target,
              "stage":stage, "result":result, "reason":reason}
    with (directory / "update.log").open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(record) + "\n")
    if result == "failed":
        (directory / "last-update-failure.txt").write_text(reason + "\n", encoding="utf-8")
except OSError:
    print("linux-bundle-install: update_log_write_failed", file=sys.stderr)
PY
}
die() { update_log failed "$*"; echo "linux-bundle-install: $*" >&2; exit 1; }
safe_blocked() { update_log failed "$*"; echo "SAFE_BLOCKED: $*" >&2; exit 1; }

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
        --migrate-recorder-override)
            (( ! migrate_recorder_override )) || die 'recorder migration supplied twice'
            migrate_recorder_override=1; shift ;;
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

if (( migrate_recorder_override )); then
    [[ "$ACTION" == update || ( "$ACTION" == install && -n "$ARCHIVE" &&
        "${CODEX_INFO_INSTALL_LOCKED:-}" == 1 && -e /proc/self/fd/9 ) ]] ||
        die '--migrate-recorder-override requires an explicit normal update'
fi

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
        update_target="${CODEX_INFO_UPDATE_TARGET:-}"
        [[ -z "$update_target" ]] || update_stage=install
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
        elif [[ "$ACTION" == timer-update ]]; then
            # A persistent timer may become due while a manual publication owns
            # L1. The publication is authoritative; defer this redundant tick
            # without turning normal lock contention into a failed unit.
            printf 'update deferred: another install, update, or control operation is running\n'
            exit 0
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
reserve_rollback_budget() {
    local now
    now="$(now_unix)" || safe_blocked 'installation clock is unavailable'
    (( operation_deadline > now + ROLLBACK_TIMEOUT )) ||
        safe_blocked 'insufficient overall time for installation and rollback'
    operation_deadline=$((operation_deadline - ROLLBACK_TIMEOUT))
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
enable_link_record() {
    case "$1" in
        codex-info-recorder.service)
            printf '%s\t%s\n' "$main_enable_destination" '../codex-info-recorder.service'
            ;;
        codex-info-rest.service)
            printf '%s\t%s\n' "$rest_enable_destination" '../codex-info-rest.service'
            ;;
        codex-info-update.timer)
            printf '%s\t%s\n' "$timer_enable_destination" '../codex-info-update.timer'
            ;;
        *) die "unsupported managed enable unit: $1" ;;
    esac
}
known_enable_link() {
    local unit="$1" destination="$2" expected="$3" link_target resolved generation_path
    [[ -L "$destination" ]] || return 1
    link_target="$(readlink -- "$destination" 2>/dev/null || true)"
    [[ "$link_target" == "$expected" ]] && return 0
    resolved="$(readlink -f -- "$destination" 2>/dev/null || true)"
    [[ "$resolved" == "$generations_dir/"*"/$unit" ]] || return 1
    generation_path="${resolved%/$unit}"
    [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || return 1
    verify_generation_files "$generation_path" >/dev/null 2>&1
}
probe_enabled() {
    local unit="$1" destination expected status=0
    IFS=$'\t' read -r destination expected <<<"$(enable_link_record "$unit")"
    if [[ -e "$destination" || -L "$destination" ]]; then
        known_enable_link "$unit" "$destination" "$expected" ||
            safe_blocked "foreign enable link for $unit"
        return 0
    fi
    systemctl_user is-enabled --quiet "$unit" >/dev/null 2>&1 || status="$?"
    case "$status" in
        0) return 0 ;;
        1|4) return 1 ;;
        *) die "could not inspect enabled state for $unit" ;;
    esac
}
enable_managed_unit() {
    local unit="$1" destination expected
    IFS=$'\t' read -r destination expected <<<"$(enable_link_record "$unit")"
    if [[ -e "$destination" || -L "$destination" ]]; then
        known_enable_link "$unit" "$destination" "$expected" ||
            safe_blocked "foreign enable link for $unit"
    fi
    mkdir -p -- "$(dirname -- "$destination")"
    atomic_symlink "$expected" "$destination"
    systemctl_user daemon-reload >/dev/null 2>&1 || return 1
    probe_enabled "$unit"
}
disable_managed_unit() {
    local unit="$1" destination expected
    IFS=$'\t' read -r destination expected <<<"$(enable_link_record "$unit")"
    if [[ -e "$destination" || -L "$destination" ]]; then
        known_enable_link "$unit" "$destination" "$expected" ||
            safe_blocked "foreign enable link for $unit"
        atomic_unlink "$destination"
        systemctl_user daemon-reload >/dev/null 2>&1 || return 1
    fi
    ! probe_enabled "$unit"
}
converge_enable_links() {
    case "$desired_state" in
        running|stopped)
            enable_managed_unit codex-info-recorder.service &&
                enable_managed_unit codex-info-rest.service &&
                enable_managed_unit codex-info-update.timer
            ;;
        disabled|removed)
            disable_managed_unit codex-info-recorder.service &&
                disable_managed_unit codex-info-rest.service &&
                disable_managed_unit codex-info-update.timer
            ;;
        *) safe_blocked "unsupported desired state for enable links: $desired_state" ;;
    esac
}
probe_active() {
    local unit="$1" status=0
    systemctl_user is-active --quiet "$unit" >/dev/null 2>&1 || status="$?"
    case "$status" in
        0) return 0 ;;
        # systemd reports an inactive known unit as 3 and a unit which has
        # not been published yet as 4.  Both are the same inactive pre-state
        # during the one-way combined-service to split-service migration.
        3|4) return 1 ;;
        *) die "could not inspect active state for $unit" ;;
    esac
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
    local unit="$1" now deadline
    now="$(now_unix)" || return 1
    deadline=$(( now + STOP_TIMEOUT ))
    if (( operation_deadline > 0 && operation_deadline < deadline )); then deadline=$operation_deadline; fi
    while :; do
        probe_active "$unit" || return 0
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
        if (probe_active codex-info-rest.service); then
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
    pid="$(systemctl_user show --property=MainPID --value codex-info-rest.service 2>/dev/null)" || die 'could not read REST MainPID'
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || { printf '0\n'; return; }
    printf '%s\n' "$pid"
}
recorder_systemd_pid() {
    local pid
    pid="$(systemctl_user show --property=MainPID --value codex-info-recorder.service 2>/dev/null)" || die 'could not read recorder MainPID'
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
    content="$(python3 - "$phase" "$operation_id" "$journal_owner_pid" "$journal_owner_starttime" "$journal_boot_id" "$previous_id" "$candidate_id" "$desired_state" "$timestamp" "${legacy_recovery_reader_image:-}" <<'PY'
import json, sys
phase, operation, owner_pid, owner_starttime, boot, old_generation, new_generation, desired, timestamp, reader_image = sys.argv[1:]
document = {"schema":"codex-info-install-transaction-v1","operation_id":operation,
            "owner_pid":int(owner_pid),"owner_starttime":int(owner_starttime),"boot_id":boot,
            "phase":phase,"old_generation":old_generation,"new_generation":new_generation,
            "desired_state":desired,"updated_at_unix":int(timestamp)}
if reader_image: document["legacy_reader_image"] = reader_image
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
    journal_line="$(python3 - "$transaction" "${legacy_recovery_journal:-}" <<'PY'
import base64, gzip, hashlib, io, json, pathlib, re, sys
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise ValueError("duplicate key")
        result[key]=value
    return result
try: document=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"), object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
required={"schema","operation_id","owner_pid","owner_starttime","boot_id","phase","old_generation","new_generation","desired_state","updated_at_unix"}
staged_image = (isinstance(document,dict) and set(document)==required|{"legacy_reader_image"}
                and pathlib.Path(sys.argv[1])==pathlib.Path(sys.argv[2]) and document["phase"]=="prepared")
if not isinstance(document,dict) or (set(document)!=required and not staged_image) or document["schema"]!="codex-info-install-transaction-v1":
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
# The opaque operation ID carries the one prestate record while preserving the
# exact v1 key set understood by the installed predecessor. All backup members
# and owner checks use this complete ID; it is never replaced on resume.
legacy = None
reader_hash = ""
if "~lc1~" in document["operation_id"]:
    digest_pattern = r"(?:[0-9a-f]{64}|[A-Za-z0-9_-]{43})"
    match = re.fullmatch(r"([A-Za-z0-9_-]{1,64})~lc1~([01]{3})~(" + digest_pattern + r")~(" + digest_pattern + r")", document["operation_id"])
    if match is None: raise SystemExit("legacy operation identity is invalid")
    def digest(value):
        if len(value) == 64: return value
        decoded = base64.b64decode(value + "=", altchars=b"-_", validate=True)
        if len(decoded) != 32 or base64.urlsafe_b64encode(decoded).decode().rstrip("=") != value:
            raise SystemExit("legacy digest encoding is invalid")
        return decoded.hex()
    flags = match.group(2)
    manifest_hash, reader_hash = map(digest, match.group(3, 4))
    legacy = {"generation": flags[0] == "1", "enabled": flags[1] == "1",
              "active": flags[2] == "1", "manifest_sha256": manifest_hash}
    if (legacy["generation"] != bool(document["old_generation"]) or
            (legacy["generation"] and not document["old_generation"].endswith("-" + manifest_hash))):
        raise SystemExit("legacy combined journal prestate is invalid")
reader_image = document.get("legacy_reader_image", "")
if staged_image:
    if legacy is None or not isinstance(reader_image,str) or not reader_image or len(reader_image)>65536:
        raise SystemExit("legacy reader image is invalid")
    compressed = base64.b64decode(reader_image, validate=True)
    if base64.b64encode(compressed).decode()!=reader_image: raise SystemExit("legacy reader encoding is invalid")
    with gzip.GzipFile(fileobj=io.BytesIO(compressed)) as image:
        data = image.read(524289)
    if len(data)>524288 or hashlib.sha256(data).hexdigest()!=reader_hash:
        raise SystemExit("legacy reader image identity is invalid")
print(document["phase"],document["operation_id"],document["owner_pid"],document["owner_starttime"],
      document["boot_id"],document["old_generation"],document["new_generation"],document["desired_state"],json.dumps(legacy) if legacy is not None else "",reader_hash,reader_image,sep="\x1f")
PY
    )" || safe_blocked 'transaction journal is invalid or ambiguous'
    IFS=$'\x1f' read -r journal_phase journal_operation_id journal_owner_pid journal_owner_starttime journal_boot_id journal_previous_id journal_candidate_id journal_desired journal_legacy_combined_prestate journal_legacy_recovery_reader_hash journal_legacy_recovery_reader_image <<<"$journal_line"
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
paths=[]; binaries={}
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
    if path_name in {"codex_info", "codex_info_recorder", "codex_info_rest"}: binaries[path_name]=entry
if paths != sorted(paths) or set(binaries) != {"codex_info", "codex_info_recorder", "codex_info_rest"}:
    raise SystemExit("manifest file entries are invalid")
if "codex-info" + ".service" in set(paths):
    raise SystemExit("combined recorder/REST unit is forbidden")
print(version,source,hashlib.sha256(raw).hexdigest(),binaries["codex_info_recorder"]["sha256"],sep="\t")
PY
}
manifest_rest_hash() {
    local path="${1:-$manifest_destination}"
    [[ -f "$path" ]] || safe_blocked 'REST manifest is absent'
    python3 - "$path" <<'PY'
import json,pathlib,re,sys
path=pathlib.Path(sys.argv[1])
document=json.loads(path.read_text(encoding="utf-8"))
entries=document.get("files") if isinstance(document,dict) else None
matches=[entry for entry in entries or [] if isinstance(entry,dict) and entry.get("path")=="codex_info_rest"]
if len(matches)!=1 or not re.fullmatch(r"[0-9a-f]{64}", str(matches[0].get("sha256"))):
    raise SystemExit("REST binary manifest entry is invalid")
print(matches[0]["sha256"])
PY
}
legacy_flat_record() {
    [[ -f "$manifest_destination" && ! -L "$manifest_destination" ]] || return 1
    python3 - "$manifest_destination" "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$installer_destination" \
        "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination" \
        "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib,json,os,pathlib,re,stat,sys
(manifest_name,ui_binary_name,recorder_binary_name,rest_binary_name,installer_name,unit_name,rest_unit_name,update_service_name,update_timer_name,
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
paths={"codex_info":(ui_binary_name,0o755),"codex_info_recorder":(recorder_binary_name,0o755),"codex_info_rest":(rest_binary_name,0o755),"install.sh":(installer_name,0o755),
       "codex-info-recorder.service":(unit_name,0o644),"codex-info-rest.service":(rest_unit_name,0o644),
       "codex-info-update.service":(update_service_name,0o644),
       "codex-info-update.timer":(update_timer_name,0o644)}
if set(paths)-set(by_path): raise SystemExit("legacy manifest omits required flat member")
for name,(actual_name,mode) in paths.items():
    actual=regular(actual_name,mode); entry=by_path[name]
    if actual.stat().st_size != entry["size"] or hashlib.sha256(actual.read_bytes()).hexdigest()!=entry["sha256"]:
        raise SystemExit("legacy flat member does not match manifest")
print(version,source,hashlib.sha256(raw).hexdigest(),by_path["codex_info_recorder"]["sha256"],manifest_path.stat().st_size,sep="\t")
PY
}
legacy_combined_record_at() {
    python3 - "$@" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib,json,os,pathlib,re,stat,sys
(manifest_name,binary_name,installer_name,unit_name,update_service_name,update_timer_name,
 schema,product,target,compatibility)=sys.argv[1:]
manifest_path=pathlib.Path(manifest_name)
def pairs(items):
    result={}
    for key,value in items:
        if key in result: raise SystemExit("legacy combined manifest has duplicate keys")
        result[key]=value
    return result
def regular(path,mode):
    path=pathlib.Path(path)
    if not path.is_file() or path.is_symlink(): raise SystemExit("legacy combined member is not regular")
    metadata=path.stat()
    if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != mode:
        raise SystemExit("legacy combined member owner or mode is not trusted")
    return path
try:
    raw=manifest_path.read_bytes()
    document=json.loads(raw.decode("utf-8"),object_pairs_hook=pairs)
except Exception as error: raise SystemExit(str(error))
regular(manifest_path,0o644)
required={"schema","product","version","source_sha","run_id","run_attempt","target","compatibility","glibc_minimum","files"}
if not isinstance(document,dict) or set(document)!=required: raise SystemExit("legacy combined manifest keys are invalid")
if document["schema"]!=schema or document["product"]!=product or document["target"]!=target or document["compatibility"]!=compatibility:
    raise SystemExit("legacy combined manifest identity is invalid")
version,source=document["version"],document["source_sha"]
if (not isinstance(version,str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",version) or
    not isinstance(source,str) or not re.fullmatch(r"[0-9a-f]{40}",source) or
    not isinstance(document["run_id"],str) or not re.fullmatch(r"[1-9][0-9]*",document["run_id"]) or
    isinstance(document["run_attempt"],bool) or not isinstance(document["run_attempt"],int) or document["run_attempt"]<1 or
    not isinstance(document["glibc_minimum"],str) or not re.fullmatch(r"[0-9]+(?:[.][0-9]+)+",document["glibc_minimum"])):
    raise SystemExit("legacy combined manifest identity fields are invalid")
entries=document["files"]
if not isinstance(entries,list) or not entries: raise SystemExit("legacy combined manifest files are invalid")
by_path={}; ordered=[]; modes_present=None
for entry in entries:
    if not isinstance(entry,dict) or set(entry) not in ({"path","size","sha256"},{"path","size","sha256","mode"}):
        raise SystemExit("legacy combined manifest entry schema is invalid")
    has_mode="mode" in entry
    if modes_present is None: modes_present=has_mode
    if modes_present != has_mode: raise SystemExit("legacy combined manifest mixes entry schemas")
    name=entry["path"]
    if (not isinstance(name,str) or not name or name.startswith("/") or "\\" in name or
        any(part in {"",".",".."} for part in name.split("/") ) or name in by_path):
        raise SystemExit("legacy combined manifest path is invalid")
    if (isinstance(entry["size"],bool) or not isinstance(entry["size"],int) or entry["size"]<0 or
        not isinstance(entry["sha256"],str) or not re.fullmatch(r"[0-9a-f]{64}",entry["sha256"])):
        raise SystemExit("legacy combined manifest file identity is invalid")
    if has_mode and (isinstance(entry["mode"],bool) or not isinstance(entry["mode"],int) or entry["mode"] not in {0o644,0o755}):
        raise SystemExit("legacy combined manifest mode is invalid")
    by_path[name]=entry; ordered.append(name)
if ordered != sorted(ordered): raise SystemExit("legacy combined manifest files are not sorted")
paths={"codex_info":(binary_name,0o755),"install.sh":(installer_name,0o755),
       "codex-info.service":(unit_name,0o644),"codex-info-update.service":(update_service_name,0o644),
       "codex-info-update.timer":(update_timer_name,0o644)}
if set(paths)-set(by_path): raise SystemExit("legacy combined manifest omits required member")
for name,(actual_name,mode) in paths.items():
    actual=regular(actual_name,mode); entry=by_path[name]
    if modes_present and entry["mode"] != mode: raise SystemExit("legacy combined member mode differs")
    if actual.stat().st_size != entry["size"] or hashlib.sha256(actual.read_bytes()).hexdigest()!=entry["sha256"]:
        raise SystemExit("legacy combined member does not match manifest")
print(version,source,hashlib.sha256(raw).hexdigest(),by_path["codex_info"]["sha256"],manifest_path.stat().st_size,sep="\t")
PY
}
legacy_combined_record() {
    local resolved generation_path installer="$installer_destination"
    [[ -e "$legacy_combined_unit_destination" || -L "$legacy_combined_unit_destination" ]] || return 1
    if [[ -L "$legacy_combined_unit_destination" ]]; then
        resolved="$(readlink -f -- "$legacy_combined_unit_destination" 2>/dev/null || true)"
        [[ "$resolved" == "$generations_dir/"*"/codex-info.service" ]] || return 1
        generation_path="${resolved%/codex-info.service}"
        [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || return 1
        legacy_combined_record_at "$generation_path/manifest.json" "$generation_path/codex_info" \
            "$generation_path/install.sh" "$generation_path/codex-info.service" \
            "$generation_path/codex-info-update.service" "$generation_path/codex-info-update.timer"
    else
        if legacy_recovery_reader_active; then
            installer="$backup_dir/$operation_id-$(basename -- "$installer_destination")"
        fi
        legacy_combined_record_at "$manifest_destination" "$binary_destination" "$installer" \
            "$legacy_combined_unit_destination" "$update_service_destination" "$update_timer_destination"
    fi
}
legacy_combined_present() {
    [[ -e "$legacy_combined_unit_destination" || -L "$legacy_combined_unit_destination" ||
        -e "$legacy_combined_enable_destination" || -L "$legacy_combined_enable_destination" ]]
}
legacy_combined_retired() {
    [[ ! -e "$legacy_combined_unit_destination" && ! -L "$legacy_combined_unit_destination" &&
        ! -e "$legacy_combined_enable_destination" && ! -L "$legacy_combined_enable_destination" ]]
}
probe_legacy_combined_enabled() {
    local status=0
    systemctl_user is-enabled --quiet codex-info.service >/dev/null 2>&1 || status="$?"
    case "$status" in 0) return 0 ;; 1|4) return 1 ;; *) die 'could not inspect legacy combined enabled state' ;; esac
}
validate_legacy_combined_enable_link() {
    if [[ -e "$legacy_combined_enable_destination" || -L "$legacy_combined_enable_destination" ]]; then
        [[ -L "$legacy_combined_enable_destination" &&
           "$(readlink -- "$legacy_combined_enable_destination" 2>/dev/null || true)" == '../codex-info.service' ]] ||
            safe_blocked 'foreign legacy combined enable link'
    fi
}
legacy_combined_owner_record() {
    local resolved="$1" generation_path
    if [[ "$resolved" == "$binary_destination" ]]; then
        legacy_combined_record
        return
    fi
    [[ "$resolved" == "$generations_dir/"*"/codex_info" ]] || return 1
    generation_path="${resolved%/codex_info}"
    [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || return 1
    legacy_combined_record_at "$generation_path/manifest.json" "$generation_path/codex_info" \
        "$generation_path/install.sh" "$generation_path/codex-info.service" \
        "$generation_path/codex-info-update.service" "$generation_path/codex-info-update.timer"
}
legacy_flat_present() {
    local path
    for path in "$manifest_destination" "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$installer_destination" \
        "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
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
generation_prune_failed() {
    printf 'GENERATION_PRUNE_FAILED: %s\n' "$*" >&2
}
prune_obsolete_generations() {
    local legacy_rollback_id=
    read_journal
    if [[ -n "$journal_previous_id" ]] &&
        ! verify_generation_files "$generations_dir/$journal_previous_id" >/dev/null 2>&1; then
        [[ -d "$generations_dir/$journal_previous_id" && ! -L "$generations_dir/$journal_previous_id" &&
           "$(stat -c '%u' -- "$generations_dir/$journal_previous_id" 2>/dev/null || true)" == "$(id -u)" &&
           "$(stat -c '%a' -- "$generations_dir/$journal_previous_id" 2>/dev/null || true)" == 700 ]] ||
            { generation_prune_failed 'rollback generation is not a trusted legacy directory'; return 1; }
        python3 - "$generations_dir/$journal_previous_id/manifest.json" <<'PY_LEGACY_SCHEMA' ||
import json, pathlib, sys
try:
    document = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
    entries = document["files"]
    expected = {"codex_info", "install.sh", "codex-info.service",
                "codex-info-update.service", "codex-info-update.timer"}
    if not isinstance(entries, list) or {entry["path"] for entry in entries} != expected:
        raise ValueError
    if any(set(entry) != {"path", "size", "sha256"} for entry in entries):
        raise ValueError
except Exception:
    raise SystemExit(1)
PY_LEGACY_SCHEMA
            { generation_prune_failed 'rollback generation is not the exact legacy combined format'; return 1; }
        [[ ! -e "$generations_dir/$journal_previous_id/SHA256SUMS" &&
           ! -L "$generations_dir/$journal_previous_id/SHA256SUMS" ]] ||
            { generation_prune_failed 'legacy rollback unexpectedly has a modern checksum list'; return 1; }
        legacy_combined_record_at "$generations_dir/$journal_previous_id/manifest.json" \
            "$generations_dir/$journal_previous_id/codex_info" "$generations_dir/$journal_previous_id/install.sh" \
            "$generations_dir/$journal_previous_id/codex-info.service" \
            "$generations_dir/$journal_previous_id/codex-info-update.service" \
            "$generations_dir/$journal_previous_id/codex-info-update.timer" >/dev/null ||
            { generation_prune_failed 'legacy rollback generation failed its installed contract'; return 1; }
        legacy_rollback_id="$journal_previous_id"
    fi
    if python3 - "$generations_dir" "$current_link" "$transaction" "$unit_dir" "$proc_root" \
        "$legacy_rollback_id" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" "$journal_operation_id" <<'PY'
import hashlib
import json
import os
import re
import stat
import sys

generations_path, current_path, journal_path, unit_dir, proc_root, legacy_rollback_name, schema, product, target, compatibility, expected_operation = sys.argv[1:]
uid = os.getuid()
generation_pattern = re.compile(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)-[0-9a-f]{40}-[0-9a-f]{64}")
journal_generation_pattern = re.compile(r"(?:|(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)-[0-9a-f]{40}-[0-9a-f]{64})")
directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
file_flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC

class Unsafe(Exception):
    pass

class ReferenceInspectionUnavailable(Exception):
    pass

def require(condition, message):
    if not condition:
        raise Unsafe(message)

def pairs(items):
    result = {}
    for key, value in items:
        require(key not in result, "duplicate JSON key")
        result[key] = value
    return result

def identity(st):
    return (st.st_dev, st.st_ino, st.st_mode, st.st_uid, st.st_size, st.st_mtime_ns, st.st_ctime_ns)

def directory_identity(st):
    return (st.st_dev, st.st_ino, st.st_mode, st.st_uid)

def read_regular(parent_fd, name, expected_mode):
    fd = os.open(name, file_flags, dir_fd=parent_fd)
    try:
        st = os.fstat(fd)
        require(stat.S_ISREG(st.st_mode) and st.st_uid == uid and stat.S_IMODE(st.st_mode) == expected_mode,
                "generation member owner or mode differs")
        chunks = []
        while True:
            chunk = os.read(fd, 1024 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
        return b"".join(chunks), identity(st)
    finally:
        os.close(fd)

def open_member(root_fd, path):
    pieces = path.split("/")
    parent = os.dup(root_fd)
    try:
        for part in pieces[:-1]:
            child = os.open(part, directory_flags, dir_fd=parent)
            os.close(parent)
            parent = child
        fd = os.open(pieces[-1], file_flags, dir_fd=parent)
        return parent, fd
    except BaseException:
        os.close(parent)
        raise

def validate_generation(name, root_fd):
    require(generation_pattern.fullmatch(name) is not None, "generation directory name is malformed")
    generation_fd = os.open(name, directory_flags, dir_fd=root_fd)
    try:
        root_st = os.fstat(generation_fd)
        require(root_st.st_uid == uid and stat.S_IMODE(root_st.st_mode) == 0o700,
                "generation directory owner or mode differs")
        manifest_bytes, manifest_identity = read_regular(generation_fd, "manifest.json", 0o644)
        try:
            document = json.loads(manifest_bytes.decode("utf-8"), object_pairs_hook=pairs)
        except Exception as error:
            raise Unsafe("generation manifest is invalid") from error
        required = {"schema", "product", "version", "source_sha", "run_id", "run_attempt",
                    "target", "compatibility", "glibc_minimum", "files"}
        require(isinstance(document, dict) and set(document) == required, "generation manifest keys differ")
        require(document["schema"] == schema and document["product"] == product and
                document["target"] == target and document["compatibility"] == compatibility,
                "generation manifest authority differs")
        require(isinstance(document["version"], str) and
                re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)", document["version"]) is not None and
                isinstance(document["source_sha"], str) and re.fullmatch(r"[0-9a-f]{40}", document["source_sha"]) is not None,
                "generation version identity differs")
        require(isinstance(document["run_id"], str) and re.fullmatch(r"[1-9][0-9]*", document["run_id"]) is not None and
                type(document["run_attempt"]) is int and document["run_attempt"] > 0,
                "generation workflow identity differs")
        require(isinstance(document["glibc_minimum"], str) and
                re.fullmatch(r"[0-9]+(?:[.][0-9]+)+", document["glibc_minimum"]) is not None,
                "generation platform identity differs")
        require(name == document["version"] + "-" + document["source_sha"] + "-" + hashlib.sha256(manifest_bytes).hexdigest(),
                "generation directory identity differs")
        entries = document["files"]
        require(isinstance(entries, list), "generation file list is invalid")
        expected = {}
        for entry in entries:
            require(isinstance(entry, dict) and set(entry) == {"path", "size", "sha256", "mode"},
                    "generation file entry keys differ")
            path = entry["path"]
            require(isinstance(path, str) and path and not path.startswith("/") and "\\" not in path and
                    all(part not in {"", ".", ".."} for part in path.split("/")),
                    "generation member path is unsafe")
            require(path not in expected and path not in {"manifest.json", "SHA256SUMS"},
                    "generation member path is duplicated")
            require(type(entry["size"]) is int and entry["size"] >= 0 and
                    isinstance(entry["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]) is not None and
                    type(entry["mode"]) is int and entry["mode"] in {0o644, 0o755},
                    "generation member identity is malformed")
            expected[path] = entry
        require(list(expected) == sorted(expected), "generation members are not sorted")
        expected_dirs = set()
        for path in expected:
            pieces = path.split("/")
            for index in range(1, len(pieces)):
                expected_dirs.add("/".join(pieces[:index]))
        expected_files = set(expected) | {"manifest.json", "SHA256SUMS"}
        seen_files = set()
        seen_dirs = {"": directory_identity(root_st)}
        file_identities = {"manifest.json": manifest_identity}

        def walk(directory_fd, prefix):
            for child_name in os.listdir(directory_fd):
                child_path = prefix + child_name if not prefix else prefix + "/" + child_name
                child_st = os.stat(child_name, dir_fd=directory_fd, follow_symlinks=False)
                if stat.S_ISDIR(child_st.st_mode):
                    require(child_path in expected_dirs and child_st.st_uid == uid and
                            stat.S_IMODE(child_st.st_mode) == 0o700,
                            "generation contains an unexpected directory")
                    child_fd = os.open(child_name, directory_flags, dir_fd=directory_fd)
                    try:
                        opened_st = os.fstat(child_fd)
                        require(directory_identity(opened_st) == directory_identity(child_st),
                                "generation directory changed during verification")
                        seen_dirs[child_path] = directory_identity(opened_st)
                        walk(child_fd, child_path)
                    finally:
                        os.close(child_fd)
                else:
                    require(stat.S_ISREG(child_st.st_mode) and child_path in expected_files,
                            "generation contains an unsafe or unexpected member")
                    seen_files.add(child_path)
                    expected_mode = expected[child_path]["mode"] if child_path in expected else 0o644
                    data, member_identity = read_regular(directory_fd, child_name, expected_mode)
                    require(identity(os.stat(child_name, dir_fd=directory_fd, follow_symlinks=False)) == member_identity,
                            "generation member changed during verification")
                    file_identities[child_path] = member_identity
                    if child_path in expected:
                        entry = expected[child_path]
                        require(len(data) == entry["size"] and hashlib.sha256(data).hexdigest() == entry["sha256"],
                                "generation payload digest differs")
        walk(generation_fd, "")
        require(seen_files == expected_files and seen_dirs.keys() == expected_dirs | {""},
                "generation member set differs")
        sums_bytes, sums_identity = read_regular(generation_fd, "SHA256SUMS", 0o644)
        require(file_identities["SHA256SUMS"] == sums_identity,
                "generation checksum list changed during verification")
        file_identities["SHA256SUMS"] = sums_identity
        records = {}
        try:
            lines = sums_bytes.decode("utf-8").splitlines()
        except UnicodeDecodeError as error:
            raise Unsafe("generation checksum list is invalid") from error
        for line in lines:
            fields = line.split()
            require(len(fields) == 2 and re.fullmatch(r"[0-9a-f]{64}", fields[0]) is not None,
                    "generation checksum record is malformed")
            member = fields[1].removeprefix("*")
            require(member not in records, "generation checksum member is duplicated")
            records[member] = fields[0]
        require(set(records) == set(expected) | {"manifest.json"}, "generation checksum coverage differs")
        require(records["manifest.json"] == hashlib.sha256(manifest_bytes).hexdigest(),
                "generation manifest checksum differs")
        for path, entry in expected.items():
            require(records[path] == entry["sha256"], "generation payload checksum differs")
        return {"root": directory_identity(root_st), "dirs": seen_dirs, "files": file_identities}
    finally:
        os.close(generation_fd)

def read_committed_state(root_fd):
    current_target = os.readlink(current_path)
    require(current_target.startswith("generations/") and current_target.count("/") == 1,
            "current generation link is not canonical")
    current_name = current_target.split("/", 1)[1]
    require(generation_pattern.fullmatch(current_name) is not None, "current generation name is invalid")
    journal_fd = os.open(journal_path, file_flags)
    try:
        journal_stat = os.fstat(journal_fd)
        require(stat.S_ISREG(journal_stat.st_mode) and journal_stat.st_uid == uid and
                stat.S_IMODE(journal_stat.st_mode) == 0o600, "transaction journal owner or mode differs")
        chunks = []
        while True:
            chunk = os.read(journal_fd, 65536)
            if not chunk:
                break
            chunks.append(chunk)
        journal_bytes = b"".join(chunks)
    finally:
        os.close(journal_fd)
    try:
        journal = json.loads(journal_bytes.decode("utf-8"), object_pairs_hook=pairs)
    except Exception as error:
        raise Unsafe("transaction journal is invalid") from error
    required = {"schema", "operation_id", "owner_pid", "owner_starttime", "boot_id", "phase",
                "old_generation", "new_generation", "desired_state", "updated_at_unix"}
    require(isinstance(journal, dict) and set(journal) == required and
            journal["operation_id"] == expected_operation and
            journal["schema"] == "codex-info-install-transaction-v1" and journal["phase"] == "committed",
            "transaction journal is not committed and exact")
    require(isinstance(journal["operation_id"], str) and journal["operation_id"] and
            type(journal["owner_pid"]) is int and journal["owner_pid"] > 0 and
            type(journal["owner_starttime"]) is int and journal["owner_starttime"] > 0 and
            isinstance(journal["boot_id"], str) and journal["boot_id"] and
            journal["desired_state"] in {"running", "stopped", "disabled", "removed"} and
            type(journal["updated_at_unix"]) is int and journal["updated_at_unix"] > 0,
            "transaction journal fields are invalid")
    require(isinstance(journal["old_generation"], str) and journal_generation_pattern.fullmatch(journal["old_generation"]) is not None and
            isinstance(journal["new_generation"], str) and journal_generation_pattern.fullmatch(journal["new_generation"]) is not None,
            "committed journal generation fields are invalid")
    install_commit = journal["new_generation"] == current_name
    rollback_commit = journal["old_generation"] == current_name
    require(install_commit or rollback_commit, "current and committed journal disagree")
    require(not legacy_rollback_name or legacy_rollback_name in {journal["old_generation"], current_name},
            "legacy rollback does not match the committed journal")
    if current_name == legacy_rollback_name:
        legacy_fd = os.open(current_name, directory_flags, dir_fd=root_fd)
        try:
            legacy_stat = os.fstat(legacy_fd)
            require(legacy_stat.st_uid == uid and stat.S_IMODE(legacy_stat.st_mode) == 0o700,
                    "legacy current directory owner or mode differs")
        finally:
            os.close(legacy_fd)
    else:
        validate_generation(current_name, root_fd)
    rollback_name = journal["old_generation"] if install_commit and journal["old_generation"] != current_name else ""
    journal_candidate_name = journal["new_generation"] if rollback_commit and not install_commit else ""
    if rollback_name and rollback_name != current_name:
        if rollback_name == legacy_rollback_name:
            legacy_fd = os.open(legacy_rollback_name, directory_flags, dir_fd=root_fd)
            try:
                legacy_stat = os.fstat(legacy_fd)
                require(legacy_stat.st_uid == uid and stat.S_IMODE(legacy_stat.st_mode) == 0o700,
                        "legacy rollback directory owner or mode differs")
            finally:
                os.close(legacy_fd)
        else:
            validate_generation(rollback_name, root_fd)
    return current_name, rollback_name, journal_candidate_name

def generation_from_path(path, known_names):
    normalized = os.path.normpath(path)
    prefix = generations_path.rstrip("/") + "/"
    if not normalized.startswith(prefix):
        return None
    remainder = normalized[len(prefix):]
    name = remainder.split("/", 1)[0]
    require(name in known_names, "reference names an unknown generation")
    return name

def collect_references(known_names):
    protected = set()
    managed_units = ("codex-info-recorder.service", "codex-info-rest.service", "codex-info.service",
                     "codex-info-update.service", "codex-info-update.timer")
    enable_dirs = ("default.target.wants", "timers.target.wants")
    reference_paths = [os.path.join(unit_dir, name) for name in managed_units]
    for directory in enable_dirs:
        enable_dir = os.path.join(unit_dir, directory)
        for name in ("codex-info-recorder.service", "codex-info-rest.service", "codex-info.service",
                     "codex-info-update.timer"):
            reference_paths.append(os.path.join(enable_dir, name))
        try:
            extra_names = os.listdir(enable_dir)
        except FileNotFoundError:
            extra_names = []
        for name in extra_names:
            if name.startswith("codex-info") and (name.endswith(".service") or name.endswith(".timer")):
                path = os.path.join(enable_dir, name)
                if path not in reference_paths:
                    reference_paths.append(path)
    for path in reference_paths:
        if not os.path.lexists(path):
            continue
        try:
            resolved = os.path.realpath(path, strict=True)
        except OSError as error:
            raise Unsafe("managed unit or enable reference cannot be resolved") from error
        generation = generation_from_path(resolved, known_names)
        require(generation is not None, "managed unit or enable reference is outside a verified generation")
        protected.add(generation)
    try:
        process_names = os.listdir(proc_root)
    except OSError as error:
        raise ReferenceInspectionUnavailable("process references cannot be enumerated") from error
    for pid in process_names:
        if not pid.isdecimal():
            continue
        process_path = os.path.join(proc_root, pid)
        try:
            process_stat = os.stat(process_path)
        except FileNotFoundError:
            continue
        except OSError as error:
            raise ReferenceInspectionUnavailable("process owner cannot be inspected") from error
        if process_stat.st_uid != uid:
            continue
        try:
            executable = os.readlink(os.path.join(process_path, "exe"))
        except FileNotFoundError:
            continue
        except OSError as error:
            raise ReferenceInspectionUnavailable("owned process executable cannot be inspected") from error
        if executable.endswith(" (deleted)"):
            executable = executable[:-10]
        generation = generation_from_path(executable, known_names)
        if generation is not None:
            protected.add(generation)
    return protected

def safe_open_parent(generation_fd, path, directory_snapshot):
    pieces = path.split("/")
    parent_fd = os.dup(generation_fd)
    prefix = ""
    try:
        for part in pieces[:-1]:
            prefix = part if not prefix else prefix + "/" + part
            next_fd = os.open(part, directory_flags, dir_fd=parent_fd)
            opened = os.fstat(next_fd)
            require(directory_identity(opened) == directory_snapshot[prefix],
                    "generation directory identity changed before unlink")
            os.close(parent_fd)
            parent_fd = next_fd
        return parent_fd, pieces[-1]
    except BaseException:
        os.close(parent_fd)
        raise

def delete_generation(name, root_fd, snapshot):
    generation_fd = os.open(name, directory_flags, dir_fd=root_fd)
    try:
        require(directory_identity(os.fstat(generation_fd)) == snapshot["root"],
                "obsolete generation directory identity changed before unlink")
        current_snapshot = validate_generation(name, root_fd)
        require(current_snapshot == snapshot, "obsolete generation identity changed before unlink")
        for path in sorted(snapshot["files"], key=lambda value: (value in {"manifest.json", "SHA256SUMS"}, value)):
            parent_fd, leaf = safe_open_parent(generation_fd, path, snapshot["dirs"])
            try:
                member_stat = os.stat(leaf, dir_fd=parent_fd, follow_symlinks=False)
                require(identity(member_stat) == snapshot["files"][path] and stat.S_ISREG(member_stat.st_mode),
                        "generation member identity changed before unlink")
                os.unlink(leaf, dir_fd=parent_fd)
                os.fsync(parent_fd)
            finally:
                os.close(parent_fd)
        directories = [path for path in snapshot["dirs"] if path]
        for path in sorted(directories, key=lambda value: (-value.count("/"), value)):
            parent_fd, leaf = safe_open_parent(generation_fd, path, snapshot["dirs"])
            try:
                directory_stat = os.stat(leaf, dir_fd=parent_fd, follow_symlinks=False)
                require(directory_identity(directory_stat) == snapshot["dirs"][path] and stat.S_ISDIR(directory_stat.st_mode),
                        "generation directory identity changed before rmdir")
                child_fd = os.open(leaf, directory_flags, dir_fd=parent_fd)
                try:
                    require(not os.listdir(child_fd), "generation subdirectory is no longer empty")
                finally:
                    os.close(child_fd)
                os.rmdir(leaf, dir_fd=parent_fd)
                os.fsync(parent_fd)
            finally:
                os.close(parent_fd)
        generation_stat = os.stat(name, dir_fd=root_fd, follow_symlinks=False)
        require(directory_identity(generation_stat) == snapshot["root"] and stat.S_ISDIR(generation_stat.st_mode),
                "generation directory identity changed before final rmdir")
    finally:
        os.close(generation_fd)
    os.rmdir(name, dir_fd=root_fd)
    os.fsync(root_fd)
    require(not os.path.lexists(os.path.join(generations_path, name)), "removed generation remains present")

def main():
    root_fd = os.open(generations_path, directory_flags)
    try:
        root_stat = os.fstat(root_fd)
        require(root_stat.st_uid == uid and stat.S_IMODE(root_stat.st_mode) == 0o700,
                "generations root owner or mode differs")
        current, rollback, journal_candidate = read_committed_state(root_fd)
        known_names = set()
        valid = {}
        for name in os.listdir(root_fd):
            entry_stat = os.stat(name, dir_fd=root_fd, follow_symlinks=False)
            if not stat.S_ISDIR(entry_stat.st_mode) or not generation_pattern.fullmatch(name):
                continue
            known_names.add(name)
        require(current in known_names and (not rollback or rollback in known_names),
                "committed current or rollback generation is absent")
        for name in sorted(known_names):
            if name == legacy_rollback_name or name == journal_candidate:
                continue
            try:
                valid[name] = validate_generation(name, root_fd)
            except (Unsafe, OSError):
                if name in {current, rollback}:
                    raise Unsafe("committed current or rollback generation failed verification")
        protected_names = {name for name in (current, rollback, journal_candidate, legacy_rollback_name) if name}
        reference_names = set(valid) | protected_names
        references = collect_references(reference_names)
        for name in sorted(valid):
            latest_current, latest_rollback, latest_candidate = read_committed_state(root_fd)
            references = collect_references(reference_names)
            if name in {latest_current, latest_rollback, latest_candidate} or name in references:
                continue
            delete_generation(name, root_fd, valid[name])
    finally:
        os.close(root_fd)

try:
    main()
except ReferenceInspectionUnavailable as error:
    # The committed generation has already been verified. Unknown process
    # references prevent deletion, but do not invalidate that outcome.
    print(f"GENERATION_PRUNE_DEFERRED: {error}; remaining obsolete generations retained", file=sys.stderr)
except Exception as error:
    print(str(error) or error.__class__.__name__, file=sys.stderr)
    raise SystemExit(1)
PY
    then
        return 0
    fi
    generation_prune_failed 'verified obsolete generation cleanup did not complete'
    return 1
}
verify_fixed_links() {
    local destination expected
    for destination in "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ -L "$destination" ]] || safe_blocked "fixed link missing: $destination"
    done
    [[ "$(readlink -- "$binary_destination")" == '../share/codex-info/current/codex_info' ]] || safe_blocked 'UI binary link is not canonical'
    [[ "$(readlink -- "$recorder_binary_destination")" == '../share/codex-info/current/codex_info_recorder' ]] || safe_blocked 'recorder binary link is not canonical'
    [[ "$(readlink -- "$rest_binary_destination")" == '../share/codex-info/current/codex_info_rest' ]] || safe_blocked 'REST binary link is not canonical'
    [[ "$(readlink -- "$launcher_destination")" == '../share/codex-info/current/run.sh' ]] || safe_blocked 'launcher link is not canonical'
    [[ "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' ]] || safe_blocked 'installer link is not canonical'
    [[ "$(readlink -- "$manifest_destination")" == 'current/manifest.json' ]] || safe_blocked 'manifest link is not canonical'
    for destination in "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
        expected="../../../.local/share/codex-info/current/$(basename -- "$destination")"
        [[ "$(readlink -- "$destination")" == "$expected" ]] || safe_blocked "unit link is not canonical"
    done
}
verify_fixed_links_local() {
    local destination expected
    for destination in "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ -L "$destination" ]] || return 1
    done
    [[ "$(readlink -- "$binary_destination")" == '../share/codex-info/current/codex_info' ]] || return 1
    [[ "$(readlink -- "$recorder_binary_destination")" == '../share/codex-info/current/codex_info_recorder' ]] || return 1
    [[ "$(readlink -- "$rest_binary_destination")" == '../share/codex-info/current/codex_info_rest' ]] || return 1
    [[ "$(readlink -- "$launcher_destination")" == '../share/codex-info/current/run.sh' ]] || return 1
    [[ "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' ]] || return 1
    [[ "$(readlink -- "$manifest_destination")" == 'current/manifest.json' ]] || return 1
    for destination in "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
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
        [[ -L "$recorder_binary_destination" && "$(readlink -- "$recorder_binary_destination")" == '../share/codex-info/current/codex_info_recorder' ]] || return 1
        [[ -L "$rest_binary_destination" && "$(readlink -- "$rest_binary_destination")" == '../share/codex-info/current/codex_info_rest' ]] || return 1
        [[ -L "$launcher_destination" && "$(readlink -- "$launcher_destination")" == '../share/codex-info/current/run.sh' ]] || return 1
        [[ -L "$installer_destination" && "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' ]] || return 1
        [[ -L "$manifest_destination" && "$(readlink -- "$manifest_destination")" == 'current/manifest.json' ]] || return 1
        [[ ! -e "$unit_destination" && ! -L "$unit_destination" ]] || return 1
        [[ ! -e "$rest_unit_destination" && ! -L "$rest_unit_destination" ]] || return 1
        [[ ! -e "$update_service_destination" && ! -L "$update_service_destination" ]] || return 1
        [[ ! -e "$update_timer_destination" && ! -L "$update_timer_destination" ]] || return 1
    else
        verify_fixed_links_local || return 1
    fi
}
check_glibc_compatibility() {
    local manifest="$1" host_text host_version
    host_text="$("$GETCONF_BIN" GNU_LIBC_VERSION 2>/dev/null || true)"
    host_version="${host_text#*glibc }"
    if [[ ! "$host_version" =~ ^[0-9]+([.][0-9]+)+$ ]]; then
        host_text="$("$LDD_BIN" --version 2>/dev/null | head -n 1 || true)"
        host_version="$(printf '%s\n' "$host_text" | grep -oE '[0-9]+(\.[0-9]+)+' | head -n 1 || true)"
    fi
    [[ "$host_version" =~ ^[0-9]+([.][0-9]+)+$ ]] || return 0
    python3 - "$manifest" "$host_version" <<'PY'
import json,pathlib,re,sys
manifest,host_text=sys.argv[1:]
try:
    document=json.loads(pathlib.Path(manifest).read_text(encoding="utf-8"))
except Exception:
    raise SystemExit(0)
minimum=document.get("glibc_minimum") if isinstance(document,dict) else None
if not isinstance(minimum,str) or not re.fullmatch(r"[0-9]+(?:[.][0-9]+)+",minimum):
    raise SystemExit(0)
host=tuple(int(part) for part in host_text.split(".")); required=tuple(int(part) for part in minimum.split("."))
if host < required: raise SystemExit("host glibc is older than candidate minimum")
PY
}
extract_bundle_manifest() {
    local archive="$1" destination="$2"
    python3 - "$archive" "$destination" <<'PY'
import pathlib,sys,tarfile
archive_path,destination=sys.argv[1:]
try:
    with tarfile.open(archive_path,"r:gz") as bundle:
        source=bundle.extractfile("manifest.json")
        if source is None:
            raise SystemExit("archive manifest cannot be read")
        pathlib.Path(destination).write_bytes(source.read())
except (KeyError,OSError,tarfile.TarError) as error:
    raise SystemExit(f"archive manifest cannot be read: {error}")
PY
}

validate_bundle() {
    local archive="$1" external="$2" destination="$3" checksum="$4" release_digest="${5:-}" validation_limit="$VALIDATE_TIMEOUT"
    [[ -f "$archive" && ! -L "$archive" ]] || die 'bundle archive is not regular'
    [[ "$archive" == *.tar.gz ]] || die 'bundle archive has wrong suffix'
    [[ -f "$external" && ! -L "$external" ]] || die 'external manifest is not regular'
    if (( operation_deadline > 0 )); then
        validation_limit="$(deadline_timeout "$VALIDATE_TIMEOUT")" || return 1
    fi
    python3 - "$archive" "$external" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" "$validation_limit" "$destination" "$checksum" "$release_digest" <<'PY'
import hashlib,json,os,pathlib,re,signal,stat,sys,tarfile,tempfile
archive_name,manifest_name,schema,product,target,compatibility,timeout_seconds,destination_name,checksum_name,release_digest=sys.argv[1:]
def reject(message): raise SystemExit("candidate staging failed: "+message)
def timeout_handler(signum, frame): raise TimeoutError("candidate staging timed out")
signal.signal(signal.SIGALRM,timeout_handler); signal.alarm(int(timeout_seconds))
def pairs(items):
    result={}
    for key,value in items:
        if key in result: reject("duplicate JSON key")
        result[key]=value
    return result
def regular_bytes(name):
    fd=os.open(name,os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(fd,"rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode): reject("input is not regular")
        return stream.read()
def identity(metadata):
    return metadata.st_dev,metadata.st_ino,metadata.st_size,metadata.st_mtime_ns,metadata.st_ctime_ns
def stage():
    destination=pathlib.Path(destination_name)
    metadata=destination.lstat()
    if (not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid!=os.getuid() or
            stat.S_IMODE(metadata.st_mode)!=0o700 or any(destination.iterdir())): reject("stage is not empty and owner-only")
    raw=regular_bytes(manifest_name)
    manifest=json.loads(raw.decode("utf-8"),object_pairs_hook=pairs)
    required={"schema","product","version","source_sha","run_id","run_attempt","target","compatibility","glibc_minimum","files"}
    if not isinstance(manifest,dict) or set(manifest)!=required: reject("manifest keys are not exact")
    if manifest["schema"]!=schema or manifest["product"]!=product: reject("manifest identity")
    if not isinstance(manifest["version"],str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",manifest["version"]): reject("version")
    if pathlib.Path(archive_name).name!=f"codex-info-{manifest['version']}-{target}.tar.gz": reject("archive name")
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
    if release_digest:
        if not re.fullmatch(r"sha256:[0-9a-f]{64}",release_digest): reject("release digest is invalid")
        expected_archive_hash=release_digest.removeprefix("sha256:")
    else:
        if "codex-info"+".service" in by_path: reject("combined recorder/REST unit is forbidden")
        required_files={"codex_info","codex_info_recorder","codex_info_rest","run.sh","install.sh","codex-info-recorder.service","codex-info-rest.service","codex-info-update.service","codex-info-update.timer","LICENSE","COPYRIGHT"}
        if not required_files.issubset(by_path) or not ({"THIRD_PARTY_NOTICES.md","NOTICE.txt"}&set(by_path)): reject("required member missing")
        lines=regular_bytes(checksum_name).decode("utf-8").splitlines()
        if sum(bool(line.strip()) and not line.lstrip().startswith("#") for line in lines)!=1: reject("external checksum must contain exactly one record")
        fields=lines[0].split() if lines else []
        if len(fields)!=2 or not re.fullmatch(r"[0-9a-fA-F]{64}",fields[0]) or fields[1].removeprefix("*")!=pathlib.Path(archive_name).name: reject("external checksum record is invalid")
        expected_archive_hash=fields[0].lower()
    # Copy and hash the bytes together. Path replacement or in-place changes cannot
    # turn a later archive open into a different validated candidate.
    source_fd=os.open(archive_name,os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(source_fd,"rb") as source, tempfile.TemporaryFile(dir=destination) as snapshot:
        before=os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode): reject("archive is not regular")
        digest=hashlib.sha256()
        while chunk:=source.read(1024*1024):
            digest.update(chunk); snapshot.write(chunk)
        if digest.hexdigest()!=expected_archive_hash: reject("archive digest differs")
        snapshot.flush(); snapshot.seek(0)
        with tarfile.open(fileobj=snapshot,mode="r:gz") as archive:
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
            for path,entry in by_path.items():
                member=archive.getmember(path)
                expected_mode=entry["mode"] if release_digest else (0o755 if path in {"codex_info","codex_info_recorder","codex_info_rest","run.sh","install.sh"} else 0o644)
                if member.mode&0o7777!=expected_mode or expected_mode!=entry["mode"] or member.size!=entry["size"]: reject("mode/size mismatch")
            for path in ("manifest.json","SHA256SUMS"):
                if archive.getmember(path).mode&0o7777!=0o644: reject("metadata mode mismatch")
            for member in archive.getmembers():
                target_path=destination.joinpath(*pathlib.PurePosixPath(member.name).parts)
                target_path.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
                stream=archive.extractfile(member)
                if stream is None: reject("member cannot be read")
                fd,temporary=tempfile.mkstemp(prefix=".codex-info.",dir=target_path.parent)
                try:
                    with os.fdopen(fd,"wb") as output:
                        digest=hashlib.sha256(); size=0
                        while chunk:=stream.read(1024*1024):
                            output.write(chunk); digest.update(chunk); size+=len(chunk)
                        if size!=member.size: reject("member size differs")
                        if member.name!="SHA256SUMS" and digest.hexdigest()!=records[member.name]: reject("SHA256SUMS digest")
                        if member.name in by_path and digest.hexdigest()!=by_path[member.name]["sha256"]: reject("manifest digest")
                        output.flush(); os.fsync(output.fileno())
                    os.chmod(temporary,member.mode&0o7777); os.replace(temporary,target_path)
                    parent_fd=os.open(target_path.parent,os.O_DIRECTORY)
                    try: os.fsync(parent_fd)
                    finally: os.close(parent_fd)
                finally:
                    try: os.unlink(temporary)
                    except FileNotFoundError: pass
        if identity(os.fstat(source.fileno()))!=identity(before) or identity(os.lstat(archive_name))!=identity(before): reject("archive changed during staging")
    stage_fd=os.open(destination,os.O_DIRECTORY)
    try: os.fsync(stage_fd)
    finally: os.close(stage_fd)
    signal.alarm(0)
    recorder_hash="" if release_digest else by_path["codex_info_recorder"]["sha256"]
    print(manifest["version"],manifest["source_sha"],hashlib.sha256(raw).hexdigest(),recorder_hash,sep="\t")
try: stage()
except (OSError,tarfile.TarError,UnicodeError,ValueError,TimeoutError) as error: reject(str(error))
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
        "$binary_destination"|"$recorder_binary_destination"|"$rest_binary_destination"|"$installer_destination") expected_mode=755 ;;
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
backup_legacy_combined_unit() {
    local destination="$legacy_combined_unit_destination" resolved generation_path backup
    [[ -e "$destination" || -L "$destination" ]] || return 0
    if [[ ! -L "$destination" ]]; then
        backup_legacy_path "$destination"
        return
    fi
    resolved="$(readlink -f -- "$destination" 2>/dev/null || true)"
    [[ "$resolved" == "$generations_dir/"*"/codex-info.service" ]] ||
        safe_blocked 'foreign legacy combined unit symlink'
    generation_path="${resolved%/codex-info.service}"
    [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] ||
        safe_blocked 'legacy combined unit generation path is invalid'
    backup="$backup_dir/$operation_id-codex-info.service"
    [[ ! -e "$backup" && ! -L "$backup" ]] || safe_blocked 'legacy combined unit backup collision'
    mkdir -p -- "$backup_dir"; chmod 700 -- "$backup_dir"
    python3 - "$destination" "$backup" <<'PY'
import os,sys
from pathlib import Path
source,destination=map(Path,sys.argv[1:])
os.replace(source,destination)
for parent in {source.parent,destination.parent}:
    fd=os.open(parent,os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
PY
}
link_entrypoints() {
    mkdir -p -- "$local_bin" "$local_libexec" "$unit_dir"
    atomic_symlink '../share/codex-info/current/codex_info' "$binary_destination"
    atomic_symlink '../share/codex-info/current/codex_info_recorder' "$recorder_binary_destination"
    atomic_symlink '../share/codex-info/current/codex_info_rest' "$rest_binary_destination"
    atomic_symlink '../share/codex-info/current/run.sh' "$launcher_destination"
    if [[ -z "${legacy_recovery_reader_hash:-}" ]]; then atomic_symlink '../share/codex-info/current/install.sh' "$installer_destination"; fi
    atomic_symlink 'current/manifest.json' "$manifest_destination"
    if [[ "${desired_state-}" == removed ]]; then
        local destination expected
        for destination in "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
            [[ -e "$destination" || -L "$destination" ]] || continue
            expected="../../../.local/share/codex-info/current/$(basename -- "$destination")"
            [[ -L "$destination" && "$(readlink -- "$destination")" == "$expected" ]] ||
                safe_blocked "foreign unit link in removed state: $destination"
            atomic_unlink "$destination"
        done
        return 0
    fi
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-recorder.service' "$unit_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-rest.service' "$rest_unit_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-update.service' "$update_service_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-update.timer' "$update_timer_destination"
}
restore_backups() {
    python3 - "$operation_id" "$backup_dir" "${legacy_recovery_reader_hash:-}" "$installer_destination" "$current_link" "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$rest_unit_destination" "$legacy_combined_unit_destination" "$update_service_destination" "$update_timer_destination" "$recorder_override_destination" <<'PY'
import os,sys
from pathlib import Path
operation,backup_root,reader_hash,installer,*destinations=sys.argv[1:]
backup_root=Path(backup_root)
for destination_name in reversed(destinations):
    if reader_hash and destination_name == installer: continue
    destination=Path(destination_name)
    backup=backup_root / (operation + "-" + destination.name)
    if backup.exists() or backup.is_symlink():
        if destination.exists() or destination.is_symlink(): destination.unlink()
        expected=0o755 if destination.name in {"codex_info","codex_info_recorder","codex_info_rest","codex-info-install.sh"} else 0o644
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
    for destination in "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
        if [[ "$destination" == "$installer_destination" && -n "${legacy_recovery_reader_hash:-}" ]]; then continue; fi
        [[ -L "$destination" ]] || continue
        link_target="$(readlink -- "$destination" 2>/dev/null || true)"
        case "$destination" in
            "$binary_destination") expected='../share/codex-info/current/codex_info' ;;
            "$recorder_binary_destination") expected='../share/codex-info/current/codex_info_recorder' ;;
            "$rest_binary_destination") expected='../share/codex-info/current/codex_info_rest' ;;
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
    (( previous_combined == 0 )) || return 0
    local destination expected
    mkdir -p -- "$local_bin" "$local_libexec" "$unit_dir"
    for destination in "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
        [[ "${desired_state-}" == removed && ("$destination" == "$unit_destination" || "$destination" == "$rest_unit_destination" || "$destination" == "$update_service_destination" || "$destination" == "$update_timer_destination") ]] && continue
        [[ -e "$destination" || -L "$destination" ]] && continue
        case "$destination" in
            "$binary_destination") expected='../share/codex-info/current/codex_info' ;;
            "$recorder_binary_destination") expected='../share/codex-info/current/codex_info_recorder' ;;
            "$rest_binary_destination") expected='../share/codex-info/current/codex_info_rest' ;;
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
    local resolved actual expected generation_path legacy_info
    resolved="$(readlink -f -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)"
    if [[ "$resolved" == "$rest_binary_destination" ]]; then
        local legacy_info legacy_binary
        legacy_info="$(legacy_flat_record)" || safe_blocked 'legacy REST listener state is not trusted'
        IFS=$'\t' read -r _ _ _ legacy_binary _ <<<"$legacy_info"
        legacy_binary="$(manifest_rest_hash "$manifest_destination")" || safe_blocked 'legacy REST listener hash is unavailable'
        [[ "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$rest_binary_destination" 2>/dev/null || true)" ]] ||
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
    if legacy_info="$(legacy_combined_owner_record "$resolved" 2>/dev/null)"; then
        IFS=$'\t' read -r _ _ _ expected _ <<<"$legacy_info"
        actual="$(sha256sum -- "$proc_root/$listener_pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        [[ -n "$actual" && "$actual" == "$expected" ]] || safe_blocked 'legacy combined listener digest differs'
        kill -TERM "$listener_pid" 2>/dev/null || safe_blocked 'legacy combined listener could not be retired'
        local legacy_deadline; legacy_deadline=$(( $(now_unix) + HEALTH_TIMEOUT ))
        while kill -0 "$listener_pid" 2>/dev/null; do
            (( $(now_unix) < legacy_deadline )) || safe_blocked 'legacy combined listener did not stop in 30s'
            sleep_interval 1
        done
        return 0
    fi
    [[ "$resolved" == "$generations_dir/"*"/codex_info_rest" ]] || safe_blocked 'foreign listener owner is present'
    generation_path="${resolved%/codex_info_rest}"
    [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || safe_blocked 'listener generation path is invalid'
    verify_generation_files "$generation_path" || safe_blocked 'known listener generation is incoherent'
    expected="$(python3 - "$resolved" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" <<'PY'
import hashlib, json, pathlib, re, stat, sys

path = pathlib.Path(sys.argv[1])
schema, product, target, compatibility = sys.argv[2:]
if path.name != "codex_info_rest" or not path.is_file() or path.is_symlink():
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
binary = [entry for entry in entries if isinstance(entry, dict) and entry.get("path") == "codex_info_rest"]
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
    local listener_pid managed_pid=0 resolved generation_path info expected actual legacy_info
    listener_pid="$(socket_pid)" || safe_blocked 'listener ownership is ambiguous'
    [[ -z "$listener_pid" ]] && return 0
    if probe_active codex-info-rest.service; then managed_pid="$(systemd_pid)"; fi
    [[ "$listener_pid" == "$managed_pid" ]] && return 0
    resolved="$(readlink -f -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)"
    if [[ "$resolved" == "$rest_binary_destination" ]]; then
        info="$(legacy_flat_record)" || safe_blocked 'legacy REST listener state is not trusted'
        expected="$(manifest_rest_hash "$manifest_destination")" || safe_blocked 'legacy REST listener hash is unavailable'
        [[ "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$rest_binary_destination" 2>/dev/null || true)" ]] || safe_blocked 'legacy REST listener executable identity differs'
    elif [[ "$resolved" == "$generations_dir/"*"/codex_info_rest" ]]; then
        generation_path="${resolved%/codex_info_rest}"
        [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || safe_blocked 'listener generation path is invalid'
        verify_generation_files "$generation_path" || safe_blocked 'known listener generation is incoherent'
        expected="$(manifest_rest_hash "$generation_path/manifest.json")" || safe_blocked 'known listener REST hash is unavailable'
    elif legacy_info="$(legacy_combined_owner_record "$resolved" 2>/dev/null)"; then
        IFS=$'\t' read -r _ _ _ expected _ <<<"$legacy_info"
        [[ "$(stat -Lc '%d:%i' -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" ]] ||
            safe_blocked 'legacy combined listener executable identity differs'
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
    if probe_active codex-info-rest.service; then managed_pid="$(systemd_pid)"; fi
    [[ "$listener_pid" == "$managed_pid" ]] || safe_blocked 'foreign listener blocks control mutation'
}
# A historical override is product-owned only when its exact configuration and
# payload bind to a completely verified installed product generation.  A name,
# location, process heartbeat, or displayed version is never ownership proof.
capture_recorder_override_prestate() {
    local generation="$1" content pid
    pid="$(recorder_systemd_pid)"
    content="$(python3 - "$operation_id" "$generation" "$recorder_override_destination" "$pid" "$proc_root" <<'PY_RECORDER_PRESTATE'
import hashlib, json, os, pathlib, stat, sys
operation, generation, override_name, pid, proc_root = sys.argv[1:]
override = pathlib.Path(override_name)
def record(path, mode):
    before = path.lstat()
    if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid() or
            stat.S_IMODE(before.st_mode) != mode or path.resolve(strict=True) != path):
        raise SystemExit("SAFE_BLOCKED: recorder prestate owner/type/mode/path differs")
    data = path.read_bytes()
    after = path.lstat()
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise SystemExit("SAFE_BLOCKED: recorder prestate changed while captured")
    return data, {"device":before.st_dev,"inode":before.st_ino,"sha256":hashlib.sha256(data).hexdigest()}
data, override_record = record(override, 0o644)
lines = data.decode("utf-8").splitlines()
if len(lines) != 3 or lines[:2] != ["[Service]", "ExecStart="] or not lines[2].startswith("ExecStart=/"):
    raise SystemExit("SAFE_BLOCKED: recorder prestate is not an ExecStart-only drop-in")
executable = pathlib.Path(lines[2].split("=",1)[1])
_, executable_record = record(executable, 0o755)
if pid != "0":
    actual = pathlib.Path(proc_root) / pid / "exe"
    metadata = actual.stat()
    if (actual.resolve(strict=True) != executable or metadata.st_dev != executable_record["device"] or
            metadata.st_ino != executable_record["inode"] or hashlib.sha256(actual.read_bytes()).hexdigest() != executable_record["sha256"]):
        raise SystemExit("SAFE_BLOCKED: running recorder differs from the prestate being preserved")
print(json.dumps({"schema":"codex-info-recorder-override-prestate-v1","operation_id":operation,
                  "generation":generation,"override":str(override),"override_record":override_record,
                  "executable":str(executable),"executable_record":executable_record}, separators=(",",":")))
PY_RECORDER_PRESTATE
    )" || safe_blocked 'recorder prestate capture failed; configuration preserved'
    mkdir -p -- "$backup_dir"; chmod 700 -- "$backup_dir"
    atomic_text "$backup_dir/$operation_id-recorder-override-prestate.json" 600 "$content"
}
recorder_execution_record() {
    local generation="$1" properties info expected_hash
    [[ -e "$unit_destination" || -L "$unit_destination" ]] || { printf 'canonical\n'; return; }
    [[ -n "$generation" ]] || safe_blocked 'recorder ExecStart override has no product generation authority'
    properties="$(systemctl_user show --property=ExecStart --property=FragmentPath --property=DropInPaths codex-info-recorder.service)" ||
        safe_blocked 'recorder effective ExecStart cannot be inspected'
    info="$(manifest_record "$generations_dir/$generation/manifest.json")" || safe_blocked 'recorder ExecStart product manifest is unavailable'
    IFS=$'\t' read -r _ _ _ expected_hash <<<"$info"
    if [[ -e "$recorder_override_destination" || -L "$recorder_override_destination" ]]; then
        verify_generation_files "$generations_dir/$generation" || safe_blocked 'recorder override product provenance is unavailable'
    fi
    python3 - "$properties" "$unit_destination" "$recorder_override_destination" "$recorder_binary_destination" \
        "$share_dir/hotfixes/issue-134/codex_info_recorder" "$generations_dir/$generation" "$expected_hash" \
        "$migrate_recorder_override" "${rollback_recorder_receipt:-}" "${operation_id:-}" "$backup_dir" <<'PY_RECORDER_EXECUTION'
import hashlib, json, os, pathlib, re, stat, sys
properties, unit_name, override_name, canonical, hotfix_name, generation_name, expected_hash, selected, receipt_name, operation, backup_name = sys.argv[1:]
def reject(reason):
    raise SystemExit("SAFE_BLOCKED: recorder ExecStart override conflict; " + reason + "; configuration preserved")
fields = {}
for line in properties.splitlines():
    key, separator, value = line.partition("=")
    if not separator or key in fields: reject("effective unit properties are ambiguous")
    fields[key] = value
if set(fields) != {"ExecStart", "FragmentPath", "DropInPaths"}: reject("effective unit properties are incomplete")
unit, override, generation = map(pathlib.Path, (unit_name, override_name, generation_name))
try:
    if pathlib.Path(fields["FragmentPath"]).resolve(strict=True) != (generation / "codex-info-recorder.service").resolve(strict=True):
        reject("service fragment is not the installed product unit")
except OSError: reject("service fragment is unavailable")
match = re.fullmatch(r"\{\s*path=(.*?)\s*;\s*argv\[\]=(.*?)\s*;[^{}]*\}", fields["ExecStart"])
if match is None: reject("effective command is ambiguous")
executable, arguments = match.groups()
if arguments != executable: reject("custom command arguments are not product-owned")
start_overrides = []
for name in fields["DropInPaths"].split():
    dropin = pathlib.Path(name)
    try:
        section = ""
        for line in dropin.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line.startswith("["): section = line
            if section == "[Service]" and line.startswith("ExecStart="):
                start_overrides.append(dropin)
                break
    except OSError: reject("drop-in cannot be inspected")
if executable == canonical and not start_overrides:
    print("canonical")
    raise SystemExit(0)
if start_overrides != [override]:
    reject("command is not the supported single recorder drop-in")
hotfix = pathlib.Path(executable)
try:
    for candidate, mode in ((override, 0o644), (hotfix, 0o755)):
        metadata = candidate.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != mode:
            reject("override or payload owner/type/mode differs")
        if candidate.resolve(strict=True) != candidate: reject("override or payload path is indirect")
    if override.read_text(encoding="utf-8") != "[Service]\nExecStart=\nExecStart=" + executable + "\n":
        reject("override contains unsupported or mixed settings")
    payload_hash = hashlib.sha256(hotfix.read_bytes()).hexdigest()
    override_hash = hashlib.sha256(override.read_bytes()).hexdigest()
    if receipt_name:
        receipt = pathlib.Path(receipt_name)
        if receipt != pathlib.Path(backup_name) / (operation + "-recorder-override-prestate.json"):
            reject("rollback receipt path differs")
        metadata = receipt.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o600 or receipt.resolve(strict=True) != receipt:
            reject("rollback receipt owner/type/mode/path differs")
        document = json.loads(receipt.read_text(encoding="utf-8"))
        required = {"schema","operation_id","generation","override","override_record","executable","executable_record"}
        if (not isinstance(document,dict) or set(document) != required or document["schema"] != "codex-info-recorder-override-prestate-v1" or
                document["operation_id"] != operation or document["generation"] != generation.name or
                document["override"] != str(override) or document["executable"] != executable):
            reject("rollback receipt authority differs")
        for candidate, key, digest in ((override,"override_record",override_hash),(hotfix,"executable_record",payload_hash)):
            metadata = candidate.lstat()
            if document[key] != {"device":metadata.st_dev,"inode":metadata.st_ino,"sha256":digest}:
                reject("restored recorder prestate differs from its exact snapshot")
        print("restored", payload_hash, sep="\t")
        raise SystemExit(0)
    if executable == hotfix_name and payload_hash == expected_hash:
        print("legacy")
        raise SystemExit(0)
    plan = ("RECORDER_OVERRIDE_MIGRATION: drop-in=" + str(override) + "; from=" + executable +
            "; to=" + canonical + "; backup=" + backup_name + "; original payload retained")
    if selected == "1":
        print(plan, file=sys.stderr)
        print("selected")
        raise SystemExit(0)
    print(plan, file=sys.stderr)
    reject("override has no verified installed product provenance; RECORDER_OVERRIDE_MIGRATION_REQUIRED; select codex-info --update --migrate-recorder-override")
except (OSError, UnicodeError, ValueError, KeyError, TypeError): reject("override, payload, or rollback receipt is unavailable")
PY_RECORDER_EXECUTION
}
recorder_binary_identity_check() {
    local pid="$1" generation="$2" execution info expected_hash actual_hash before after record restored_hash
    record="$(recorder_execution_record "$generation")" || return 1
    IFS=$'\t' read -r execution restored_hash <<<"$record"
    [[ "$execution" == canonical || ( "$execution" == legacy && "${allow_legacy_recorder_override:-0}" == 1 ) ||
       ( "$execution" == restored && "${allow_legacy_recorder_override:-0}" == 1 && -n "${rollback_recorder_receipt:-}" ) ]] ||
        safe_blocked 'recorder effective ExecStart remains pinned by a product override'
    info="$(manifest_record "$generations_dir/$generation/manifest.json")" || return 1
    IFS=$'\t' read -r _ _ _ expected_hash <<<"$info"
    if [[ "$execution" == restored ]]; then expected_hash="$restored_hash"; fi
    [[ "$expected_hash" =~ ^[0-9a-f]{64}$ ]] || safe_blocked 'recorder verification digest is invalid'
    before="$(proc_starttime "$pid" 2>/dev/null || true)"
    [[ -n "$before" && "$(recorder_systemd_pid)" == "$pid" ]] || safe_blocked 'recorder MainPID/starttime is unavailable'
    actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
    [[ "$actual_hash" == "$expected_hash" ]] || safe_blocked 'running recorder executable digest is not the current product artifact'
    if [[ "$execution" == restored ]]; then
        # The classifier validated this operation's owner-private receipt.
        # Rollback restores its exact file; canonical reuse keeps the hash rule.
        if ! python3 - "$rollback_recorder_receipt" "$proc_root/$pid/exe" <<'PY_RESTORED_RECORDER_IDENTITY'
import json, pathlib, sys
document = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
actual = pathlib.Path(sys.argv[2])
metadata = actual.stat()
expected = document["executable_record"]
if (actual.resolve(strict=True) != pathlib.Path(document["executable"]) or
        metadata.st_dev != expected["device"] or metadata.st_ino != expected["inode"]):
    raise SystemExit("SAFE_BLOCKED: running recorder is not the exact restored prestate executable")
PY_RESTORED_RECORDER_IDENTITY
        then return 1; fi
    fi
    after="$(proc_starttime "$pid" 2>/dev/null || true)"
    [[ "$after" == "$before" && "$(recorder_systemd_pid)" == "$pid" ]] || safe_blocked 'recorder MainPID/starttime changed during artifact verification'
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
def read_object(path, keys, optional_keys=frozenset()):
    path = pathlib.Path(path)
    if not path.is_file() or path.is_symlink(): raise SystemExit("recorder identity file unavailable")
    metadata = path.stat()
    if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o600: raise SystemExit("recorder identity file is not owner-private")
    try: value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=pairs)
    except Exception as error: raise SystemExit(str(error))
    if not isinstance(value, dict) or not keys <= set(value) or set(value) - keys - optional_keys: raise SystemExit("recorder identity schema is invalid")
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
state = read_object(recorder_name, {"schema","pid","process_starttime","owner_nonce","write_state","partition_id_hash","data_generation","collector_epoch","cycle_seq","last_commit_unix","updated_at_unix"}, {"recorder_version"})
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
if write_state in {"ready", "degraded"}:
    if partition is None or any(state[key] is None for key in ("data_generation","collector_epoch","cycle_seq","last_commit_unix")): raise SystemExit("recorder state is incomplete")
    if state["last_commit_unix"] > now + 5 or now - state["last_commit_unix"] > 150: raise SystemExit("recorder commit is stale")
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
    expected_exe="$generations_dir/$version-$source-$manifest_hash/codex_info_rest"
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
if document.get("state") not in {"ready","auth_required","error"}:
    raise SystemExit("details state is unknown")
observed_at=document.get("observed_at")
if isinstance(observed_at,bool) or not isinstance(observed_at,int) or observed_at <= 0:
    raise SystemExit("details observed_at is invalid")
' <<< "$details"
    local recorder_pid
    recorder_pid="$(recorder_systemd_pid)"; [[ "$recorder_pid" != 0 ]] || safe_blocked 'recorder service has no MainPID'
    recorder_identity_check "$recorder_pid" "$version" "$source" "$manifest_hash"
    proc_identity_check "$pid" "$binary_hash" "$expected_exe"
}
verify_runtime() {
    verify_fixed_links
    legacy_combined_retired || safe_blocked 'legacy combined unit was not retired'
    [[ -L "$current_link" ]] || safe_blocked 'current generation is absent'
    local target generation info version source manifest_hash binary_hash rest_hash pid recorder_pid before
    target="$(readlink -- "$current_link")"
    [[ "$target" == generations/* && "$target" != */*/* ]] || safe_blocked 'current generation link is invalid'
    generation="${target#generations/}"
    [[ -d "$generations_dir/$generation" && ! -L "$generations_dir/$generation" ]] || safe_blocked 'current generation directory is invalid'
    info="$(manifest_record)"; IFS=$'\t' read -r version source manifest_hash binary_hash <<<"$info"
    rest_hash="$(manifest_rest_hash)" || safe_blocked 'REST binary digest is unavailable'
    [[ "$generation" == "$version-$source-$manifest_hash" ]] || safe_blocked 'generation identity mismatch'
    verify_generation_files "$generations_dir/$generation" || safe_blocked 'generation artifact set is incoherent'
    [[ "$(sha256sum -- "$recorder_binary_destination" | awk '{print $1}')" == "$binary_hash" ]] || safe_blocked 'installed recorder binary digest mismatch'
    [[ "$(sha256sum -- "$rest_binary_destination" | awk '{print $1}')" == "$rest_hash" ]] || safe_blocked 'installed REST binary digest mismatch'
    probe_active codex-info-recorder.service || safe_blocked 'recorder service is inactive'
    probe_active codex-info-rest.service || safe_blocked 'REST service is inactive'
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || safe_blocked 'REST service has no MainPID'
    before="$(proc_starttime "$pid" 2>/dev/null || true)"; [[ -n "$before" ]] || safe_blocked 'MainPID starttime unavailable'
    recorder_pid="$(recorder_systemd_pid)"; [[ "$recorder_pid" != 0 ]] || safe_blocked 'recorder service has no MainPID'
    recorder_binary_identity_check "$recorder_pid" "$generation" || return 1
    health_readback "$pid" "$before" "$version" "$source" "$manifest_hash" "$rest_hash"
    printf 'ready version=%s source=%s generation=%s pid=%s\n' "$version" "$source" "$generation" "$pid"
}
verify_ui_source() {
    local target generation info version source manifest_hash binary_hash rest_hash pid expected_exe resolved listener_pid
    verify_fixed_links_local || return 1
    [[ -L "$current_link" ]] || return 1
    target="$(readlink -- "$current_link")"
    [[ "$target" == generations/* && "$target" != */*/* ]] || return 1
    generation="${target#generations/}"
    [[ -d "$generations_dir/$generation" && ! -L "$generations_dir/$generation" ]] || return 1
    info="$(manifest_record)" || return 1
    IFS=$'\t' read -r version source manifest_hash binary_hash <<<"$info"
    rest_hash="$(manifest_rest_hash)" || return 1
    [[ "$generation" == "$version-$source-$manifest_hash" ]] || return 1
    verify_generation_files "$generations_dir/$generation" || return 1
    if ! probe_active codex-info-rest.service; then
        listener_pid="$(socket_pid 2>/dev/null || true)"
        [[ -z "$listener_pid" ]]
        return
    fi
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || return 1
    expected_exe="$generations_dir/$generation/codex_info_rest"
    resolved="$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)"
    [[ "$resolved" == "$expected_exe" ]] || return 1
    [[ "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$expected_exe" 2>/dev/null || true)" ]] || return 1
    [[ "$(sha256sum -- "$resolved" 2>/dev/null | awk '{print $1}')" == "$rest_hash" ]] || return 1
    listener_pid="$(socket_pid 2>/dev/null || true)"
    [[ -z "$listener_pid" || "$listener_pid" == "$pid" ]]
}
verify_legacy_runtime() {
    local info version source manifest_hash binary_hash pid recorder_pid before after listener_pid
    info="$(legacy_flat_record)" || return 1
    IFS=$'\t' read -r version source manifest_hash binary_hash _ <<<"$info"
    probe_active codex-info-recorder.service || return 1
    probe_active codex-info-rest.service || return 1
    pid="$(systemd_pid)"; [[ "$pid" != 0 ]] || return 1
    [[ "$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)" == "$rest_binary_destination" ]] || return 1
    [[ "$(stat -Lc '%d:%i' -- "$proc_root/$pid/exe" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$rest_binary_destination" 2>/dev/null || true)" ]] || return 1
    [[ "$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}')" == "$(manifest_rest_hash "$manifest_destination")" ]] || return 1
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
    recorder_pid="$(recorder_systemd_pid)"; [[ "$recorder_pid" != 0 ]] || return 1
    recorder_identity_check "$recorder_pid" "$version" "$source" "$manifest_hash"
}
verify_legacy_terminal() {
    legacy_flat_record >/dev/null || return 1
    [[ ! -L "$current_link" ]] || return 1
    if [[ "$desired_state" == running ]]; then
        verify_legacy_runtime
    else
        ! probe_active codex-info-recorder.service || return 1
        ! probe_active codex-info-rest.service || return 1
        [[ -z "$(socket_pid 2>/dev/null || true)" ]]
    fi
}
rearm_update_timer() {
    if probe_active codex-info-update.timer; then
        systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || return 1
        wait_inactive codex-info-update.timer || return 1
    fi
    enable_managed_unit codex-info-update.timer || return 1
    systemctl_user start --no-block codex-info-update.timer >/dev/null 2>&1 || return 1
}
reset_failed_main() {
    systemctl_user reset-failed codex-info-recorder.service >/dev/null 2>&1
    systemctl_user reset-failed codex-info-rest.service >/dev/null 2>&1
}
repair_known_managed_runtime() {
    local pid resolved expected generation_path current_path current_info expected_hash actual_hash
    probe_active codex-info-recorder.service || return 0
    [[ "$TRIGGER" != startup ]] || return 0
    pid="$(recorder_systemd_pid)"; [[ "$pid" != 0 ]] || safe_blocked 'managed recorder service has no MainPID'
    current_path="$(readlink -f -- "$current_link" 2>/dev/null || true)"
    [[ "$current_path" == "$generations_dir/"* ]] || safe_blocked 'current generation path is unavailable for runtime repair'
    expected="$current_path/codex_info_recorder"
    resolved="$(readlink -f -- "$proc_root/$pid/exe" 2>/dev/null || true)"
    if [[ "$resolved" == "$expected" ]]; then
        current_info="$(manifest_record "$current_path/manifest.json")" || safe_blocked 'current generation manifest is unavailable for runtime repair'
        IFS=$'\t' read -r _ _ _ expected_hash <<<"$current_info"
        actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        [[ "$actual_hash" == "$expected_hash" ]] || safe_blocked 'managed current executable digest differs'
        return 0
    fi
    if [[ "$resolved" == "$generations_dir/"*"/codex_info_recorder" ]]; then
        generation_path="${resolved%/codex_info_recorder}"
        [[ "$(dirname -- "$generation_path")" == "$generations_dir" ]] || safe_blocked 'managed runtime generation path is invalid'
        verify_generation_files "$generation_path" || safe_blocked 'known managed runtime generation is incoherent'
        actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        current_info="$(manifest_record "$generation_path/manifest.json")" || safe_blocked 'known managed runtime manifest is unavailable'
        IFS=$'\t' read -r _ _ _ expected_hash <<<"$current_info"
        [[ "$actual_hash" == "$expected_hash" ]] || safe_blocked 'known managed runtime digest differs'
    elif [[ "$resolved" == "$recorder_binary_destination" ]]; then
        legacy_flat_record >/dev/null || safe_blocked 'legacy managed runtime is not trusted'
        actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
        expected_hash="$(sha256sum -- "$recorder_binary_destination" 2>/dev/null | awk '{print $1}' || true)"
        [[ -n "$actual_hash" && "$actual_hash" == "$expected_hash" ]] || safe_blocked 'legacy managed runtime digest differs'
    else
        safe_blocked 'foreign managed service executable blocks runtime repair'
    fi
    reset_failed_main || safe_blocked 'could not reset failed managed service for runtime repair'
    systemctl_user restart --no-block codex-info-recorder.service >/dev/null 2>&1 || safe_blocked 'could not restart recorder service for runtime repair'
    systemctl_user restart --no-block codex-info-rest.service >/dev/null 2>&1 || safe_blocked 'could not restart REST service for runtime repair'
}
verify_nonrunning_terminal() {
    local desired="$1" listener_pid
    legacy_combined_retired || return 1
    if [[ "$desired" == removed ]]; then
        unit_inactive_or_absent codex-info-recorder.service || return 1
        unit_inactive_or_absent codex-info-rest.service || return 1
    else
        probe_active codex-info-recorder.service && return 1
        probe_active codex-info-rest.service && return 1
    fi
    listener_pid="$(socket_pid 2>/dev/null || true)"
    [[ -z "$listener_pid" ]] || return 1
    verify_local_generation || return 1
    case "$desired" in
        stopped)
            probe_enabled codex-info-recorder.service || return 1
            probe_enabled codex-info-rest.service || return 1
            probe_enabled codex-info-update.timer || return 1
            probe_active codex-info-update.timer || return 1
            ;;
        disabled)
            probe_enabled codex-info-recorder.service && return 1
            probe_enabled codex-info-rest.service && return 1
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
            [[ -L "$binary_destination" && -L "$recorder_binary_destination" && -L "$rest_binary_destination" && -L "$launcher_destination" && -L "$installer_destination" && -L "$manifest_destination" ]] || return 1
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
    probe_enabled codex-info-recorder.service && main_enabled=1 || true; probe_active codex-info-recorder.service && main_active=1 || true
    rest_enabled=0; rest_active=0
    probe_enabled codex-info-rest.service && rest_enabled=1 || true; probe_active codex-info-rest.service && rest_active=1 || true
    probe_enabled codex-info-update.timer && timer_enabled=1 || true; probe_active codex-info-update.timer && timer_active=1 || true
}
capture_legacy_combined_state() {
    legacy_combined_enabled=0; legacy_combined_active=0; legacy_combined_generation=0
    legacy_combined_record >/dev/null || safe_blocked 'legacy combined predecessor is not trusted'
    validate_legacy_combined_enable_link
    probe_legacy_combined_enabled && legacy_combined_enabled=1 || true
    probe_active codex-info.service && legacy_combined_active=1 || true
    [[ -L "$legacy_combined_unit_destination" ]] && legacy_combined_generation=1
    return 0
}
legacy_recovery_reader_source() {
    printf '%s\n' "$running_installer_source"
}
legacy_recovery_reader_valid() {
    local expected="${1:-$legacy_recovery_reader_hash}"
    [[ -n "$expected" && -f "$legacy_recovery_reader_destination" && ! -L "$legacy_recovery_reader_destination" &&
       "$(stat -c '%u:%a' -- "$legacy_recovery_reader_destination")" == "$(id -u):755" &&
       "$(sha256sum -- "$legacy_recovery_reader_destination" | awk '{print $1}')" == "$expected" ]]
}
legacy_recovery_reader_active() {
    [[ -n "${legacy_recovery_reader_hash:-}" && -L "$installer_destination" &&
       "$(readlink -- "$installer_destination")" == "$legacy_recovery_reader_destination" &&
       "$(stat -c '%u' -- "$installer_destination")" == "$(id -u)" ]] && legacy_recovery_reader_valid
}
atomic_recovery_copy() {
    python3 - "$1" "$2" "$3" "${4:-}" "${5:-}" <<'PY'
import base64, gzip, hashlib, io, os, pathlib, sys, tempfile
source, destination = map(pathlib.Path, sys.argv[1:3])
expected, previous, reader_image = sys.argv[3:]
if reader_image:
    if len(reader_image)>65536: raise SystemExit("recovery image is too large")
    with gzip.GzipFile(fileobj=io.BytesIO(base64.b64decode(reader_image, validate=True))) as image:
        data = image.read(524289)
    if len(data)>524288: raise SystemExit("recovery image is too large")
else:
    if not source.is_file() or source.is_symlink() or source.stat().st_uid != os.getuid():
        raise SystemExit("recovery source is not owned regular bytes")
    data = source.read_bytes()
if hashlib.sha256(data).hexdigest() != expected:
    raise SystemExit("recovery source identity changed")
if destination.exists() or destination.is_symlink():
    if (destination.is_symlink() or not destination.is_file() or destination.stat().st_uid != os.getuid() or
            (destination.stat().st_mode & 0o7777) != 0o755 or not previous or
            hashlib.sha256(destination.read_bytes()).hexdigest() != previous):
        raise SystemExit("foreign recovery destination")
fd, temporary = tempfile.mkstemp(prefix=".codex-info.", dir=destination.parent)
try:
    with os.fdopen(fd, "wb") as output:
        output.write(data); output.flush(); os.fsync(output.fileno())
    os.chmod(temporary, 0o755); os.replace(temporary, destination)
    fd = os.open(destination.parent, os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
finally:
    try: os.unlink(temporary)
    except FileNotFoundError: pass
PY
}
# One bounded executable cache; all prestate remains in the journal operation ID.
# Keep the cache callable through startup and rollback, including the running caller.
# The same prepared journal is staged until the new reader owns the installed
# entrypoint, then renamed into the canonical path. The predecessor never sees
# a pending transaction it cannot recover. This is not a second member ledger.
validate_legacy_handoff_destination() {
    local transaction="$1"
    local journal_line journal_phase journal_operation_id journal_owner_pid journal_owner_starttime
    local journal_boot_id journal_previous_id journal_candidate_id journal_desired
    local journal_legacy_combined_prestate journal_legacy_recovery_reader_hash
    [[ -e "$transaction" || -L "$transaction" ]] || return 0
    read_journal
    [[ "$journal_phase" == committed ]] || safe_blocked 'another transaction blocks legacy reader handoff'
}

stage_legacy_recovery_handoff() {
    validate_legacy_handoff_destination "$transaction"
    [[ ! -e "$legacy_recovery_journal" && ! -L "$legacy_recovery_journal" ]] ||
        safe_blocked 'legacy reader handoff journal already exists'
    local legacy_recovery_reader_image
    legacy_recovery_reader_image="$(python3 - "$(legacy_recovery_reader_source)" "$legacy_recovery_reader_hash" <<'PY'
import base64, gzip, hashlib, os, pathlib, sys
source = pathlib.Path(sys.argv[1])
if not source.is_file() or source.is_symlink() or source.stat().st_uid!=os.getuid():
    raise SystemExit("legacy reader source is not owned regular bytes")
data = source.read_bytes()
if len(data)>524288 or hashlib.sha256(data).hexdigest()!=sys.argv[2]:
    raise SystemExit("legacy reader source identity changed")
image = base64.b64encode(gzip.compress(data, mtime=0)).decode()
if len(image)>65536: raise SystemExit("legacy reader image is too large")
print(image)
PY
    )" || safe_blocked 'legacy reader image could not be preserved'
    local transaction="$legacy_recovery_journal" CODEX_INFO_INTERRUPT_PHASE=
    write_journal prepared
}

promote_legacy_recovery_handoff() {
    validate_legacy_handoff_destination "$transaction"
    (
        local transaction="$legacy_recovery_journal"
        read_journal
        [[ "$journal_phase" == prepared && "$journal_operation_id" == "$operation_id" &&
           -n "$journal_legacy_combined_prestate" &&
           "$journal_legacy_recovery_reader_hash" == "$legacy_recovery_reader_hash" ]] ||
            safe_blocked 'legacy reader handoff identity is invalid'
    ) || safe_blocked 'legacy reader handoff journal is invalid'
    legacy_recovery_reader_active || safe_blocked 'legacy reader handoff entrypoint is unavailable'
    seal_legacy_reader_handoff
    python3 - "$legacy_recovery_journal" "$transaction" <<'PY'
import os, pathlib, sys
source, destination = map(pathlib.Path, sys.argv[1:])
os.replace(source, destination)
fd = os.open(destination.parent, os.O_DIRECTORY)
try: os.fsync(fd)
finally: os.close(fd)
PY
}

recover_legacy_reader_handoff() {
    local canonical_transaction="$transaction" transaction="$legacy_recovery_journal"
    read_journal
    [[ "$journal_phase" == prepared && -n "$journal_legacy_combined_prestate" &&
       -n "$journal_legacy_recovery_reader_hash" ]] || safe_blocked 'legacy reader handoff state is invalid'
    journal_owner_stale || safe_blocked 'legacy reader handoff owner is still live'
    validate_legacy_handoff_destination "$canonical_transaction"
    operation_id="$journal_operation_id"; candidate_id="$journal_candidate_id"
    previous_id="$journal_previous_id"; desired_state="$journal_desired"
    require_user_manager
    recover_legacy_combined_state
    prepare_legacy_recovery_reader
    publish_legacy_recovery_reader
    transaction="$canonical_transaction"
    promote_legacy_recovery_handoff
}

# Commit linearizes the verified old runtime while the recovery reader still
# protects startup. Only its final entrypoint switch remains; on interruption,
# finish that switch under L1 without publishing or committing the operation again.
finish_committed_legacy_recovery() {
    [[ -n "${journal_legacy_recovery_reader_hash:-}" && -L "$installer_destination" &&
       "$(readlink -- "$installer_destination")" == "$legacy_recovery_reader_destination" ]] || return 0
    operation_id="$journal_operation_id"; candidate_id="$journal_candidate_id"
    previous_id="$journal_previous_id"; desired_state="$journal_desired"
    capture_runtime_state
    recover_legacy_combined_state
    [[ "$(current_generation)" == "$previous_id" ]] || safe_blocked 'committed legacy recovery generation changed'
    restore_legacy_combined_runtime || safe_blocked 'committed legacy runtime could not be restored'
    verify_legacy_combined_terminal || safe_blocked 'committed legacy runtime is not ready'
    restore_legacy_recovery_installer
    verify_legacy_combined_terminal || safe_blocked 'committed legacy snapshot is incomplete'
    transaction_recovered=1
}

legacy_recovery_reader_image() {
    (
        local transaction="$legacy_recovery_journal"
        read_journal
        [[ "$journal_phase" == prepared && "$journal_operation_id" == "$operation_id" &&
           "$journal_legacy_recovery_reader_hash" == "$legacy_recovery_reader_hash" ]] ||
            safe_blocked 'legacy reader image belongs to another operation'
        printf '%s\n' "$journal_legacy_recovery_reader_image"
    )
}

legacy_recovery_reader_prior_hash() {
    (
        local transaction="$share_dir/install-transaction.json"
        [[ -e "$transaction" || -L "$transaction" ]] || return 0
        read_journal
        [[ "$journal_phase" == committed ]] || safe_blocked 'unsettled journal blocks cache replacement'
        printf '%s\n' "$journal_legacy_recovery_reader_hash"
    )
}

# Once the cache and its installed binding are durable, strip the temporary
# executable image before promotion. The canonical journal always has v1 keys.
seal_legacy_reader_handoff() {
    legacy_recovery_reader_active || safe_blocked 'legacy reader is not durable before journal promotion'
    local content
    content="$(python3 - "$legacy_recovery_journal" <<'PY'
import json, pathlib, sys
document = json.loads(pathlib.Path(sys.argv[1]).read_text())
document.pop("legacy_reader_image", None)
print(json.dumps(document, ensure_ascii=False, indent=2)+"\n", end="")
PY
    )" || safe_blocked 'legacy reader handoff could not be sealed'
    atomic_text "$legacy_recovery_journal" 600 "$content"
}

prepare_legacy_recovery_reader() {
    [[ -n "$legacy_recovery_reader_hash" ]] || return 0
    legacy_recovery_reader_valid && return 0
    local previous= reader_image=
    if [[ -e "$legacy_recovery_reader_destination" || -L "$legacy_recovery_reader_destination" ]]; then
        previous="$(legacy_recovery_reader_prior_hash)" || safe_blocked 'prior legacy reader authority is invalid'
        legacy_recovery_reader_valid "$previous" || safe_blocked 'foreign legacy recovery executable'
    fi
    if [[ -e "$legacy_recovery_journal" || -L "$legacy_recovery_journal" ]]; then
        reader_image="$(legacy_recovery_reader_image)" || safe_blocked 'legacy reader image is unavailable'
    fi
    atomic_recovery_copy "$(legacy_recovery_reader_source)" "$legacy_recovery_reader_destination" "$legacy_recovery_reader_hash" "$previous" "$reader_image" ||
        safe_blocked 'legacy recovery executable could not be preserved'
}
validate_legacy_recovery_installer_binding() {
    legacy_recovery_reader_active && return 0
    if [[ -L "$installer_destination" ]]; then
        [[ "$(readlink -- "$installer_destination")" == '../share/codex-info/current/install.sh' &&
           "$(stat -c '%u' -- "$installer_destination")" == "$(id -u)" ]] || return 1
        local generation; generation="$(current_generation)"
        [[ "$generation" == "$previous_id" || "$generation" == "$candidate_id" ]] && [[ -n "$generation" ]]
    elif (( ! legacy_combined_generation )); then
        local info manifest_hash
        info="$(legacy_combined_record)" || return 1
        IFS=$'\t' read -r _ _ manifest_hash _ _ <<<"$info"
        [[ "$manifest_hash" == "$legacy_combined_manifest_hash" ]]
    else
        return 1
    fi
}
publish_legacy_recovery_reader() {
    [[ -n "$legacy_recovery_reader_hash" ]] || return 0
    legacy_recovery_reader_valid || safe_blocked 'legacy recovery executable identity is unavailable'
    validate_legacy_recovery_installer_binding || safe_blocked 'foreign legacy installer blocks recovery'
    if (( ! legacy_combined_generation )) && ! legacy_recovery_reader_active; then
        local backup="$backup_dir/$operation_id-$(basename -- "$installer_destination")"
        if [[ ! -e "$backup" && ! -L "$backup" ]]; then
            mkdir -p -- "$backup_dir"; chmod 700 -- "$backup_dir"
            atomic_recovery_copy "$installer_destination" "$backup" "$(sha256sum -- "$installer_destination" | awk '{print $1}')" ||
                safe_blocked 'legacy installer prestate could not be preserved'
        fi
    fi
    atomic_symlink "$legacy_recovery_reader_destination" "$installer_destination"
}
restore_legacy_recovery_installer() {
    [[ -n "$legacy_recovery_reader_hash" ]] || return 0
    legacy_recovery_reader_active || safe_blocked 'foreign legacy installer blocks final restore'
    if (( legacy_combined_generation )); then
        atomic_symlink '../share/codex-info/current/install.sh' "$installer_destination"
    else
        validate_legacy_combined_recovery
        python3 - "$backup_dir/$operation_id-$(basename -- "$installer_destination")" "$installer_destination" <<'PY'
import os, pathlib, sys
source, destination = map(pathlib.Path, sys.argv[1:])
os.replace(source, destination)
for parent in {source.parent, destination.parent}:
    fd = os.open(parent, os.O_DIRECTORY)
    try: os.fsync(fd)
    finally: os.close(fd)
PY
    fi
}
capture_legacy_combined_prestate() {
    capture_legacy_combined_state
    local info manifest_hash
    info="$(legacy_combined_record)" || safe_blocked 'legacy combined identity is unavailable'
    IFS=$'\t' read -r _ _ manifest_hash _ _ <<<"$info"
    legacy_combined_prestate="$(python3 - "$legacy_combined_generation" "$legacy_combined_enabled" "$legacy_combined_active" "$manifest_hash" <<'PY'
import json, re, sys
generation, enabled, active, manifest_hash = sys.argv[1:]
if not re.fullmatch(r"[0-9a-f]{64}", manifest_hash): raise SystemExit("legacy manifest identity is invalid")
print(json.dumps({"generation": generation == "1", "enabled": enabled == "1",
                  "active": active == "1", "manifest_sha256": manifest_hash}))
PY
    )" || safe_blocked 'legacy combined prestate could not be captured'
    local source; source="$(legacy_recovery_reader_source)"
    legacy_recovery_reader_hash="$(sha256sum -- "$source" | awk '{print $1}')" || safe_blocked 'running installer identity is unavailable'
    [[ "$operation_id" =~ ^[A-Za-z0-9_-]{1,64}$ && "$legacy_recovery_reader_hash" =~ ^[0-9a-f]{64}$ ]] ||
        safe_blocked 'legacy recovery operation identity is invalid'
    local encoded_identity
    encoded_identity="$(python3 - "$manifest_hash" "$legacy_recovery_reader_hash" <<'PY'
import base64, sys
print("~".join(base64.urlsafe_b64encode(bytes.fromhex(value)).decode().rstrip("=") for value in sys.argv[1:]))
PY
    )" || safe_blocked 'legacy recovery operation identity could not be encoded'
    operation_id+="~lc1~$legacy_combined_generation$legacy_combined_enabled$legacy_combined_active~$encoded_identity"
}
validate_legacy_combined_recovery() {
    local destination backup expected info manifest_hash generation_path
    local -a members=("$manifest_destination" "$binary_destination" "$installer_destination" "$legacy_combined_unit_destination" "$update_service_destination" "$update_timer_destination")
    local -a restore_paths=()
    if (( legacy_combined_generation )); then
        generation_path="$generations_dir/$previous_id"
        # A trusted generation does not authorize overwriting a foreign unit.
        # Validate both the live binding and the same operation's pending move.
        expected='../../../.local/share/codex-info/current/codex-info.service'
        for destination in "$legacy_combined_unit_destination" "$backup_dir/$operation_id-codex-info.service"; do
            [[ -e "$destination" || -L "$destination" ]] || continue
            [[ -L "$destination" && "$(stat -c '%u' -- "$destination")" == "$(id -u)" ]] &&
                { [[ "$(readlink -- "$destination")" == "$expected" ]] ||
                  [[ "$(readlink -f -- "$destination")" == "$generation_path/codex-info.service" ]]; } ||
                safe_blocked 'foreign legacy combined unit blocks recovery'
        done
        members=("$generation_path/manifest.json" "$generation_path/codex_info" "$generation_path/install.sh" "$generation_path/codex-info.service" "$generation_path/codex-info-update.service" "$generation_path/codex-info-update.timer")
    fi
    for destination in "${members[@]}"; do
        backup="$backup_dir/$operation_id-$(basename -- "$destination")"
        if (( ! legacy_combined_generation )) && [[ -e "$backup" || -L "$backup" ]]; then
            if [[ -e "$destination" || -L "$destination" ]]; then
                case "$destination" in
                    "$manifest_destination") expected='current/manifest.json' ;;
                    "$binary_destination") expected='../share/codex-info/current/codex_info' ;;
                    "$installer_destination") expected='../share/codex-info/current/install.sh' ;;
                    *) expected="../../../.local/share/codex-info/current/$(basename -- "$destination")" ;;
                esac
                if [[ "$destination" == "$installer_destination" ]] &&
                    { legacy_recovery_reader_active ||
                      { [[ -f "$destination" && ! -L "$destination" && "$(stat -c '%u:%a' -- "$destination")" == "$(id -u):755" ]] &&
                        cmp -s -- "$destination" "$backup"; }; }; then
                    :
                else
                    [[ -L "$destination" && "$(readlink -- "$destination")" == "$expected" ]] ||
                        safe_blocked 'foreign legacy destination blocks recovery'
                fi
            fi
            restore_paths+=("$backup")
        else
            restore_paths+=("$destination")
        fi
    done
    info="$(legacy_combined_record_at "${restore_paths[@]}")" || safe_blocked 'legacy combined recovery snapshot is not trusted'
    IFS=$'\t' read -r _ _ manifest_hash _ _ <<<"$info"
    [[ "$manifest_hash" == "$legacy_combined_manifest_hash" ]] || safe_blocked 'legacy combined recovery identity changed'
    if [[ -n "$legacy_recovery_reader_hash" ]]; then
        validate_legacy_recovery_installer_binding || safe_blocked 'foreign legacy installer blocks recovery'
    fi
    validate_legacy_combined_enable_link
}
wait_legacy_combined_ready() {
    local now deadline remaining health details pid
    now="$(now_unix)" || return 1
    deadline=$((now + HEALTH_TIMEOUT))
    if (( operation_deadline > 0 && operation_deadline < deadline )); then deadline=$operation_deadline; fi
    while :; do
        now="$(now_unix)" || return 1; remaining=$((deadline - now))
        (( remaining > 0 )) || return 1
        if probe_active codex-info.service && legacy_combined_listener_matches; then
            pid="$(systemctl_user show codex-info.service --property=MainPID --value)" || return 1
            if [[ "$pid" == "$(socket_pid)" ]]; then
                health="$("$CURL_BIN" --fail --silent --show-error --connect-timeout 2 --max-time "$remaining" "$HEALTH_URL")" || health=
                now="$(now_unix)" || return 1; remaining=$((deadline - now))
                (( remaining > 0 )) || return 1
                details="$("$CURL_BIN" --fail --silent --show-error --connect-timeout 2 --max-time "$remaining" "$DETAILS_URL")" || details=
                if python3 - "$health" "$details" "$legacy_combined_version" <<'PY'
import json, sys
def pairs(items):
    value = {}
    for key, item in items:
        if key in value: raise ValueError("duplicate runtime key")
        value[key] = item
    return value
try:
    health, details = (json.loads(raw, object_pairs_hook=pairs) for raw in sys.argv[1:3])
    valid = (health == {"api_version": "v1", "service": "codex-info", "product_version": sys.argv[3]} and
             isinstance(details, dict) and details.get("state") in {"ready", "auth_required"} and
             type(details.get("observed_at")) is int and details["observed_at"] > 0)
except (ValueError, TypeError):
    valid = False
raise SystemExit(0 if valid else 1)
PY
                then
                    [[ "$(systemctl_user show codex-info.service --property=MainPID --value)" == "$pid" && "$(socket_pid)" == "$pid" ]] &&
                        legacy_combined_listener_matches && return 0
                fi
            fi
        fi
        sleep_interval 1
    done
}
legacy_combined_mixed_split_present() {
    local path
    for path in "$recorder_binary_destination" "$rest_binary_destination" "$unit_destination" "$rest_unit_destination"; do
        [[ -e "$path" || -L "$path" ]] && return 0
    done
    return 1
}
legacy_combined_listener_matches() {
    local listener_pid resolved info expected actual
    listener_pid="$(socket_pid 2>/dev/null || true)"
    [[ -n "$listener_pid" ]] || return 1
    resolved="$(readlink -f -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)"
    info="$(legacy_combined_owner_record "$resolved" 2>/dev/null || true)"
    [[ -n "$info" ]] || return 1
    IFS=$'\t' read -r _ _ _ expected _ <<<"$info"
    [[ "$(stat -Lc '%d:%i' -- "$proc_root/$listener_pid/exe" 2>/dev/null || true)" == "$(stat -Lc '%d:%i' -- "$resolved" 2>/dev/null || true)" ]] || return 1
    actual="$(sha256sum -- "$proc_root/$listener_pid/exe" 2>/dev/null | awk '{print $1}' || true)"
    [[ -n "$actual" && "$actual" == "$expected" ]]
}
retire_legacy_combined() {
    local managed_pid="$1" listener_pid
    legacy_combined_present || return 0
    (( previous_combined )) && return 0
    [[ -n "$legacy_combined_prestate" ]] || safe_blocked 'legacy retirement lacks durable prestate'
    previous_combined=1
    if (( legacy_combined_active )); then
        systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 ||
            safe_blocked 'legacy combined service could not be stopped'
        wait_inactive codex-info.service || safe_blocked 'legacy combined service did not stop'
    fi
    if (( legacy_combined_enabled )); then
        systemctl_user disable --no-block codex-info.service >/dev/null 2>&1 ||
            safe_blocked 'legacy combined service could not be disabled'
        probe_legacy_combined_enabled && safe_blocked 'legacy combined service remained enabled'
    fi
    if [[ -e "$legacy_combined_enable_destination" || -L "$legacy_combined_enable_destination" ]]; then
        validate_legacy_combined_enable_link
        atomic_unlink "$legacy_combined_enable_destination"
    fi
    if probe_active codex-info.service; then
        systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 ||
            safe_blocked 'legacy combined service restarted during retirement'
        wait_inactive codex-info.service || safe_blocked 'legacy combined service restarted during retirement'
    fi
    listener_pid="$(socket_pid 2>/dev/null || true)"
    if [[ -n "$listener_pid" ]]; then
        legacy_combined_listener_matches || safe_blocked 'legacy combined listener remained after stop'
        retire_known_unmanaged "$managed_pid"
    fi
    [[ -z "$(socket_pid 2>/dev/null || true)" ]] || safe_blocked 'legacy combined listener could not be retired'
    backup_legacy_combined_unit
}
restore_legacy_combined_entrypoints() {
    (( previous_combined && legacy_combined_generation )) || return 0
    atomic_symlink '../share/codex-info/current/codex_info' "$binary_destination"
    atomic_symlink '../share/codex-info/current/run.sh' "$launcher_destination"
    if [[ -z "${legacy_recovery_reader_hash:-}" ]]; then atomic_symlink '../share/codex-info/current/install.sh' "$installer_destination"; fi
    atomic_symlink 'current/manifest.json' "$manifest_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info.service' "$legacy_combined_unit_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-update.service' "$update_service_destination"
    atomic_symlink '../../../.local/share/codex-info/current/codex-info-update.timer' "$update_timer_destination"
}
restore_legacy_combined_runtime() {
    local split_unit
    for split_unit in codex-info-recorder.service codex-info-rest.service codex-info-update.timer; do
        if probe_active "$split_unit"; then
            systemctl_stop_user stop --no-block "$split_unit" >/dev/null 2>&1 || return 1
            wait_inactive "$split_unit" || return 1
        fi
        disable_managed_unit "$split_unit" || return 1
    done
    if (( legacy_combined_enabled )) && [[ "$desired_state" != disabled && "$desired_state" != removed ]]; then
        if [[ -e "$legacy_combined_enable_destination" || -L "$legacy_combined_enable_destination" ]]; then
            validate_legacy_combined_enable_link
        fi
        atomic_symlink '../codex-info.service' "$legacy_combined_enable_destination"
        systemctl_user daemon-reload >/dev/null 2>&1 || return 1
        probe_legacy_combined_enabled || return 1
    else
        if [[ -e "$legacy_combined_enable_destination" || -L "$legacy_combined_enable_destination" ]]; then
            validate_legacy_combined_enable_link
            atomic_unlink "$legacy_combined_enable_destination"
        fi
        ! probe_legacy_combined_enabled || return 1
    fi
    if [[ "$desired_state" == running ]]; then
        systemctl_user start --no-block codex-info.service >/dev/null 2>&1 || return 1
        probe_active codex-info.service || return 1
    elif probe_active codex-info.service; then
        systemctl_stop_user stop --no-block codex-info.service >/dev/null 2>&1 || return 1
        wait_inactive codex-info.service || return 1
    fi
    return 0
}
verify_legacy_combined_terminal() {
    local info
    info="$(legacy_combined_record)" || return 1
    IFS=$'\t' read -r legacy_combined_version _ legacy_combined_restored_hash _ _ <<<"$info"
    [[ "$legacy_combined_restored_hash" == "$legacy_combined_manifest_hash" ]] || return 1
    if (( legacy_combined_generation )); then
        [[ -n "$previous_id" && "$(current_generation)" == "$previous_id" ]] || return 1
    else
        [[ ! -L "$current_link" ]] || return 1
    fi
    if (( legacy_combined_enabled )) && [[ "$desired_state" != disabled && "$desired_state" != removed ]]; then
        probe_legacy_combined_enabled || return 1
    else
        ! probe_legacy_combined_enabled || return 1
    fi
    if [[ "$desired_state" == running ]]; then
        wait_legacy_combined_ready
    else
        ! probe_active codex-info.service || return 1
        [[ -z "$(socket_pid 2>/dev/null || true)" ]]
    fi
}
recover_legacy_combined_state() {
    previous_combined=0; legacy_combined_generation=0
    legacy_combined_prestate="${journal_legacy_combined_prestate:-}"
    legacy_recovery_reader_hash="${journal_legacy_recovery_reader_hash:-}"
    if [[ -n "$legacy_combined_prestate" ]]; then
        local state
        state="$(python3 - "$legacy_combined_prestate" <<'PY'
import json, sys
state = json.loads(sys.argv[1])
print(int(state["generation"]), int(state["enabled"]), int(state["active"]), state["manifest_sha256"], sep="\t")
PY
        )" || safe_blocked 'legacy combined prestate is unavailable'
        IFS=$'\t' read -r legacy_combined_generation legacy_combined_enabled legacy_combined_active legacy_combined_manifest_hash <<<"$state"
        previous_combined=1
        validate_legacy_combined_recovery
        return 0
    fi
    if legacy_combined_present || [[ -e "$backup_dir/$operation_id-codex-info.service" || -L "$backup_dir/$operation_id-codex-info.service" ]]; then
        safe_blocked 'legacy combined journal lacks trusted prestate'
    fi
}
recorder_artifact_matches_previous() {
    local candidate_hash="$1" previous_path previous_hash pid actual_hash
    [[ -n "$previous_id" && -n "$candidate_hash" ]] || return 1
    previous_path="$generations_dir/$previous_id"
    verify_generation_files "$previous_path" >/dev/null 2>&1 || return 1
    previous_path="$previous_path/codex_info_recorder"
    [[ -f "$previous_path" && ! -L "$previous_path" ]] || return 1
    previous_hash="$(sha256sum -- "$previous_path" | awk '{print $1}')" || return 1
    [[ "$previous_hash" == "$candidate_hash" ]] || return 1
    pid="$(recorder_systemd_pid)" || return 1
    actual_hash="$(sha256sum -- "$proc_root/$pid/exe" 2>/dev/null | awk '{print $1}' || true)"
    [[ "$actual_hash" == "$candidate_hash" ]]
}
enforce_desired_state() {
    if [[ "$desired_state" != running && "$main_active" == 1 ]]; then
        systemctl_stop_user stop --no-block codex-info-recorder.service >/dev/null 2>&1 || return 1
        wait_inactive codex-info-recorder.service || return 1
        main_active=0
    fi
    if [[ "$desired_state" != running && "$rest_active" == 1 ]]; then
        systemctl_stop_user stop --no-block codex-info-rest.service >/dev/null 2>&1 || return 1
        wait_inactive codex-info-rest.service || return 1
        rest_active=0
    fi
    if [[ "$desired_state" == disabled ]]; then
        if [[ "$timer_active" == 1 ]]; then
            systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || return 1
            wait_inactive codex-info-update.timer || return 1
            timer_active=0
        fi
        disable_managed_unit codex-info-update.timer || return 1
        timer_enabled=0
    fi
}
restore_runtime_state() {
    local failed=0
    if ((timer_enabled)); then enable_managed_unit codex-info-update.timer || failed=1; else disable_managed_unit codex-info-update.timer || failed=1; fi
    if ((timer_active)); then systemctl_user start --no-block codex-info-update.timer >/dev/null 2>&1 || failed=1; else systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || failed=1; wait_inactive codex-info-update.timer || failed=1; fi
    if ((main_enabled)); then enable_managed_unit codex-info-recorder.service || failed=1; else disable_managed_unit codex-info-recorder.service || failed=1; fi
    if ((main_active)); then
        if ((recorder_reused)); then
            probe_active codex-info-recorder.service || failed=1
        else
            systemctl_user restart --no-block codex-info-recorder.service >/dev/null 2>&1 || failed=1
        fi
    else
        systemctl_stop_user stop --no-block codex-info-recorder.service >/dev/null 2>&1 || failed=1
        wait_inactive codex-info-recorder.service || failed=1
    fi
    if ((rest_enabled)); then enable_managed_unit codex-info-rest.service || failed=1; else disable_managed_unit codex-info-rest.service || failed=1; fi
    if ((rest_active)); then systemctl_user restart --no-block codex-info-rest.service >/dev/null 2>&1 || failed=1; else systemctl_stop_user stop --no-block codex-info-rest.service >/dev/null 2>&1 || failed=1; wait_inactive codex-info-rest.service || failed=1; fi
    return "$failed"
}
rollback_transaction() {
    local previous="$1" reason="$2" ok=1 saved_deadline="$operation_deadline" rollback_now rollback_deadline
    if [[ -n "$update_stage" ]]; then update_stage=rollback; update_log started "$reason"; fi
    local allow_legacy_recorder_override=1
    local rollback_recorder_receipt=
    if [[ -f "$backup_dir/$operation_id-recorder-override-prestate.json" ]]; then
        rollback_recorder_receipt="$backup_dir/$operation_id-recorder-override-prestate.json"
    fi
    local overall_deadline="${install_deadline:-$operation_deadline}"
    rollback_now="$(now_unix)" || safe_blocked 'rollback clock is unavailable'
    rollback_deadline=$((rollback_now + ROLLBACK_TIMEOUT))
    if (( overall_deadline > 0 && overall_deadline < rollback_deadline )); then rollback_deadline=$overall_deadline; fi
    operation_deadline=$rollback_deadline
    if (( previous_combined )) && [[ -n "$legacy_recovery_reader_hash" ]]; then
        publish_legacy_recovery_reader
    fi
    if [[ -n "$previous" ]]; then atomic_symlink "generations/$previous" "$current_link" || ok=0; else atomic_unlink "$current_link" || ok=0; fi
    remove_published_entrypoints || ok=0
    restore_backups || ok=0
    previous_id="$previous"
    if (( previous_combined )); then
        restore_legacy_combined_entrypoints || ok=0
        ((ok)) && validate_legacy_combined_recovery || safe_blocked 'legacy combined restore is incomplete'
    else
        ensure_entrypoints_for_generation || ok=0
    fi
    write_journal rollback_switched "$reason" || ok=0
    systemctl_user daemon-reload >/dev/null 2>&1 || ok=0
    if (( previous_combined )); then
        restore_legacy_combined_runtime || ok=0
    else
        restore_runtime_state || ok=0
    fi
    if ((ok)) && [[ "$desired_state" != running && "$main_active" == 1 ]]; then
        systemctl_stop_user stop --no-block codex-info-recorder.service >/dev/null 2>&1 || ok=0
        wait_inactive codex-info-recorder.service || ok=0
        main_active=0
    fi
    if ((ok)) && [[ "$desired_state" != running && "$rest_active" == 1 ]]; then
        systemctl_stop_user stop --no-block codex-info-rest.service >/dev/null 2>&1 || ok=0
        wait_inactive codex-info-rest.service || ok=0
        rest_active=0
    fi
    if ((ok)); then
        if (( previous_combined )); then
            verify_legacy_combined_terminal || ok=0
        elif [[ -n "$previous" ]]; then
            if [[ "$desired_state" == running ]]; then
                if ! ((main_active)); then
                    enable_managed_unit codex-info-recorder.service || ok=0
                    enable_managed_unit codex-info-rest.service || ok=0
                    systemctl_user start --no-block codex-info-recorder.service >/dev/null 2>&1 || ok=0
                    systemctl_user start --no-block codex-info-rest.service >/dev/null 2>&1 || ok=0
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
    if ((ok)); then
        write_journal rollback_verified "$reason"
        write_journal committed rolled_back
        if (( previous_combined )) && [[ -n "$legacy_recovery_reader_hash" ]]; then
            restore_legacy_recovery_installer || ok=0
            ((ok)) && verify_legacy_combined_terminal || ok=0
        fi
    fi
    operation_deadline=$saved_deadline
    ((ok)) || safe_blocked "rollback could not be verified within $ROLLBACK_TIMEOUT seconds"
    update_log succeeded "$reason"
}
resume_transaction() {
    if [[ -e "${legacy_recovery_journal:-}" || -L "${legacy_recovery_journal:-}" ]]; then
        recover_legacy_reader_handoff
    fi
    [[ -f "$transaction" ]] || return 0
    read_journal
    if [[ "$journal_phase" == committed ]]; then
        if [[ -n "${journal_legacy_recovery_reader_hash:-}" ]]; then finish_committed_legacy_recovery; fi
        return 0
    fi
    transaction_recovered=1
    journal_owner_stale || safe_blocked 'transaction journal owner is still live'
    operation_id="$journal_operation_id"; candidate_id="$journal_candidate_id"; previous_id="$journal_previous_id"; desired_state="$journal_desired"
    candidate_quarantine="$backup_dir/$operation_id-generation-$candidate_id"
    [[ -e "$candidate_quarantine" || -L "$candidate_quarantine" ]] || candidate_quarantine=
    journal_owner_pid="$$"; journal_owner_starttime="$(owner_starttime)" || safe_blocked 'journal resume owner starttime is unavailable'; journal_boot_id="$(boot_id)"
    require_user_manager
    capture_runtime_state
    recover_legacy_combined_state
    if [[ -n "${legacy_recovery_reader_hash:-}" ]]; then prepare_legacy_recovery_reader; fi
    if [[ "$journal_phase" == current_switched || "$journal_phase" == activation_requested ]]; then
        if [[ "$(current_generation)" == "$candidate_id" ]] && verify_local_generation >/dev/null 2>&1 &&
            { [[ "$desired_state" != running ]] || (verify_runtime >/dev/null 2>&1); }; then
            converge_enable_links || safe_blocked 'candidate enable links could not be recovered'
            write_journal candidate_verified resumed-live-owner
            write_journal committed resumed
            return 0
        fi
    elif [[ "$journal_phase" == rollback_switched ]] && (( ! previous_combined )); then
        if [[ -n "$previous_id" ]]; then
            if [[ "$(current_generation)" == "$previous_id" ]] && verify_local_generation >/dev/null 2>&1 &&
                { [[ "$desired_state" != running ]] || (verify_runtime >/dev/null 2>&1); }; then
                converge_enable_links || safe_blocked 'rollback enable links could not be recovered'
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
    if [[ -n "$update_stage" ]]; then update_stage=activation; update_log started; fi
    local candidate_recorder_hash
    recorder_reused=0
    candidate_recorder_hash="$(manifest_record "$generations_dir/$candidate_id/manifest.json" | awk -F $'\t' '{print $4}')" || return 1
    [[ "$candidate_recorder_hash" =~ ^[0-9a-f]{64}$ ]] || return 1
    systemctl_user daemon-reload >/dev/null 2>&1 || return 1
    converge_enable_links || return 1
    if [[ "$desired_state" == stopped ]]; then
        rearm_update_timer
        return
    fi
    [[ "$desired_state" == running ]] || return 0
    reset_failed_main || return 1
    rearm_update_timer || return 1
    [[ "$TRIGGER" == startup ]] && return
    if ((main_active)); then
        if (( ! recorder_override_migrated )) && recorder_artifact_matches_previous "$candidate_recorder_hash"; then
            recorder_reused=1
        else
            systemctl_user restart --no-block codex-info-recorder.service >/dev/null 2>&1 || return 1
        fi
    else
        systemctl_user start --no-block codex-info-recorder.service >/dev/null 2>&1 || return 1
    fi
    if ((rest_active)); then systemctl_user restart --no-block codex-info-rest.service >/dev/null 2>&1 || return 1
    else systemctl_user start --no-block codex-info-rest.service >/dev/null 2>&1 || return 1; fi
}
verify_candidate() {
    if [[ -n "$update_stage" ]]; then update_stage=readiness; update_log started; fi
    verify_local_generation; [[ "$(current_generation)" == "$candidate_id" ]] || safe_blocked 'candidate is not current'
    if [[ "$desired_state" == running && "$TRIGGER" != startup ]]; then wait_runtime_ready; fi
}
perform_install() {
    local validation bundle_version source_hash manifest_hash binary_hash recorder_execution=canonical
    local install_deadline="$operation_deadline"
    local operation_deadline="$operation_deadline"
    local release_digest=
    if [[ "${CODEX_INFO_RELEASE_DIGEST_VERIFIED:-}" == 1 ]]; then
        release_digest="${CODEX_INFO_RELEASE_ARCHIVE_DIGEST:-}"
        [[ -n "$release_digest" ]] || die 'verified release archive digest is unavailable'
    fi
    candidate_stage="$(mktemp -d "$generations_dir/.candidate.XXXXXX")"
    validation="$(validate_bundle "$ARCHIVE" "$MANIFEST" "$candidate_stage" "${CHECKSUM:-$ARCHIVE.sha256}" "$release_digest")" ||
        die 'candidate staging failed before mutation'
    check_glibc_compatibility "$candidate_stage/manifest.json" || die 'candidate glibc compatibility check failed'
    IFS=$'\t' read -r bundle_version source_hash manifest_hash binary_hash <<<"$validation"
    candidate_id="$bundle_version-$source_hash-$manifest_hash"; previous_id="$(current_generation)"; operation_id="$(new_operation_id)"
    previous_flat=0; previous_combined=0; legacy_combined_generation=0; legacy_combined_prestate=; legacy_recovery_reader_hash=; recorder_reused=0
    journal_owner_pid=""; journal_owner_starttime=""; journal_boot_id=""; legacy_combined_prestate=; legacy_recovery_reader_hash=
    load_control_state; require_user_manager
    if [[ -n "$previous_id" ]]; then
        recorder_execution="$(recorder_execution_record "$previous_id")" || return 1
    fi
    recorder_override_migrated=0
    if legacy_combined_present; then
        legacy_combined_mixed_split_present && safe_blocked 'legacy combined and split installation states are mixed'
        capture_legacy_combined_prestate
        local legacy_info
        legacy_info="$(legacy_combined_record)" || safe_blocked 'legacy combined identity is unavailable'
        IFS=$'\t' read -r legacy_combined_version _ legacy_combined_manifest_hash _ _ <<<"$legacy_info"
    elif [[ -z "$previous_id" ]] && legacy_flat_present; then
        local legacy_info
        legacy_info="$(legacy_flat_record)" || safe_blocked 'flat predecessor is not trusted'
        previous_flat=1
    fi
    capture_runtime_state
    local managed_pid=0; probe_active codex-info-rest.service && managed_pid="$(systemd_pid)" || true
    reserve_rollback_budget
    # The durable pre-state marker must exist before the first stop or TERM.
    # This makes a crash after owner retirement resumable instead of leaving a
    # listener-less flat installation with no recovery authority.
    # Preserve explicit prestate before prepared, so interruption there can restore it.
    if [[ "$recorder_execution" == selected ]]; then capture_recorder_override_prestate "$previous_id"; fi
    if [[ -n "$legacy_recovery_reader_hash" ]]; then
        stage_legacy_recovery_handoff
        [[ "${CODEX_INFO_INTERRUPT_PHASE-}" != legacy_handoff_staged ]] || exit 75
        prepare_legacy_recovery_reader
        publish_legacy_recovery_reader
        [[ "${CODEX_INFO_INTERRUPT_PHASE-}" != legacy_handoff_bound ]] || exit 75
        promote_legacy_recovery_handoff
    fi
    write_journal prepared
    if [[ "$recorder_execution" == legacy || "$recorder_execution" == selected ]]; then
        backup_legacy_path "$recorder_override_destination"
        recorder_override_migrated=1
        write_journal prepared
    fi
    retire_legacy_combined "$managed_pid"
    enforce_desired_state || safe_blocked 'could not enforce desired runtime state'
    retire_known_unmanaged "$managed_pid"
    for destination in "$current_link" "$binary_destination" "$recorder_binary_destination" "$rest_binary_destination" "$launcher_destination" "$installer_destination" "$manifest_destination" "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do
        if [[ "$destination" != "$installer_destination" || -z "$legacy_recovery_reader_hash" ]]; then backup_legacy_path "$destination"; fi
        # Persist each legacy move, so a crash between two moves can always
        # replay the same operation without guessing from mtimes.
        write_journal prepared
    done
    write_journal legacy_backed_up; link_entrypoints; write_journal entrypoints_linked
    candidate_final="$generations_dir/$candidate_id"; publish_candidate "$candidate_stage" "$candidate_final"; candidate_stage=
    if ! verify_generation_files "$candidate_final"; then
        (( candidate_created )) && rm -r -- "$candidate_final"
        safe_blocked 'published candidate artifacts are incoherent'
    fi
    write_journal candidate_published; atomic_symlink "generations/$candidate_id" "$current_link"; write_journal current_switched
    if [[ -n "$legacy_recovery_reader_hash" ]]; then atomic_symlink '../share/codex-info/current/install.sh' "$installer_destination"; fi
    if ! activate_candidate; then rollback_transaction "$previous_id" 'candidate activation failed'; die 'candidate activation failed; previous generation restored'; fi
    write_journal activation_requested
    if ! verify_candidate; then rollback_transaction "$previous_id" 'candidate verification failed'; die 'candidate verification failed; previous generation restored'; fi
    if ! write_control_state "$desired_state"; then
        rollback_transaction "$previous_id" 'control state publication failed'
        die 'control state publication failed; previous generation restored'
    fi
    if [[ "$desired_state" == stopped || "$desired_state" == disabled ]] &&
        ! verify_nonrunning_terminal "$desired_state"; then
        rollback_transaction "$previous_id" 'candidate non-running terminal verification failed'
        die 'candidate non-running terminal verification failed; previous generation restored'
    fi
    write_journal candidate_verified; write_journal committed installed
    if [[ "$TRIGGER" != startup && "$desired_state" != removed ]]; then
        prune_obsolete_generations || exit 73
    fi
    ((QUIET)) || printf 'installed generation=%s\n' "$candidate_id"
}
update_failure_with_fallback() {
    local reason="$1" current_id startup_id fallback_ok=0
    if [[ "$reason" == *GENERATION_PRUNE_FAILED* ]]; then update_stage=cleanup; fi
    update_log failed "$reason"
    current_id="$(current_generation 2>/dev/null || true)"
    if [[ -n "$update_root" && -d "$update_root" && ! -L "$update_root" ]]; then
        rm -r -- "$update_root"
        update_root=
    fi
    if startup_id="$(startup_local_generation_can_run)"; then
        ((QUIET)) || printf 'update deferred: %s; starting verified local generation=%s\n' "$reason" "$startup_id"
        exit 0
    fi
    if [[ "$desired_state" == running ]]; then
        if [[ -n "$current_id" ]]; then
            (verify_runtime >/dev/null 2>&1) && fallback_ok=1 || true
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
startup_local_generation_can_run() {
    local current_id listener_pid
    [[ "$TRIGGER" == startup && "$desired_state" == running ]] || return 1
    current_id="$(current_generation 2>/dev/null || true)"
    [[ -n "$current_id" ]] || return 1
    verify_local_generation >/dev/null 2>&1 || return 1
    if [[ -e "$transaction" || -L "$transaction" ]]; then
        [[ -f "$transaction" && ! -L "$transaction" ]] || return 1
        read_journal
        [[ "$journal_phase" == committed ]] || return 1
    fi
    listener_pid="$(socket_pid)" || return 1
    [[ -z "$listener_pid" ]] || return 1
    printf '%s\n' "$current_id"
}
reconcile_recorder_override() {
    local generation="$1" execution
    execution="$(recorder_execution_record "$generation")" || return 1
    [[ "$execution" == legacy || "$execution" == selected ]] || return 0
    previous_id="$generation"; candidate_id="$generation"; operation_id="$(new_operation_id)"
    previous_flat=0; previous_combined=0; recorder_reused=0
    journal_owner_pid=""; journal_owner_starttime=""; journal_boot_id=""; legacy_combined_prestate=; legacy_recovery_reader_hash=
    capture_runtime_state
    reserve_rollback_budget
    if [[ "$execution" == selected ]]; then capture_recorder_override_prestate "$generation"; fi
    write_journal prepared
    backup_legacy_path "$recorder_override_destination"
    recorder_override_migrated=1
    write_journal current_switched
    if ! activate_candidate; then
        rollback_transaction "$previous_id" 'recorder override activation failed'
        die 'recorder override activation failed; previous configuration restored'
    fi
    write_journal activation_requested
    if ! verify_candidate; then
        rollback_transaction "$previous_id" 'recorder override verification failed'
        die 'recorder override verification failed; previous configuration restored'
    fi
    write_journal candidate_verified
    write_journal committed override-migrated
}
run_update() {
    local start update_deadline releases selection info local_coherent=0 discovery_limit
    update_stage=start; update_log started
    rm -f -- "$update_failure_file"
    [[ ! -f "$transaction" ]] || resume_transaction
    start="$(now_unix)" || safe_blocked 'update clock is unavailable'; [[ "$TRIGGER" == timer ]] && update_deadline=$((start+TIMER_TIMEOUT)) || update_deadline=$((start+MANUAL_TIMEOUT))
    if (( operation_deadline > 0 && operation_deadline < update_deadline )); then
        update_deadline=$operation_deadline
    fi
    operation_deadline=$update_deadline
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
    [[ -z "$current_id" ]] || recorder_execution_record "$current_id" >/dev/null || return 1
    preflight_listener_owner
    IFS=$'\t' read -r installed_version _ _ _ <<<"$info"
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
    update_target="$installed_version"; update_stage=discovery; update_log started
    command -v "$CURL_BIN" >/dev/null 2>&1 || update_failure_with_fallback "$CURL_BIN is required"
    discovery_limit=30
    if (( operation_deadline > 0 )); then discovery_limit="$(deadline_timeout 30)" || update_failure_with_fallback 'update overall timeout exceeded before discovery'; fi
    if ! "$CURL_BIN" --fail --silent --show-error --proto '=https' --max-time "$discovery_limit" --header 'Accept: application/vnd.github+json' --header 'X-GitHub-Api-Version: 2022-11-28' "$RELEASES_URL" >"$releases"; then
        update_failure_with_fallback 'public release discovery failed'
    fi
    if ! select_release "$releases" "$installed_version" "$local_coherent" >"$selection"; then
        update_failure_with_fallback 'release selection failed'
    fi
    local state newest; IFS=$'\t' read -r state newest < "$selection"
    update_target="$newest"; update_stage=selection; update_log succeeded
    if [[ "$state" == no-update ]]; then
        verify_local_generation || safe_blocked 'no-update local generation is incoherent'
        reconcile_recorder_override "$current_id"
        if [[ "$desired_state" == running ]]; then
            if ! probe_active codex-info-recorder.service || ! probe_active codex-info-rest.service; then
                retire_known_unmanaged 0
                if [[ "$TRIGGER" == startup ]]; then
                    converge_enable_links || safe_blocked 'startup enable links could not be recovered'
                    rm -r -- "$update_root"; update_root=; update_log no-update; ((QUIET)) || printf 'no update current=%s newest=%s\n' "$installed_version" "$newest"; return
                fi
                reset_failed_main || safe_blocked 'could not reset failed managed service'
                enable_managed_unit codex-info-recorder.service || safe_blocked 'could not enable recorder service'
                enable_managed_unit codex-info-rest.service || safe_blocked 'could not enable REST service'
                systemctl_user start --no-block codex-info-recorder.service >/dev/null 2>&1 || safe_blocked 'could not start recorder service'
                systemctl_user start --no-block codex-info-rest.service >/dev/null 2>&1 || safe_blocked 'could not start REST service'
            else
                repair_known_managed_runtime
            fi
            wait_runtime_ready || safe_blocked 'no-update managed runtime is not healthy'
        fi
        if [[ "$desired_state" != disabled && "$desired_state" != removed ]]; then
            rearm_update_timer || safe_blocked 'update timer could not be rearmed'
        fi
        if [[ "$TRIGGER" != startup && "$desired_state" != removed ]] && (( ! transaction_recovered )); then
            if [[ "$desired_state" != running ]]; then
                verify_nonrunning_terminal "$desired_state" || safe_blocked 'no-update non-running terminal is not healthy'
            fi
            if ! prune_obsolete_generations; then
                update_failure_with_fallback 'GENERATION_PRUNE_FAILED after verified no-update'
            fi
        fi
        rm -r -- "$update_root"; update_root=; update_log no-update; ((QUIET)) || printf 'no update current=%s newest=%s\n' "$installed_version" "$newest"; return
    fi
    [[ "$state" == update ]] || die 'release selection returned unknown state'
    (( $(now_unix) <= update_deadline )) || update_failure_with_fallback 'update overall timeout exceeded before download'
    local archive_name archive_url archive_digest
    IFS=$'\t' read -r archive_name archive_url archive_digest < <(sed -n '2p' "$selection")
    local archive_path="$update_root/$archive_name" manifest_path="$update_root/manifest.json"
    update_stage=download; update_log started
    download_asset "$archive_url" "$archive_path" "$archive_digest" || update_failure_with_fallback 'release archive download failed'
    extract_bundle_manifest "$archive_path" "$manifest_path" || update_failure_with_fallback 'release archive manifest is unavailable'
    (( $(now_unix) <= update_deadline )) || update_failure_with_fallback 'update overall timeout exceeded before candidate installation'
    local child_limit
    child_limit="$(deadline_timeout "$MANUAL_TIMEOUT")" || update_failure_with_fallback 'update overall timeout exceeded before candidate installation'
    local child_status=0
    update_stage=install; update_log started
    local -a migration_options=()
    if (( migrate_recorder_override )); then migration_options=(--migrate-recorder-override); fi
    if CODEX_INFO_INTERNAL_TRIGGER="$TRIGGER" CODEX_INFO_DEADLINE="$update_deadline" CODEX_INFO_INSTALL_LOCKED=1 \
        CODEX_INFO_RELEASE_DIGEST_VERIFIED=1 CODEX_INFO_RELEASE_ARCHIVE_DIGEST="$archive_digest" \
        CODEX_INFO_UPDATE_TARGET="$newest" \
        timeout --foreground "$child_limit" "$0" --bundle "$archive_path" --manifest "$manifest_path" "${migration_options[@]}"; then
        child_status=0
    else
        child_status="$?"
        if [[ "$child_status" == 73 ]]; then
            update_stage=cleanup
            update_failure_with_fallback 'candidate committed but GENERATION_PRUNE_FAILED'
        fi
        update_failure_with_fallback "$(cat -- "$update_failure_file" 2>/dev/null || printf 'candidate installation failed')"
    fi
    (( $(now_unix) <= update_deadline )) || update_failure_with_fallback 'update overall timeout exceeded after candidate installation'
    rm -r -- "$update_root"; update_root=; update_stage=complete; update_log succeeded; ((QUIET)) || printf 'updated from=%s to=%s\n' "$installed_version" "$newest"
}
select_release() {
    local release="$1" current="$2" local_coherent="${3:-0}"
    python3 - "$release" "$current" "$local_coherent" "$TARGET" <<'PY'
import json,pathlib,re,sys
release_path,current_text,local_coherent,target=sys.argv[1:]
def reject(message): raise SystemExit("release metadata validation failed: "+message)
try: release=json.loads(pathlib.Path(release_path).read_text(encoding="utf-8"))
except Exception as error: reject(str(error))
if not re.fullmatch(r"(?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)",current_text): reject("installed version invalid")
if not isinstance(release,dict): reject("latest release is not an object")
tag=release.get("tag_name")
match=re.fullmatch(r"windows-v((?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*)[.](?:0|[1-9][0-9]*))",tag) if isinstance(tag,str) else None
if match is None: reject("latest release tag malformed")
newest_text=match.group(1); newest=tuple(map(int,newest_text.split(".")))
current=tuple(map(int,current_text.split(".")))
assets=release.get("assets")
if not isinstance(assets,list): reject("latest release assets are not an array")
archive_name=f"codex-info-{newest_text}-{target}.tar.gz"
selected=None
for asset in assets:
    if not isinstance(asset,dict): continue
    name=asset.get("name")
    if name != archive_name: continue
    if selected is not None: reject("latest release has multiple Linux archives for this version")
    url,digest=(asset.get(key) for key in ("browser_download_url","digest"))
    if not isinstance(url,str) or not isinstance(digest,str) or not re.fullmatch(r"sha256:[0-9a-f]{64}",digest):
        reject("Linux archive digest or download URL is unavailable")
    selected=(url,digest)
if selected is None: reject("latest release is missing its Linux archive")
needs=(newest>current or local_coherent!="1")
if newest<current or not needs: print("no-update",newest_text,sep="\t"); raise SystemExit(0)
print("update",newest_text,sep="\t")
print(archive_name,selected[0],selected[1],sep="\t")
PY
}
download_asset() {
    local url="$1" destination="$2" digest="$3" download_limit
    download_limit=300
    if (( operation_deadline > 0 )); then
        download_limit="$(deadline_timeout 300)" || return 1
    fi
    "$CURL_BIN" --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --max-time "$download_limit" --output "$destination" "$url" || return 1
    [[ -f "$destination" && ! -L "$destination" ]] || return 1
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

# Settle publication and the bounded legacy entrypoint handoff under L1.
# Committed history is never rewritten by final entrypoint cleanup.
if (( ! lock_bypassed )); then
    resume_transaction
fi

if [[ "$ACTION" == start ]]; then
    load_control_state; require_user_manager
    # An explicit start supersedes a same-boot --stop intent.  Persist this
    # desired-state transition before resolving so the shared installer keeps
    # the service running even when the resolver selects an equal generation.
    write_control_state running
    run_update
    enable_managed_unit codex-info-recorder.service || safe_blocked 'could not enable recorder service'
    enable_managed_unit codex-info-rest.service || safe_blocked 'could not enable REST service'
    rearm_update_timer || safe_blocked 'could not enable update timer'
    if ! probe_active codex-info-recorder.service; then
        systemctl_user start --no-block codex-info-recorder.service >/dev/null 2>&1 || safe_blocked 'could not start recorder service'
    fi
    if ! probe_active codex-info-rest.service; then
        systemctl_user start --no-block codex-info-rest.service >/dev/null 2>&1 || safe_blocked 'could not start REST service'
    fi
    wait_runtime_ready || safe_blocked 'managed runtime is not healthy after start'
    printf 'started codex-info-recorder.service and codex-info-rest.service\n'
    exit
fi
if [[ "$ACTION" == stop ]]; then
    load_control_state; require_user_manager; guard_control_listener
    systemctl_stop_user stop --no-block codex-info-recorder.service >/dev/null 2>&1 || die 'could not stop recorder service'
    wait_inactive codex-info-recorder.service || safe_blocked 'recorder service did not stop within 20s'
    systemctl_stop_user stop --no-block codex-info-rest.service >/dev/null 2>&1 || die 'could not stop REST service'
    wait_inactive codex-info-rest.service || safe_blocked 'REST service did not stop within 20s'
    enable_managed_unit codex-info-recorder.service || die 'could not keep recorder service enabled'
    enable_managed_unit codex-info-rest.service || die 'could not keep REST service enabled'
    rearm_update_timer || safe_blocked 'update timer could not remain active after stop'
    desired_state=stopped
    verify_nonrunning_terminal stopped || safe_blocked 'stopped terminal could not be verified'
    write_control_state stopped
    load_control_state; verify_nonrunning_terminal stopped || safe_blocked 'stopped control state readback failed'
    printf 'stopped recorder and REST services (timer remains enabled)\n'; exit
fi
if [[ "$ACTION" == disable ]]; then
    load_control_state; require_user_manager; guard_control_listener
    systemctl_stop_user stop --no-block codex-info-recorder.service >/dev/null 2>&1 || die 'could not stop recorder service'
    wait_inactive codex-info-recorder.service || safe_blocked 'recorder service did not stop within 20s'
    disable_managed_unit codex-info-recorder.service || die 'could not disable recorder service'
    systemctl_stop_user stop --no-block codex-info-rest.service >/dev/null 2>&1 || die 'could not stop REST service'
    wait_inactive codex-info-rest.service || safe_blocked 'REST service did not stop within 20s'
    disable_managed_unit codex-info-rest.service || die 'could not disable REST service'
    systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || die 'could not stop timer'
    wait_inactive codex-info-update.timer || safe_blocked 'update timer did not stop within 20s'
    disable_managed_unit codex-info-update.timer || die 'could not disable timer'
    desired_state=disabled
    verify_nonrunning_terminal disabled || safe_blocked 'disabled terminal could not be verified'
    write_control_state disabled
    load_control_state; verify_nonrunning_terminal disabled || safe_blocked 'disabled control state readback failed'
    printf 'disabled autostart (unit files retained)\n'; exit
fi
if [[ "$ACTION" == remove ]]; then
    load_control_state; require_user_manager; guard_control_listener
    systemctl_stop_user stop --no-block codex-info-recorder.service >/dev/null 2>&1 || die 'could not stop recorder service'
    wait_inactive codex-info-recorder.service || safe_blocked 'recorder service did not stop within 20s'
    disable_managed_unit codex-info-recorder.service || die 'could not disable recorder service'
    systemctl_stop_user stop --no-block codex-info-rest.service >/dev/null 2>&1 || die 'could not stop REST service'
    wait_inactive codex-info-rest.service || safe_blocked 'REST service did not stop within 20s'
    disable_managed_unit codex-info-rest.service || die 'could not disable REST service'
    systemctl_stop_user stop --no-block codex-info-update.timer >/dev/null 2>&1 || die 'could not stop timer'
    wait_inactive codex-info-update.timer || safe_blocked 'update timer did not stop within 20s'
    disable_managed_unit codex-info-update.timer || die 'could not disable timer'
    systemctl_stop_user stop --no-block codex-info-update.service >/dev/null 2>&1 || die 'could not stop update service'
    wait_inactive codex-info-update.service || safe_blocked 'update service did not stop within 20s'
    for destination in "$unit_destination" "$rest_unit_destination" "$update_service_destination" "$update_timer_destination"; do [[ -L "$destination" ]] || safe_blocked "refusing to remove non-symlink unit: $destination"; done
    atomic_unlink "$unit_destination"; atomic_unlink "$rest_unit_destination"; atomic_unlink "$update_service_destination"; atomic_unlink "$update_timer_destination"; systemctl_user daemon-reload >/dev/null 2>&1 || die 'daemon-reload failed during remove'
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
    if probe_active codex-info-recorder.service && probe_active codex-info-rest.service; then verify_runtime; else ((QUIET)) || printf 'startup reconcile complete; service activation is pending\n'; fi
    exit
fi
if [[ "$ACTION" == update || "$ACTION" == timer-update ]]; then run_update; exit; fi

[[ -n "$ARCHIVE" ]] || die '--bundle ARCHIVE is required'
archive_dir="$(cd -- "$(dirname -- "$ARCHIVE")" && pwd)"
ARCHIVE="$archive_dir/$(basename -- "$ARCHIVE")"
[[ -n "$MANIFEST" ]] || MANIFEST="${ARCHIVE%.tar.gz}.manifest.json"
if [[ "${CODEX_INFO_RELEASE_DIGEST_VERIFIED:-}" == 1 ]]; then
    [[ "$ACTION" == install && "${CODEX_INFO_INSTALL_LOCKED:-}" == 1 && -e /proc/self/fd/9 ]] ||
        die 'verified-release installation requires the active update transaction'
else
    [[ -n "$CHECKSUM" ]] || CHECKSUM="$ARCHIVE.sha256"
fi
perform_install

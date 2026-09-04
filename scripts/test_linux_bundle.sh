#!/usr/bin/env bash
set -euo pipefail

# Finite isolated contract test for the Linux archive, persistent installer,
# journal recovery, equal-version repair, and retention/removal boundaries.
# No host HOME, systemd user manager, profile, process, or network is touched.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_SCRIPT="$ROOT_DIR/scripts/build_linux_bundle.sh"
ORIGINAL_PATH="$PATH"
BUNDLE_DIR=''
while (($# > 0)); do
    case "$1" in
        --bundle-dir)
            (($# >= 2)) || { echo 'linux-bundle-test: --bundle-dir requires a path' >&2; exit 2; }
            [[ -z "$BUNDLE_DIR" ]] || { echo 'linux-bundle-test: --bundle-dir supplied twice' >&2; exit 2; }
            BUNDLE_DIR="$2"
            shift 2
            ;;
        -h|--help)
            printf 'usage: test_linux_bundle.sh [--bundle-dir DIR]\n'
            exit 0
            ;;
        *)
            echo "linux-bundle-test: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done
TEST_ROOT="$(mktemp -d /tmp/codex-info-linux-bundle-test.XXXXXX)"
trap 'rm -r -- "$TEST_ROOT"' EXIT

fail() { echo "linux-bundle-test: $*" >&2; exit 1; }
assert_file() { [[ -f "$1" && ! -L "$1" ]] || fail "missing regular file: $1"; }
assert_symlink() { [[ -L "$1" ]] || fail "missing symlink: $1"; }

validate_workflow_candidate() {
    local directory="$1" archive manifest checksum
    [[ -d "$directory" && ! -L "$directory" ]] || fail "bundle directory is not a directory: $directory"
    mapfile -t archives < <(find "$directory" -mindepth 1 -maxdepth 1 -type f \
        -name 'codex-info-*-x86_64-unknown-linux-gnu.tar.gz' -printf '%p\n' | LC_ALL=C sort)
    ((${#archives[@]} == 1)) || fail "bundle directory must contain exactly one Linux archive (found ${#archives[@]})"
    archive="${archives[0]}"
    manifest="${archive%.tar.gz}.manifest.json"
    checksum="$archive.sha256"
    [[ -f "$manifest" && ! -L "$manifest" ]] || fail 'candidate manifest sidecar is missing or not regular'
    [[ -f "$checksum" && ! -L "$checksum" ]] || fail 'candidate checksum sidecar is missing or not regular'
    python3 - "$archive" "$manifest" "$ROOT_DIR/run.sh" <<'PY'
import hashlib
import json
import pathlib
import stat
import sys
import tarfile

archive_path, external_manifest_path, source_run_path = map(pathlib.Path, sys.argv[1:])
def reject(message):
    raise SystemExit(f"workflow candidate validation failed: {message}")
def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            reject("duplicate JSON key")
        result[key] = value
    return result
def digest(data):
    return hashlib.sha256(data).hexdigest()
try:
    external_bytes = external_manifest_path.read_bytes()
    external = json.loads(external_bytes, object_pairs_hook=pairs)
except Exception as error:
    reject(f"external manifest is invalid: {error}")
try:
    with tarfile.open(archive_path, "r:gz") as stream:
        members = stream.getmembers()
        if any(not item.isfile() or item.issym() or item.islnk() for item in members):
            reject("archive contains a non-regular member")
        names = [item.name for item in members]
        if names != sorted(names) or len(names) != len(set(names)):
            reject("archive members are not sorted and unique")
        data = {item.name: stream.extractfile(item).read() for item in members}
        modes = {item.name: stat.S_IMODE(item.mode) for item in members}
except Exception as error:
    reject(f"archive cannot be read: {error}")
if data.get("manifest.json") != external_bytes:
    reject("internal and external manifest bytes differ")
if set(external) != {"schema", "product", "version", "source_sha", "run_id", "run_attempt", "target", "compatibility", "glibc_minimum", "files"}:
    reject("manifest top-level keys are not exact")
files = external.get("files")
if not isinstance(files, list):
    reject("manifest files is not a list")
expected = {}
for entry in files:
    if not isinstance(entry, dict) or set(entry) != {"path", "size", "sha256", "mode"}:
        reject("manifest file entry keys are not exact")
    path, size, sha, mode = (entry.get(key) for key in ("path", "size", "sha256", "mode"))
    if not isinstance(path, str) or not path or path.startswith("/") or "\\" in path or ".." in pathlib.PurePosixPath(path).parts:
        reject("manifest path is unsafe")
    if path in expected or path in {"manifest.json", "SHA256SUMS"}:
        reject("manifest file paths are not unique/payload-only")
    if type(size) is not int or size < 0 or not isinstance(sha, str) or len(sha) != 64 or any(c not in "0123456789abcdef" for c in sha):
        reject("manifest size/hash is malformed")
    if type(mode) is not int or mode not in {0o644, 0o755}:
        reject("manifest mode is malformed")
    expected[path] = (size, sha, mode)
if list(expected) != sorted(expected):
    reject("manifest entries are not sorted")
if set(data) != set(expected) | {"manifest.json", "SHA256SUMS"}:
    reject("manifest does not cover exact archive payload")
for path, (size, sha, mode) in expected.items():
    if len(data[path]) != size or digest(data[path]) != sha or modes[path] != mode:
        reject(f"archive member identity mismatch: {path}")
    required_mode = 0o755 if path in {"codex_info", "run.sh", "install.sh"} else 0o644
    if mode != required_mode:
        reject(f"archive member mode is not canonical: {path}")
if modes.get("manifest.json") != 0o644 or modes.get("SHA256SUMS") != 0o644:
    reject("manifest/checksum modes are not 0644")
if data.get("run.sh") != source_run_path.read_bytes():
    reject("archive run.sh differs from repository source")
if modes.get("install.sh") != 0o755:
    reject("installer is not executable in archive")
lines = data.get("SHA256SUMS", b"").decode("utf-8").splitlines()
checks = {}
for line in lines:
    fields = line.split("  ", 1)
    if len(fields) != 2 or len(fields[0]) != 64 or any(c not in "0123456789abcdef" for c in fields[0]):
        reject("SHA256SUMS line is malformed")
    name = fields[1]
    if name in checks or name == "SHA256SUMS" or name not in data:
        reject("SHA256SUMS names are not exact")
    checks[name] = fields[0]
if list(checks) != sorted(checks) or set(checks) != set(data) - {"SHA256SUMS"}:
    reject("SHA256SUMS does not cover exact archive payload")
for name, sha in checks.items():
    if sha != digest(data[name]):
        reject(f"SHA256SUMS digest mismatch: {name}")
PY
    (cd -- "$directory" && sha256sum --check --status "$(basename -- "$checksum")") ||
        fail 'candidate archive sidecar checksum failed'
    printf 'case workflow candidate (--bundle-dir): PASS\n'
}

if [[ -n "$BUNDLE_DIR" ]]; then
    validate_workflow_candidate "$BUNDLE_DIR"
fi

fake_bin="$TEST_ROOT/fake-bin"
fake_home="$TEST_ROOT/home"
fake_proc="$TEST_ROOT/proc"
fixture_root="$TEST_ROOT/fixture"
output_root="$TEST_ROOT/output"
release_json="$TEST_ROOT/release.json"
release_assets="$TEST_ROOT/release-assets"
update_tmp="$TEST_ROOT/update-tmp"
log="$TEST_ROOT/commands.log"
mkdir -p -- "$fake_bin" "$fake_home" "$fake_proc/net" "$fixture_root" "$output_root" "$release_assets" "$update_tmp"
: > "$fake_proc/net/tcp"

cat > "$fake_bin/systemctl" <<'FAKE_SYSTEMCTL'
#!/usr/bin/env bash
set -euo pipefail
printf 'systemctl %s\n' "$*" >> "$FAKE_LOG"
[[ "${1-}" == --user ]] && shift
case "${1-}" in
    show-environment) exit 0 ;;
    is-enabled)
        unit="${*: -1}"
        case "$unit" in
            codex-info.service) [[ "${FAKE_MAIN_ENABLED:-0}" == 1 ]] && exit 0 || exit 1 ;;
            codex-info-update.timer) [[ "${FAKE_TIMER_ENABLED:-0}" == 1 ]] && exit 0 || exit 1 ;;
            *) exit 1 ;;
        esac
        ;;
    is-active)
        unit="${*: -1}"
        case "$unit" in
            codex-info.service) [[ "${FAKE_MAIN_ACTIVE:-0}" == 1 ]] && exit 0 || exit 3 ;;
            codex-info-update.timer) [[ "${FAKE_TIMER_ACTIVE:-0}" == 1 ]] && exit 0 || exit 3 ;;
            *) exit 3 ;;
        esac
        ;;
    show)
        [[ "$*" == *MainPID* ]] && printf '%s\n' "${FAKE_MAIN_PID:-0}"
        exit 0
        ;;
    daemon-reload|enable|disable|start|stop|restart)
        unit="${*: -1}"
        if [[ "${FAKE_STARTUP_CONDITION:-0}" == 1 &&
              "$unit" == codex-info.service &&
              ("$1" == start || "$1" == restart) ]]; then
            [[ -n "${FAKE_INSTALLER:-}" ]] || exit 1
            transaction="${FAKE_INSTALLER%/.local/libexec/codex-info-install.sh}/.local/share/codex-info/install-transaction.json"
            if [[ -f "$transaction" ]] && ! grep -Fq '"phase": "committed"' "$transaction"; then
                env -u CODEX_INFO_PROC_ROOT bash "$FAKE_INSTALLER" --startup-reconcile >/dev/null
            fi
            env -u CODEX_INFO_PROC_ROOT bash "$FAKE_INSTALLER" --startup-condition >/dev/null
        fi
        exit 0
        ;;
    *) exit 0 ;;
esac
FAKE_SYSTEMCTL
chmod 0755 "$fake_bin/systemctl"

cat > "$fake_bin/curl" <<'FAKE_CURL'
#!/usr/bin/env bash
set -euo pipefail
printf 'curl %s\n' "$*" >> "$FAKE_LOG"
url=''
output=''
write_out=''
while (($# > 0)); do
    case "$1" in
        --output|-o|--write-out|-w|--max-time|--max-redirs|--proto|--proto-redir|--header|-H)
            (($# >= 2)) || exit 2
            if [[ "$1" == --output || "$1" == -o ]]; then output="$2"; fi
            if [[ "$1" == --write-out || "$1" == -w ]]; then write_out="$2"; fi
            shift 2
            ;;
        --fail|--silent|--show-error|--location) shift ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
payload=''
payload_file=''
if [[ "$url" == */releases?per_page=100 ]]; then
    [[ "${FAKE_RELEASE_FAILURE:-0}" == 1 ]] && exit 22
    payload_file="$FAKE_RELEASE_JSON"
elif [[ "$url" == */releases/download/*/* ]]; then
    payload_file="$FAKE_RELEASE_ASSETS/${url##*/}"
elif [[ "$url" == http://127.0.0.1:8787/v1/details ]]; then
    details_padding=''
    printf -v details_padding '%*s' "${FAKE_DETAILS_PADDING_BYTES:-0}" ''
    details_padding="${details_padding// /x}"
    payload="$(printf '{\"state\":\"%s\",\"observed_at\":1,\"padding\":\"%s\"}\n' \
        "${FAKE_DETAILS_STATE:-auth_required}" "$details_padding")"
elif [[ "$url" == http://127.0.0.1:8787/v1/health ]]; then
    if [[ -n "${FAKE_HEALTH_COUNT_FILE:-}" ]]; then
        count=0
        [[ -f "$FAKE_HEALTH_COUNT_FILE" ]] && count="$(<"$FAKE_HEALTH_COUNT_FILE")"
        count=$((count + 1))
        printf '%s\n' "$count" > "$FAKE_HEALTH_COUNT_FILE"
        if ((count <= ${FAKE_HEALTH_FAILURES:-0})); then exit 22; fi
    fi
    case "${FAKE_HEALTH_SHAPE:-exact}" in
        exact)
            payload="$(printf '{"api_version":"v1","service":"codex-info","product_version":"%s"}\n' \
                "$FAKE_HEALTH_VERSION")" ;;
        extra)
            payload="$(printf '{"api_version":"v1","service":"codex-info","product_version":"%s","extra":true}\n' \
                "$FAKE_HEALTH_VERSION")" ;;
        duplicate)
            payload="$(printf '{"api_version":"v1","service":"codex-info","product_version":"%s","product_version":"%s"}\n' \
                "$FAKE_HEALTH_VERSION" "$FAKE_HEALTH_VERSION")" ;;
        old)
            payload='{"api_version":"v1","service":"codex-info","product_version":"0.0.1"}'$'\n' ;;
        *) exit 1 ;;
    esac
else
    exit 1
fi
if [[ -n "$payload_file" ]]; then
    [[ -f "$payload_file" ]] || exit 1
    if [[ -n "$output" ]]; then cp -- "$payload_file" "$output"; else payload="$(<"$payload_file")"; fi
elif [[ -n "$output" ]]; then
    printf '%s' "$payload" > "$output"
fi
if [[ -z "$output" ]]; then printf '%s' "$payload"; fi
if [[ "$write_out" == '%{url_effective}' ]]; then printf '%s' "${FAKE_CURL_EFFECTIVE_URL:-$url}"; fi
FAKE_CURL
chmod 0755 "$fake_bin/curl"

cat > "$fake_bin/objdump" <<'FAKE_OBJDUMP'
#!/usr/bin/env bash
printf 'fake GLIBC_2.31\n'
FAKE_OBJDUMP
chmod 0755 "$fake_bin/objdump"

cat > "$fake_bin/getconf" <<'FAKE_GETCONF'
#!/usr/bin/env bash
set -euo pipefail
if [[ -n "${FAKE_GLIBC_VERSION:-}" ]]; then
    printf 'glibc %s\n' "$FAKE_GLIBC_VERSION"
else
    exec /usr/bin/getconf "$@"
fi
FAKE_GETCONF
chmod 0755 "$fake_bin/getconf"
cat > "$fake_bin/ldd" <<'FAKE_LDD'
#!/usr/bin/env bash
set -euo pipefail
if [[ -n "${FAKE_GLIBC_VERSION:-}" ]]; then
    printf 'ldd (GNU libc) %s\n' "$FAKE_GLIBC_VERSION"
else
    exec /usr/bin/ldd "$@"
fi
FAKE_LDD
chmod 0755 "$fake_bin/ldd"

printf 'fixture binary generation one\n' > "$fixture_root/codex_info"
chmod 0755 "$fixture_root/codex_info"

build_bundle() {
    local source="$1" version="$2"
    SOURCE_SHA="$source" RUN_ID=92001 RUN_ATTEMPT=1 OBJDUMP_BIN="$fake_bin/objdump" \
        bash "$BUILD_SCRIPT" --binary "$fixture_root/codex_info" --version "$version" \
        --output-dir "$output_root" >/dev/null
    printf '%s/codex-info-%s-x86_64-unknown-linux-gnu.tar.gz\n' "$output_root" "$version"
}

archive_version() {
    basename -- "$1" | sed 's/^codex-info-//; s/-x86_64-unknown-linux-gnu\.tar\.gz$//'
}

write_release() {
    local archive="$1" version archive_name
    version="$(archive_version "$archive")"
    archive_name="$(basename -- "$archive")"
    rm -f -- "$release_assets"/*
    cp -- "$archive" "$release_assets/$archive_name"
    cp -- "${archive%.tar.gz}.manifest.json" "$release_assets/${archive_name%.tar.gz}.manifest.json"
    cp -- "$archive.sha256" "$release_assets/$archive_name.sha256"
    printf 'fixture Windows Setup %s\n' "$version" > "$release_assets/CodexInfo.WindowsClient.Setup.exe"
    printf '{"version":"%s"}\n' "$version" > "$release_assets/CodexInfo.WindowsClient.update.json"
    python3 - "$release_assets" "$version" "$release_json" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1]); version = sys.argv[2]; output = pathlib.Path(sys.argv[3])
names = [
    f"codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz",
    f"codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz.sha256",
    f"codex-info-{version}-x86_64-unknown-linux-gnu.manifest.json",
    "CodexInfo.WindowsClient.Setup.exe",
    "CodexInfo.WindowsClient.update.json",
]
assets = []
for name in names:
    data = (root / name).read_bytes()
    assets.append({
        "name": name,
        "browser_download_url": f"https://github.com/salty919/codex_info_v2/releases/download/windows-v{version}/{name}",
        "state": "uploaded",
        "size": len(data),
        "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
    })
release = {
    "tag_name": f"windows-v{version}", "draft": False, "prerelease": False,
    "published_at": "2026-09-01T00:00:00Z", "assets": assets,
}
output.write_text(json.dumps([release], separators=(",", ":")) + "\n", encoding="utf-8")
PY
}

extract_installer() {
    local archive="$1" version destination
    version="$(archive_version "$archive")"
    destination="$TEST_ROOT/install-$version.sh"
    tar -xOf "$archive" install.sh > "$destination"
    chmod 0755 "$destination"
    printf '%s\n' "$destination"
}

run_install() {
    local archive="$1" home="$2" script
    script="$(extract_installer "$archive")"
    HOME="$home" CODEX_HOME="$home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$script" --bundle "$archive"
}
run_install_glibc() {
    local archive="$1" home="$2" version="$3" script
    script="$(extract_installer "$archive")"
    HOME="$home" CODEX_HOME="$home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        FAKE_GLIBC_VERSION="$version" GETCONF_BIN="$fake_bin/getconf" LDD_BIN="$fake_bin/ldd" \
        CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$script" --bundle "$archive"
}
run_install_with_manifest() {
    local archive="$1" home="$2" manifest="$3" script
    script="$(extract_installer "$archive")"
    HOME="$home" CODEX_HOME="$home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$script" --bundle "$archive" --manifest "$manifest"
}

run_update() {
    local home="$1"
    HOME="$home" CODEX_HOME="$home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        FAKE_STARTUP_CONDITION=1 FAKE_INSTALLER="$home/.local/libexec/codex-info-install.sh" \
        FAKE_RELEASE_JSON="$release_json" FAKE_RELEASE_ASSETS="$release_assets" \
        TMPDIR="$update_tmp" CODEX_INFO_PROC_ROOT="$fake_proc" \
        SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$home/.local/libexec/codex-info-install.sh" --update
}

boot_id_value="$(< /proc/sys/kernel/random/boot_id)"
readonly_home="$TEST_ROOT/readonly-home"
mkdir -p -- "$readonly_home"
for readonly_action in --status --verify-runtime; do
    HOME="$readonly_home" CODEX_HOME="$readonly_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$ROOT_DIR/packaging/install_linux_bundle.sh" "$readonly_action" >/dev/null 2>&1 || true
done
[[ ! -e "$readonly_home/.local/share" && ! -e "$readonly_home/.local/bin" &&
   ! -e "$readonly_home/.local/libexec" && ! -e "$readonly_home/.config" ]] ||
    fail 'read-only status/verify created installation state'
printf 'case read-only empty-home: PASS\n'
write_stopped_state() {
    local home="$1"
    mkdir -p -- "$home/.local/share/codex-info"
    printf '{"schema":"codex-info-control-state-v1","desired_state":"stopped","boot_id":"%s","operation_id":"fixture","generation_id":"","updated_at_unix":1}\n' \
        "$boot_id_value" > "$home/.local/share/codex-info/control-state.json"
    chmod 0600 "$home/.local/share/codex-info/control-state.json"
}
write_running_state() {
    local home="$1"
    python3 - "$home/.local/share/codex-info/control-state.json" <<'PY'
import json, pathlib, sys, time
path = pathlib.Path(sys.argv[1])
value = json.loads(path.read_text())
value["desired_state"] = "running"
value["updated_at_unix"] = int(time.time())
path.write_text(json.dumps(value, separators=(",", ":")) + "\n")
PY
    chmod 0600 "$home/.local/share/codex-info/control-state.json"
}

# Hold a real descriptor-9 lock while exposing an unsettled journal.  This
# models systemd's separate ExecCondition process observing the installer
# that owns the active transaction, without granting the condition an
# environment-only bypass.
transaction_owner_fixture="$TEST_ROOT/transaction-owner-fixture.sh"
cat > "$transaction_owner_fixture" <<'TRANSACTION_OWNER_FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
lock_path="$1"
transaction_path="$2"
boot_id="$3"
phase="$4"
previous_generation="$5"
candidate_generation="$6"
hold_pipe="$7"
ready_path="$8"
exec 9<>"$lock_path"
flock --exclusive 9
exec 8<"$hold_pipe"
owner_pid="$BASHPID"
owner_starttime="$(python3 - "$owner_pid" <<'PY'
from pathlib import Path
import sys
text = Path("/proc", sys.argv[1], "stat").read_text(encoding="utf-8")
fields = text.rsplit(") ", 1)[1].split()
print(fields[19])
PY
)"
python3 - "$transaction_path" "$boot_id" "$phase" "$previous_generation" "$candidate_generation" "$owner_pid" "$owner_starttime" <<'PY'
import json
import os
import pathlib
import sys
path, boot_id, phase, previous, candidate, owner_pid, owner_starttime = sys.argv[1:]
document = {
    "schema": "codex-info-install-transaction-v1",
    "operation_id": "active-fixture",
    "owner_pid": int(owner_pid),
    "owner_starttime": int(owner_starttime),
    "boot_id": boot_id,
    "phase": phase,
    "old_generation": previous,
    "new_generation": candidate,
    "desired_state": "running",
    "updated_at_unix": 1,
}
pathlib.Path(path).write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
os.chmod(path, 0o600)
PY
: > "$ready_path"
read -r _ <&8
TRANSACTION_OWNER_FIXTURE
chmod 0755 "$transaction_owner_fixture"
run_startup_condition() {
    local home="$1"
    env -u CODEX_INFO_PROC_ROOT HOME="$home" CODEX_HOME="$home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" \
        FAKE_LOG="$log" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$home/.local/libexec/codex-info-install.sh" --startup-condition
}
run_startup_reconcile() {
    local home="$1"
    env -u CODEX_INFO_PROC_ROOT HOME="$home" CODEX_HOME="$home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" \
        FAKE_LOG="$log" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$home/.local/libexec/codex-info-install.sh" --startup-reconcile
}
run_active_startup_condition_case() {
    local home="$1" phase="$2" previous="$3" candidate="$4" expected="$5" label="$6"
    local ready_path="$TEST_ROOT/transaction-owner-$label.ready" hold_pipe="$TEST_ROOT/transaction-owner-$label.pipe"
    local owner_pid hold_fd
    rm -f -- "$ready_path" "$hold_pipe"
    mkfifo -- "$hold_pipe"
    "$transaction_owner_fixture" "$home/.local/share/codex-info/.install.lock" \
        "$home/.local/share/codex-info/install-transaction.json" "$boot_id_value" \
        "$phase" "$previous" "$candidate" "$hold_pipe" "$ready_path" &
    owner_pid="$!"
    exec {hold_fd}>"$hold_pipe"
    for _ in {1..100}; do
        [[ -f "$ready_path" ]] && break
        sleep 0.01
    done
    if [[ ! -f "$ready_path" ]]; then
        kill "$owner_pid" 2>/dev/null || true
        wait "$owner_pid" 2>/dev/null || true
        exec {hold_fd}>&-
        fail "transaction owner fixture did not become ready: $label"
    fi
    if [[ "$expected" == pass ]]; then
        if ! run_startup_reconcile "$home" >/dev/null || ! run_startup_condition "$home" >/dev/null; then
            kill "$owner_pid" 2>/dev/null || true
            wait "$owner_pid" 2>/dev/null || true
            exec {hold_fd}>&-
            fail "active transaction condition unexpectedly rejected: $label"
        fi
    elif run_startup_reconcile "$home" >/dev/null 2>&1 || run_startup_condition "$home" >/dev/null 2>&1; then
        kill "$owner_pid" 2>/dev/null || true
        wait "$owner_pid" 2>/dev/null || true
        exec {hold_fd}>&-
        fail "unsafe transaction condition unexpectedly accepted: $label"
    fi
    kill "$owner_pid" 2>/dev/null || true
    wait "$owner_pid" 2>/dev/null || true
    exec {hold_fd}>&-
    rm -f -- "$ready_path" "$hold_pipe"
}

archive_v1="$(build_bundle 1111111111111111111111111111111111111111 1.0.19)"
archive_v2=''

glibc_home="$TEST_ROOT/glibc-home"
mkdir -p -- "$glibc_home"
write_stopped_state "$glibc_home"
for glibc_case in unknown 2.30; do
    if run_install_glibc "$archive_v1" "$glibc_home" "$glibc_case" >/dev/null 2>&1; then
        fail "glibc $glibc_case candidate unexpectedly accepted"
    fi
    [[ ! -L "$glibc_home/.local/share/codex-info/current" ]] || fail "glibc $glibc_case rejection published current"
done
run_install_glibc "$archive_v1" "$glibc_home" 2.31 >/dev/null
assert_symlink "$glibc_home/.local/share/codex-info/current"
printf 'case glibc unknown/older/equal compatibility: PASS\n'
glibc_newer_home="$TEST_ROOT/glibc-newer-home"
mkdir -p -- "$glibc_newer_home"
write_stopped_state "$glibc_newer_home"
run_install_glibc "$archive_v1" "$glibc_newer_home" 2.32 >/dev/null
assert_symlink "$glibc_newer_home/.local/share/codex-info/current"
printf 'case glibc newer compatibility: PASS\n'

python3 - "$archive_v1" "$ROOT_DIR/run.sh" <<'PY'
import json, pathlib, stat, sys, tarfile
archive, source = map(pathlib.Path, sys.argv[1:])
with tarfile.open(archive, "r:gz") as stream:
    members = stream.getmembers()
    names = [item.name for item in members]
    assert names == sorted(names) and len(names) == len(set(names))
    assert "manifest.json" in names and "SHA256SUMS" in names
    manifest = json.loads(stream.extractfile("manifest.json").read())
    external = json.loads(pathlib.Path(str(archive).removesuffix(".tar.gz") + ".manifest.json").read_text())
    assert manifest == external
    for entry in manifest["files"]:
        assert set(entry) == {"path", "size", "sha256", "mode"}
        assert entry["mode"] == (0o755 if entry["path"] in {"codex_info", "run.sh", "install.sh"} else 0o644)
    for item in members:
        assert stat.S_IMODE(item.mode) == (0o755 if item.name in {"codex_info","run.sh","install.sh"} else 0o644)
    assert stream.extractfile("run.sh").read() == source.read_bytes()
print("case archive/hash/modes: PASS")
PY

write_stopped_state "$fake_home"
mkdir -p -- "$fake_home/.codex" "$fake_home/.config/codex-info"
printf 'profile sentinel\n' > "$fake_home/.codex/session.jsonl"
printf 'settings sentinel\n' > "$fake_home/.config/codex-info/settings.json"
profile_hash="$(sha256sum "$fake_home/.codex/session.jsonl" "$fake_home/.config/codex-info/settings.json")"
run_install "$archive_v1" "$fake_home" >/dev/null

bad_mode_manifest="$TEST_ROOT/bad-mode.manifest.json"
python3 - "$archive_v1" "$bad_mode_manifest" <<'PY'
import json, pathlib, tarfile, sys
archive, output = sys.argv[1:]
with tarfile.open(archive, "r:gz") as stream:
    document = json.loads(stream.extractfile("manifest.json").read())
document["files"][0].pop("mode")
pathlib.Path(output).write_text(json.dumps(document) + "\n")
PY
current_before="$(readlink -- "$fake_home/.local/share/codex-info/current")"
if run_install_with_manifest "$archive_v1" "$fake_home" "$bad_mode_manifest" >/dev/null 2>&1; then
    fail 'manifest without exact mode unexpectedly accepted'
fi
[[ "$(readlink -- "$fake_home/.local/share/codex-info/current")" == "$current_before" ]] ||
    fail 'rejected mode manifest changed current generation'
printf 'case manifest mode rejection: PASS\n'

for path in \
    "$fake_home/.local/bin/codex_info" \
    "$fake_home/.local/bin/codex-info" \
    "$fake_home/.local/libexec/codex-info-install.sh" \
    "$fake_home/.local/share/codex-info/manifest.json" \
    "$fake_home/.config/systemd/user/codex-info.service" \
    "$fake_home/.config/systemd/user/codex-info-update.service" \
    "$fake_home/.config/systemd/user/codex-info-update.timer"; do
    assert_symlink "$path"
done
generation="$(readlink -- "$fake_home/.local/share/codex-info/current")"
[[ "$generation" == generations/1.0.19-* ]] || fail "unexpected generation: $generation"
python3 - "$fake_home/.local/share/codex-info/control-state.json" "${generation#generations/}" <<'PY'
import json, pathlib, sys
value = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert set(value) == {"schema","desired_state","boot_id","operation_id","generation_id","updated_at_unix"}
assert value["generation_id"] == sys.argv[2]
PY
[[ "$(sha256sum "$fake_home/.codex/session.jsonl" "$fake_home/.config/codex-info/settings.json")" == "$profile_hash" ]] ||
    fail 'profile sentinels changed during install'
printf 'case initial generation/retention: PASS\n'

write_release "$archive_v1"
run_update "$fake_home" >/dev/null
[[ -z "$(find "$update_tmp" -mindepth 1 -maxdepth 1 -print -quit)" ]] || fail 'no-update left temporary files'
printf 'case equal-version no-update: PASS\n'

rm -- "$fake_home/.config/systemd/user/codex-info.service"
run_update "$fake_home" >/dev/null
assert_symlink "$fake_home/.config/systemd/user/codex-info.service"
printf 'case equal-version repair: PASS\n'

rm -- "$fake_home/.local/share/codex-info/manifest.json"
run_update "$fake_home" >/dev/null
assert_symlink "$fake_home/.local/share/codex-info/manifest.json"
printf 'case equal-version manifest repair: PASS\n'

# Source-bound readiness fixture: the fake MainPID owns the loopback socket,
# executable identity, lock record, and recorder state for the installed tuple.
health_pid=4242
health_starttime=12345
health_generation="$(readlink -- "$fake_home/.local/share/codex-info/current")"
health_generation_dir="$fake_home/.local/share/codex-info/$health_generation"
mkdir -p -- "$fake_proc/$health_pid/fd" "$fake_home/.codex/history"
ln -s -- "$health_generation_dir/codex_info" "$fake_proc/$health_pid/exe"
ln -s -- 'socket:[9001]' "$fake_proc/$health_pid/fd/3"
python3 - "$fake_proc/$health_pid/stat" "$fake_home/.codex/history/usage_record_daemon.lock" \
    "$fake_home/.codex/history/recorder-state.json" "$health_generation_dir/codex_info" "$health_pid" "$health_starttime" <<'PY'
import hashlib, json, os, pathlib, stat, sys
stat_path, lock_path, state_path, executable, pid, starttime = sys.argv[1:]
fields = ["S"] + ["0"] * 18 + [starttime]
pathlib.Path(stat_path).write_text(f"{pid} (codex_info) " + " ".join(fields) + "\n")
metadata = os.stat(executable)
nonce = "ab" * 16
pathlib.Path(lock_path).write_text(json.dumps({
    "pid": int(pid), "started_at": 1, "starttime_ticks": int(starttime),
    "executable_device": metadata.st_dev, "executable_inode": metadata.st_ino,
    "owner_nonce": nonce,
}) + "\n")
pathlib.Path(lock_path).chmod(stat.S_IRUSR | stat.S_IWUSR)
pathlib.Path(state_path).write_text(json.dumps({
    "schema": "codex-info-recorder-state-v1", "pid": int(pid),
    "process_starttime": int(starttime), "owner_nonce": nonce,
    "write_state": "idle_no_account", "partition_id_hash": None,
    "data_generation": None, "collector_epoch": None, "cycle_seq": None,
    "last_commit_unix": None, "updated_at_unix": int(__import__('time').time()),
}) + "\n")
pathlib.Path(state_path).chmod(stat.S_IRUSR | stat.S_IWUSR)
PY
printf '  sl local_address rem_address st tx_queue tr tm->when retrnsmt uid timeout inode\n0: 0100007F:2253 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 9001 1\n' > "$fake_proc/net/tcp"
health_version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$fake_home/.local/share/codex-info/current/manifest.json")"
health_source="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["source_sha"])' "$fake_home/.local/share/codex-info/current/manifest.json")"
health_manifest="$(sha256sum "$fake_home/.local/share/codex-info/current/manifest.json" | awk '{print $1}')"
HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_RELEASE_JSON="$release_json" FAKE_RELEASE_ASSETS="$release_assets" TMPDIR="$update_tmp" \
    FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    FAKE_HEALTH_VERSION="$health_version" FAKE_HEALTH_SOURCE="$health_source" \
    FAKE_HEALTH_MANIFEST="$health_manifest" CODEX_INFO_PROC_ROOT="$fake_proc" \
    SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --verify-runtime >/dev/null
printf 'case source-bound health/readiness: PASS\n'
HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    FAKE_HEALTH_VERSION="$health_version" FAKE_DETAILS_PADDING_BYTES=$((300 * 1024)) \
    CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --verify-runtime >/dev/null
printf 'case large details response via stdin: PASS\n'
for health_shape in extra duplicate old; do
    if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
        FAKE_HEALTH_VERSION="$health_version" FAKE_HEALTH_SHAPE="$health_shape" CODEX_INFO_PROC_ROOT="$fake_proc" \
        SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$fake_home/.local/libexec/codex-info-install.sh" --verify-runtime >/dev/null 2>&1; then
        fail "health shape unexpectedly accepted: $health_shape"
    fi
done
if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    FAKE_HEALTH_VERSION="$health_version" FAKE_HEALTH_SHAPE=exact FAKE_DETAILS_STATE=error \
    CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --verify-runtime >/dev/null 2>&1; then
    fail 'runtime with error details unexpectedly passed functional readiness'
fi
state_backup="$TEST_ROOT/recorder-state.good"
cp -- "$fake_home/.codex/history/recorder-state.json" "$state_backup"
python3 - "$fake_home/.codex/history/recorder-state.json" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]); value = json.loads(path.read_text()); value["updated_at_unix"] = 1
path.write_text(json.dumps(value) + "\n")
PY
if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    FAKE_HEALTH_VERSION="$health_version" FAKE_HEALTH_SHAPE=exact CODEX_INFO_PROC_ROOT="$fake_proc" \
    SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --verify-runtime >/dev/null 2>&1; then
    fail 'stale idle recorder heartbeat unexpectedly accepted'
fi
mv -- "$state_backup" "$fake_home/.codex/history/recorder-state.json"
for recorder_case in ready degraded; do
    cp -- "$fake_home/.codex/history/recorder-state.json" "$state_backup"
    python3 - "$fake_home/.codex/history/recorder-state.json" "$recorder_case" <<'PY'
import json, pathlib, sys, time
path, case = sys.argv[1:]
value = json.loads(pathlib.Path(path).read_text())
value["write_state"] = case
value["updated_at_unix"] = int(time.time())
if case == "ready":
    value.update({"partition_id_hash": "cd" * 32, "data_generation": 1,
                  "collector_epoch": "ef" * 16, "cycle_seq": 1,
                  "last_commit_unix": 1})
pathlib.Path(path).write_text(json.dumps(value) + "\n")
PY
    if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
        FAKE_HEALTH_VERSION="$health_version" FAKE_HEALTH_SHAPE=exact CODEX_INFO_PROC_ROOT="$fake_proc" \
        SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$fake_home/.local/libexec/codex-info-install.sh" --verify-runtime >/dev/null 2>&1; then
        fail "recorder $recorder_case state unexpectedly accepted"
    fi
    mv -- "$state_backup" "$fake_home/.codex/history/recorder-state.json"
done
printf 'case health schema/heartbeat rejection: PASS\n'

write_running_state "$fake_home"
if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_RELEASE_FAILURE=1 FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    FAKE_HEALTH_VERSION="$health_version" CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --update >/dev/null 2>&1; then
    fail 'release API failure unexpectedly reported success'
fi
grep -Fq 'curl --fail --silent --show-error --proto =https' "$log" || fail 'release API failure fixture did not query discovery'
grep -Fq '127.0.0.1:8787/v1/health' "$log" || fail 'release API failure did not read back coherent B'
grep -Fq '127.0.0.1:8787/v1/details' "$log" || fail 'release API failure did not verify functional readiness'
write_stopped_state "$fake_home"
printf 'case release-failure coherent-B readback: PASS\n'

clock_file="$TEST_ROOT/readiness-clock"
printf '%s\n' "$(date +%s)" > "$clock_file"
clock_bin="$TEST_ROOT/readiness-clock.sh"
sleep_bin="$TEST_ROOT/readiness-sleep.sh"
# shellcheck disable=SC2016
printf '%s\n' '#!/usr/bin/env bash' 'cat -- "$READINESS_CLOCK_FILE"' > "$clock_bin"
# shellcheck disable=SC2016
printf '%s\n' '#!/usr/bin/env bash' 'value="$(<"$READINESS_CLOCK_FILE")"' 'printf "%s\\n" "$((value + ${1:-1}))" > "$READINESS_CLOCK_FILE"' > "$sleep_bin"
chmod 0755 "$clock_bin" "$sleep_bin"
health_count="$TEST_ROOT/health-count"
: > "$health_count"
write_stopped_state "$fake_home"
HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_RELEASE_JSON="$release_json" FAKE_RELEASE_ASSETS="$release_assets" TMPDIR="$update_tmp" \
    FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    FAKE_HEALTH_VERSION="$health_version" FAKE_HEALTH_FAILURES=2 FAKE_HEALTH_COUNT_FILE="$health_count" \
    FAKE_HEALTH_SHAPE=exact CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    CODEX_INFO_CLOCK_BIN="$clock_bin" CODEX_INFO_SLEEP_BIN="$sleep_bin" READINESS_CLOCK_FILE="$clock_file" \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --start >/dev/null
[[ "$(<"$health_count")" -ge 3 ]] || fail 'readiness did not retry transient health failure'
write_stopped_state "$fake_home"
printf 'case bounded readiness retry: PASS\n'

# The next fixture models a clean stopped service; leave the prior health
# owner out of the synthetic proc tree so the updater need not retire it.
: > "$fake_proc/net/tcp"
printf 'fixture binary generation two\n' > "$fixture_root/codex_info"
archive_v2="$(build_bundle 2222222222222222222222222222222222222222 1.0.20)"
write_release "$archive_v2"
if CODEX_INFO_INTERRUPT_PHASE=current_switched run_update "$fake_home" >/dev/null 2>&1; then
    fail 'current_switched interruption unexpectedly succeeded'
fi
grep -Fq '"phase": "current_switched"' "$fake_home/.local/share/codex-info/install-transaction.json" ||
    fail 'interruption did not persist current_switched journal'
python3 - "$fake_home/.local/share/codex-info/install-transaction.json" <<'PY'
import json, pathlib, sys
value = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert set(value) == {"schema","operation_id","owner_pid","owner_starttime","boot_id",
                      "phase","old_generation","new_generation","desired_state","updated_at_unix"}
assert value["owner_pid"] > 0 and value["owner_starttime"] > 0
PY
run_update "$fake_home" >/dev/null
grep -Fq '"phase": "committed"' "$fake_home/.local/share/codex-info/install-transaction.json" ||
    fail 'journal did not resume to committed'
[[ "$(readlink -- "$fake_home/.local/share/codex-info/current")" == generations/1.0.20-* ]] ||
    fail 'resume did not converge to v2'
printf 'case journal interruption/resume: PASS\n'

# A live installer may let systemd activate only the exact switched
# generation while it still owns descriptor-9.  Rollback uses the exact
# predecessor under the same rule; direct, stale, mismatched, and malformed
# journal states remain fail-closed.
write_running_state "$fake_home"
condition_generation="$(readlink -- "$fake_home/.local/share/codex-info/current")"
condition_generation="${condition_generation#generations/}"
cp -- "$fake_home/.local/share/codex-info/install-transaction.json" \
    "$TEST_ROOT/committed-journal-before-condition-cases.json"
run_active_startup_condition_case "$fake_home" current_switched '' "$condition_generation" pass candidate
if run_startup_condition "$fake_home" >/dev/null 2>&1; then
    fail 'stale transaction journal unexpectedly authorized startup condition'
fi
if run_startup_reconcile "$fake_home" >/dev/null 2>&1; then
    fail 'stale transaction journal unexpectedly authorized startup reconcile'
fi
run_active_startup_condition_case "$fake_home" rollback_switched "$condition_generation" '' pass rollback
mismatch_generation="9.9.9-$(printf 'f%.0s' {1..40})-$(printf 'e%.0s' {1..64})"
run_active_startup_condition_case "$fake_home" current_switched '' "$mismatch_generation" fail mismatch
printf '{}\n' > "$fake_home/.local/share/codex-info/install-transaction.json"
chmod 0600 "$fake_home/.local/share/codex-info/install-transaction.json"
if run_startup_reconcile "$fake_home" >/dev/null 2>&1 || run_startup_condition "$fake_home" >/dev/null 2>&1; then
    fail 'malformed transaction journal unexpectedly authorized startup condition'
fi
cp -- "$TEST_ROOT/committed-journal-before-condition-cases.json" \
    "$fake_home/.local/share/codex-info/install-transaction.json"
printf 'case active transaction exact-generation condition/rollback safety: PASS\n'

# An active MainPID from a retained, but no-longer-current, managed generation
# is repairable through systemd.  The updater must restart the unit and never
# signal the known process directly; readiness then remains bounded because
# this fixture intentionally does not recreate its listener.
write_running_state "$fake_home"
restart_log_start="$(wc -l < "$log")"
if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_RELEASE_JSON="$release_json" FAKE_RELEASE_ASSETS="$release_assets" TMPDIR="$update_tmp" \
    FAKE_MAIN_ENABLED=1 FAKE_MAIN_ACTIVE=1 FAKE_MAIN_PID="$health_pid" \
    CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    CODEX_INFO_CLOCK_BIN="$clock_bin" CODEX_INFO_SLEEP_BIN="$sleep_bin" READINESS_CLOCK_FILE="$clock_file" \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --update >/dev/null 2>&1; then
    fail 'known old managed MainPID unexpectedly reached a healthy terminal'
fi
tail -n +$((restart_log_start + 1)) "$log" | grep -Fq 'systemctl --user restart --no-block codex-info.service' ||
    fail 'known old managed MainPID was not repaired through systemd restart'
tail -n +$((restart_log_start + 1)) "$log" | grep -Fq 'systemctl --user reset-failed codex-info.service' ||
    fail 'managed restart did not reset the start-limit epoch'
write_stopped_state "$fake_home"
printf 'case known-managed wrong-PID systemd repair: PASS\n'

exec {held_fd}>"$fake_home/.local/share/codex-info/.install.lock"
flock --exclusive --nonblock "$held_fd"
if run_update "$fake_home" >/dev/null 2>&1; then
    fail 'concurrent L1 operation unexpectedly succeeded'
fi
exec {held_fd}>&-
printf 'case concurrent-L1 rejection: PASS\n'

legacy_home="$TEST_ROOT/legacy-home"
write_stopped_state "$legacy_home"
mkdir -p -- "$legacy_home/.local/bin" "$legacy_home/.local/libexec" \
    "$legacy_home/.local/share/codex-info" "$legacy_home/.config/systemd/user"
tar -xOf "$archive_v1" codex_info > "$legacy_home/.local/bin/codex_info"
tar -xOf "$archive_v1" install.sh > "$legacy_home/.local/libexec/codex-info-install.sh"
tar -xOf "$archive_v1" codex-info.service > "$legacy_home/.config/systemd/user/codex-info.service"
tar -xOf "$archive_v1" codex-info-update.service > "$legacy_home/.config/systemd/user/codex-info-update.service"
tar -xOf "$archive_v1" codex-info-update.timer > "$legacy_home/.config/systemd/user/codex-info-update.timer"
chmod 0755 "$legacy_home/.local/bin/codex_info" "$legacy_home/.local/libexec/codex-info-install.sh"
chmod 0644 "$legacy_home/.config/systemd/user/codex-info.service" \
    "$legacy_home/.config/systemd/user/codex-info-update.service" \
    "$legacy_home/.config/systemd/user/codex-info-update.timer"
python3 - "$archive_v1" "$legacy_home/.local/share/codex-info/manifest.json" <<'PY'
import json, pathlib, tarfile, sys
archive_name, manifest_name = sys.argv[1:]
with tarfile.open(archive_name, "r:gz") as archive:
    document = json.loads(archive.extractfile("manifest.json").read())
document["files"] = [
    {key: value for key, value in entry.items() if key != "mode"}
    for entry in document["files"] if entry["path"] != "run.sh"
]
pathlib.Path(manifest_name).write_text(json.dumps(document, indent=2) + "\n")
PY
chmod 0644 "$legacy_home/.local/share/codex-info/manifest.json"
run_install "$archive_v1" "$legacy_home" >/dev/null
[[ "$(find "$legacy_home/.local/share/codex-info/legacy-backups" -type f | wc -l)" -ge 6 ]] ||
    fail 'legacy regular files were not backed up'
printf 'case legacy migration/journal: PASS\n'

run_remove() {
    HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
        CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$fake_home/.local/libexec/codex-info-install.sh" --remove
}
run_remove >/dev/null
for path in \
    "$fake_home/.config/systemd/user/codex-info.service" \
    "$fake_home/.config/systemd/user/codex-info-update.service" \
    "$fake_home/.config/systemd/user/codex-info-update.timer"; do
    [[ ! -e "$path" && ! -L "$path" ]] || fail "remove retained unit link: $path"
done
assert_symlink "$fake_home/.local/bin/codex-info"
assert_symlink "$fake_home/.local/libexec/codex-info-install.sh"
assert_symlink "$fake_home/.local/share/codex-info/current"
assert_file "$fake_home/.codex/session.jsonl"
python3 - "$fake_home/.local/share/codex-info/control-state.json" <<'PY'
import json, pathlib, sys
value = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert set(value) == {"schema","desired_state","boot_id","operation_id","generation_id","updated_at_unix"}
assert value["desired_state"] == "removed"
PY
printf 'case remove-retention/control-state: PASS\n'

# Removal is reversible through the retained verified generation: --start
# must republish only the missing unit links before attempting activation.
write_running_state "$fake_home"
if HOME="$fake_home" CODEX_HOME="$fake_home/.codex" PATH="$fake_bin:$ORIGINAL_PATH" FAKE_LOG="$log" \
    FAKE_RELEASE_JSON="$release_json" FAKE_RELEASE_ASSETS="$release_assets" TMPDIR="$update_tmp" \
    CODEX_INFO_PROC_ROOT="$fake_proc" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
    CODEX_INFO_CLOCK_BIN="$clock_bin" CODEX_INFO_SLEEP_BIN="$sleep_bin" READINESS_CLOCK_FILE="$clock_file" \
    bash "$fake_home/.local/libexec/codex-info-install.sh" --start >/dev/null 2>&1; then
    fail 'remove-to-start fixture unexpectedly reached healthy fake service'
fi
for path in \
    "$fake_home/.config/systemd/user/codex-info.service" \
    "$fake_home/.config/systemd/user/codex-info-update.service" \
    "$fake_home/.config/systemd/user/codex-info-update.timer"; do
    assert_symlink "$path"
done
write_stopped_state "$fake_home"
printf 'case remove-to-start unit republish: PASS\n'

grep -Fq 'OnActiveSec=5min' "$ROOT_DIR/packaging/codex-info-update.timer"
grep -Fq 'OnUnitActiveSec=1h' "$ROOT_DIR/packaging/codex-info-update.timer"
grep -Fq 'AccuracySec=1s' "$ROOT_DIR/packaging/codex-info-update.timer"
grep -Fq 'Restart=always' "$ROOT_DIR/packaging/codex-info.service"
grep -Fq 'RestartSec=5s' "$ROOT_DIR/packaging/codex-info.service"
grep -Fq 'StartLimitIntervalSec=0' "$ROOT_DIR/packaging/codex-info.service"
if grep -q '^StartLimitBurst=' "$ROOT_DIR/packaging/codex-info.service"; then
    fail 'daemon start-rate limiting must stay disabled'
fi
grep -Fq 'TimeoutStartSec=1h20min31s' "$ROOT_DIR/packaging/codex-info-update.service"
printf 'linux bundle contract cases passed\n'

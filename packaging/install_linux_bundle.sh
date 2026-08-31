#!/usr/bin/env bash
set -euo pipefail

# Bundle-only consumer.  In particular, this file must remain usable after a
# source checkout is gone: it never resolves build metadata or source-tree
# files.  Every installed byte is read from the supplied archive after the
# external checksum and manifest have been validated.
TARGET="x86_64-unknown-linux-gnu"
COMPATIBILITY="glibc"
PRODUCT="codex_info"
SCHEMA="codex-info-linux-bundle-v1"
ACTION="install"
ARCHIVE=""
MANIFEST=""
CHECKSUM=""
SYSTEMCTL_BIN="${SYSTEMCTL_BIN:-systemctl}"
CURL_BIN="${CURL_BIN:-curl}"
HEALTH_URL="http://127.0.0.1:8787/v1/health"

usage() {
    cat <<'EOF'
usage: install.sh --bundle ARCHIVE [--manifest FILE] [--sha256 FILE]
       install.sh --remove

The install path consumes only the supplied tar.gz, its external SHA-256
file, and its manifest.  --remove removes the user unit only while retaining
the installed binary and profile data.
EOF
}

die() {
    echo "linux-bundle-install: $*" >&2
    exit 1
}

while (($# > 0)); do
    case "$1" in
        --bundle|--archive)
            (($# >= 2)) || die "$1 requires a tar.gz path"
            ARCHIVE="$2"
            shift 2
            ;;
        --manifest)
            (($# >= 2)) || die '--manifest requires a path'
            MANIFEST="$2"
            shift 2
            ;;
        --sha256|--checksum)
            (($# >= 2)) || die "$1 requires a path"
            CHECKSUM="$2"
            shift 2
            ;;
        --remove)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] ||
                die '--remove cannot be combined with bundle options'
            ACTION="remove"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            (($# == 0)) || die 'unexpected positional argument'
            ;;
        *)
            if [[ -z "$ARCHIVE" && "$1" != -* ]]; then
                ARCHIVE="$1"
                shift
            else
                die "unknown argument: $1"
            fi
            ;;
    esac
done

command -v "$SYSTEMCTL_BIN" >/dev/null 2>&1 || die "$SYSTEMCTL_BIN is required"

unit_dir="$HOME/.config/systemd/user"
local_bin="$HOME/.local/bin"
binary_destination="$local_bin/codex_info"
unit_destination="$unit_dir/codex-info.service"

require_user_manager() {
    "$SYSTEMCTL_BIN" --user show-environment >/dev/null 2>&1 ||
        die 'systemd user manager is unavailable'
}

remove_installation() {
    require_user_manager
    # This is intentionally the complete deletion allowlist.  The installed
    # binary is retained just like the existing systemd installer; profile DB,
    # verified backups, reset hints, session JSONL, and configuration do not
    # occur in this transaction.
    "$SYSTEMCTL_BIN" --user disable --now codex-info.service >/dev/null 2>&1 ||
        die 'could not stop and disable codex-info.service during remove'
    mkdir -p -- "$unit_dir"
    [[ ! -d "$unit_destination" ]] || die "unit path is a directory: $unit_destination"
    rm -f -- "$unit_destination"
    "$SYSTEMCTL_BIN" --user daemon-reload >/dev/null 2>&1 ||
        die 'systemd user daemon-reload failed during remove'
    "$SYSTEMCTL_BIN" --user reset-failed codex-info.service >/dev/null 2>&1 ||
        die 'systemd user reset-failed failed during remove'
    printf 'removed unit=%s (binary and profile data preserved)\n' "$unit_destination"
}

if [[ "$ACTION" == remove ]]; then
    [[ -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] ||
        die '--remove cannot be combined with bundle options'
    remove_installation
    exit 0
fi

command -v "$CURL_BIN" >/dev/null 2>&1 || die "$CURL_BIN is required"

[[ -n "$ARCHIVE" ]] || die '--bundle ARCHIVE is required'
[[ -f "$ARCHIVE" && ! -L "$ARCHIVE" ]] || die "bundle archive is not a regular file: $ARCHIVE"
[[ "$ARCHIVE" == *.tar.gz ]] || die 'bundle archive must have a .tar.gz suffix'

archive_dir="$(cd -- "$(dirname -- "$ARCHIVE")" && pwd)"
archive_file="$(basename -- "$ARCHIVE")"
ARCHIVE="$archive_dir/$archive_file"
[[ -n "$CHECKSUM" ]] || CHECKSUM="$ARCHIVE.sha256"
[[ -n "$MANIFEST" ]] || MANIFEST="${ARCHIVE%.tar.gz}.manifest.json"
[[ -f "$CHECKSUM" && ! -L "$CHECKSUM" ]] ||
    die "external SHA-256 file is not a regular file: $CHECKSUM"
[[ -f "$MANIFEST" && ! -L "$MANIFEST" ]] ||
    die "manifest is not a regular file: $MANIFEST"

checksum_count="$(awk 'NF && $0 !~ /^[[:space:]]*#/ { count++ } END { print count + 0 }' "$CHECKSUM")"
[[ "$checksum_count" == 1 ]] || die 'external SHA-256 file must contain exactly one checksum record'
read -r expected_hash expected_name checksum_extra < "$CHECKSUM" ||
    die 'could not read external SHA-256 record'
[[ -z "${checksum_extra:-}" ]] || die 'external SHA-256 record has extra fields'
expected_name="${expected_name#\*}"
[[ "$expected_name" == "$archive_file" ]] ||
    die "checksum names $expected_name, expected $archive_file"
[[ "$expected_hash" =~ ^[[:xdigit:]]{64}$ ]] || die 'external SHA-256 value is invalid'
actual_hash="$(sha256sum -- "$ARCHIVE" | awk '{print $1}')"
[[ "$actual_hash" == "$expected_hash" ]] || die 'bundle SHA-256 does not match external checksum'

manifest_validation="$(mktemp)"
cleanup_manifest_validation() {
    rm -f -- "$manifest_validation"
}
trap cleanup_manifest_validation EXIT

python3 - "$ARCHIVE" "$MANIFEST" "$SCHEMA" "$PRODUCT" "$TARGET" \
    "$COMPATIBILITY" <<'PY' >"$manifest_validation"
import hashlib
import json
import pathlib
import re
import sys
import tarfile

archive_name, manifest_name, expected_schema, expected_product, expected_target, expected_compatibility = sys.argv[1:]

def reject(message):
    raise SystemExit(f"manifest/archive validation failed: {message}")

def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            reject(f"duplicate JSON key: {key}")
        result[key] = value
    return result

try:
    with pathlib.Path(manifest_name).open("r", encoding="utf-8") as stream:
        manifest = json.load(stream, object_pairs_hook=unique_pairs)
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    reject(f"invalid manifest JSON: {error}")

if not isinstance(manifest, dict):
    reject("manifest root is not an object")
required = {
    "schema", "product", "version", "source_sha", "run_id", "run_attempt",
    "target", "compatibility", "files",
}
missing = required.difference(manifest)
if missing:
    reject("missing fields: " + ", ".join(sorted(missing)))
if manifest["product"] != expected_product:
    reject("unexpected product")
if manifest["schema"] != expected_schema:
    reject("unexpected schema")
if not isinstance(manifest["version"], str) or not re.fullmatch(
    r"[0-9]+\.[0-9]+\.[0-9]+", manifest["version"]
):
    reject("invalid version")
if not isinstance(manifest["source_sha"], str) or not re.fullmatch(r"[0-9a-f]{40}", manifest["source_sha"]):
    reject("invalid source_sha")
for field in ("run_id",):
    value = manifest[field]
    if not isinstance(value, str) or not re.fullmatch(r"[1-9][0-9]*", value):
        reject(f"invalid {field}")
if isinstance(manifest["run_attempt"], bool) or not isinstance(manifest["run_attempt"], int):
    reject("run_attempt is not an integer")
if manifest["run_attempt"] < 1:
    reject("run_attempt is not positive")
if manifest["target"] != expected_target:
    reject("unexpected target")
if manifest["compatibility"] != expected_compatibility:
    reject("unexpected compatibility")
glibc_minimum = manifest.get("glibc_minimum")
if not isinstance(glibc_minimum, str) or not re.fullmatch(r"[0-9]+\.[0-9]+", glibc_minimum):
    reject("invalid glibc_minimum")

def glibc_version():
    import subprocess
    for command in (("getconf", "GNU_LIBC_VERSION"), ("ldd", "--version")):
        try:
            output = subprocess.check_output(command, text=True, stderr=subprocess.STDOUT)
        except (OSError, subprocess.CalledProcessError):
            continue
        match = re.search(r"(?:glibc|GNU libc)[^0-9]*([0-9]+\.[0-9]+)", output, re.IGNORECASE)
        if match:
            return match.group(1)
    return None

host_glibc = glibc_version()
if host_glibc is None:
    reject("could not measure host glibc")
def version_tuple(value):
    return tuple(int(part) for part in value.split("."))
if version_tuple(host_glibc) < version_tuple(glibc_minimum):
    reject(f"host glibc {host_glibc} is below required {glibc_minimum}")

entries = manifest["files"]
if not isinstance(entries, list) or not entries:
    reject("files is not a non-empty array")
entry_by_path = {}
for entry in entries:
    if not isinstance(entry, dict):
        reject("files contains a non-object")
    for key in ("path", "size", "sha256"):
        if key not in entry:
            reject(f"file entry misses {key}")
    path = entry["path"]
    if (
        not isinstance(path, str)
        or not path
        or any(part in {"", ".", ".."} for part in path.split("/"))
        or path.startswith("/")
        or "\\" in path
        or path.startswith("./")
        or "/../" in f"/{path}/"
        or path == ".."
    ):
        reject(f"unsafe file path: {path!r}")
    if path in entry_by_path:
        reject(f"duplicate file path: {path}")
    if isinstance(entry["size"], bool) or not isinstance(entry["size"], int) or entry["size"] < 0:
        reject(f"invalid size for {path}")
    digest = entry["sha256"]
    if not isinstance(digest, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", digest):
        reject(f"invalid SHA-256 for {path}")
    entry_by_path[path] = entry

for required_path in ("codex_info", "codex-info.service", "install.sh", "LICENSE", "COPYRIGHT"):
    if required_path not in entry_by_path:
        reject(f"required file is missing: {required_path}")
if "THIRD_PARTY_NOTICES.md" not in entry_by_path and "NOTICE.txt" not in entry_by_path:
    reject("a third-party notice file is required")

metadata_paths = {"manifest.json", "SHA256SUMS"}
try:
    with tarfile.open(archive_name, mode="r:gz") as archive:
        members = archive.getmembers()
        actual_paths = []
        for member in members:
            path = member.name
            if (
                not path
                or any(part in {"", ".", ".."} for part in path.split("/"))
                or path.startswith("/")
                or "\\" in path
                or path.startswith("./")
                or "/../" in f"/{path}/"
                or path == ".."
                or not member.isfile()
            ):
                reject(f"archive member is unsafe or not regular: {path!r}")
            if path in actual_paths:
                reject(f"archive member is duplicated: {path}")
            actual_paths.append(path)
        if not metadata_paths.issubset(actual_paths):
            reject("archive metadata files are missing")
        payload_paths = set(actual_paths).difference(metadata_paths)
        if payload_paths != set(entry_by_path) or len(payload_paths) != len(entry_by_path):
            reject("archive members and manifest files differ")
        internal_manifest = archive.extractfile(archive.getmember("manifest.json"))
        if internal_manifest is None:
            reject("cannot read archive manifest")
        internal_bytes = internal_manifest.read()
        try:
            internal = json.loads(internal_bytes.decode("utf-8"), object_pairs_hook=unique_pairs)
        except (UnicodeError, json.JSONDecodeError) as error:
            reject(f"invalid internal manifest JSON: {error}")
        if internal != manifest:
            reject("internal and external manifests differ")

        sums_file = archive.extractfile(archive.getmember("SHA256SUMS"))
        if sums_file is None:
            reject("cannot read archive SHA256SUMS")
        sum_entries = {}
        for raw_line in sums_file.read().decode("utf-8").splitlines():
            pieces = raw_line.split()
            if len(pieces) != 2 or not re.fullmatch(r"[0-9a-fA-F]{64}", pieces[0]):
                reject("invalid internal SHA256SUMS record")
            name = pieces[1].removeprefix("*")
            if name in sum_entries:
                reject(f"duplicate internal SHA256SUMS record: {name}")
            if name not in set(actual_paths).difference({"SHA256SUMS"}):
                reject(f"internal SHA256SUMS names unknown file: {name}")
            sum_entries[name] = pieces[0].lower()
        expected_sum_paths = set(actual_paths).difference({"SHA256SUMS"})
        if set(sum_entries) != expected_sum_paths:
            reject("internal SHA256SUMS does not cover archive files")
        for path, expected_digest in sum_entries.items():
            stream = archive.extractfile(archive.getmember(path))
            if stream is None:
                reject(f"cannot read checksummed member: {path}")
            digest = hashlib.sha256()
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
            if digest.hexdigest() != expected_digest:
                reject(f"internal SHA256SUMS mismatch for {path}")
        for path in actual_paths:
            if path in metadata_paths:
                continue
            member = archive.getmember(path)
            entry = entry_by_path[path]
            if member.size != entry["size"]:
                reject(f"size mismatch for {path}")
            if path == "codex_info" and not (member.mode & 0o111):
                reject("codex_info is not executable")
            if path == "install.sh" and not (member.mode & 0o111):
                reject(f"{path} is not executable")
            stream = archive.extractfile(member)
            if stream is None:
                reject(f"cannot read archive member: {path}")
            digest = hashlib.sha256()
            while True:
                chunk = stream.read(1024 * 1024)
                if not chunk:
                    break
                digest.update(chunk)
            if digest.hexdigest() != entry["sha256"].lower():
                reject(f"content SHA-256 mismatch for {path}")
except (OSError, tarfile.TarError) as error:
    reject(f"invalid gzip tar archive: {error}")

print("validated\t" + manifest["version"])
PY
validated_line="$(head -n 1 -- "$manifest_validation")"
[[ "$validated_line" == $'validated\t'* ]] || die 'bundle validation did not complete'
bundle_version="$(printf '%s' "$validated_line" | cut -f2-)"
[[ -n "$bundle_version" ]] || die 'validated bundle version is empty'
rm -f -- "$manifest_validation"
trap - EXIT

require_user_manager
mkdir -p -- "$local_bin" "$unit_dir"
[[ ! -d "$binary_destination" ]] || die "binary path is a directory: $binary_destination"
[[ ! -d "$unit_destination" ]] || die "unit path is a directory: $unit_destination"

stage_root=''
binary_stage=''
unit_stage=''
binary_backup="$local_bin/.codex_info.previous.$$"
unit_backup="$unit_dir/.codex-info.previous.$$"
had_unit=0
binary_backed_up=0
unit_backed_up=0
binary_moved=0
unit_moved=0

cleanup_install_staging() {
    if [[ -n "$binary_stage" ]]; then
        rm -f -- "$binary_stage"
        binary_stage=''
    fi
    if [[ -n "$unit_stage" ]]; then
        rm -f -- "$unit_stage"
        unit_stage=''
    fi
    if [[ -n "$stage_root" && -d "$stage_root" ]]; then
        rm -r -- "$stage_root"
        stage_root=''
    fi
}

trap cleanup_install_staging EXIT

stage_root="$(mktemp -d "$local_bin/.codex-info-linux-stage.XXXXXX")"
binary_stage="$(mktemp "$local_bin/.codex-info-linux-binary.XXXXXX")"
unit_stage="$(mktemp "$unit_dir/.codex-info-linux-unit.XXXXXX")"

rollback_install() {
    local rollback_ok=1
    if ((unit_moved)); then
        rm -f -- "$unit_destination" || rollback_ok=0
    fi
    if ((unit_backed_up)); then
        mv -- "$unit_backup" "$unit_destination" || rollback_ok=0
    fi
    if ((binary_moved)); then
        rm -f -- "$binary_destination" || rollback_ok=0
    fi
    if ((binary_backed_up)); then
        mv -- "$binary_backup" "$binary_destination" || rollback_ok=0
    fi
    "$SYSTEMCTL_BIN" --user daemon-reload >/dev/null 2>&1 || rollback_ok=0
    # If the old unit was running, ask systemd to put that exact old unit back
    # into service.  A failing restart remains a failed installation, but it
    # must never turn into a successful report.
    if ((had_unit)); then
        "$SYSTEMCTL_BIN" --user restart codex-info.service >/dev/null 2>&1 || rollback_ok=0
    else
        # A first install may have enabled and started a new unit before its
        # health check failed.  There is no previous unit to restart, so
        # disable and stop the failed generation before returning.
        "$SYSTEMCTL_BIN" --user disable --now codex-info.service >/dev/null 2>&1 || rollback_ok=0
    fi
    ((rollback_ok)) || echo 'linux-bundle-install: rollback was not fully confirmed' >&2
}

install -m 0755 -- /dev/null "$binary_stage"
install -m 0644 -- /dev/null "$unit_stage"
tar --extract --gzip --file="$ARCHIVE" --directory="$stage_root" --no-same-owner
install -m 0755 -- "$stage_root/codex_info" "$binary_stage"
install -m 0644 -- "$stage_root/codex-info.service" "$unit_stage"

if [[ -e "$binary_destination" || -L "$binary_destination" ]]; then
    if ! mv -- "$binary_destination" "$binary_backup"; then
        cleanup_install_staging
        die 'could not stage existing binary for atomic update'
    fi
    binary_backed_up=1
fi
if [[ -e "$unit_destination" || -L "$unit_destination" ]]; then
    had_unit=1
    if ! mv -- "$unit_destination" "$unit_backup"; then
        restore_ok=1
        if ((binary_backed_up)); then
            mv -- "$binary_backup" "$binary_destination" || restore_ok=0
            binary_backed_up=0
        fi
        ((restore_ok)) || echo 'linux-bundle-install: could not restore binary after unit staging failure' >&2
        cleanup_install_staging
        die 'could not stage existing unit for atomic update'
    fi
    unit_backed_up=1
fi
if ! mv -- "$binary_stage" "$binary_destination"; then
    rollback_install
    cleanup_install_staging
    die 'could not atomically publish binary'
fi
binary_moved=1
if ! mv -- "$unit_stage" "$unit_destination"; then
    rollback_install
    cleanup_install_staging
    die 'could not atomically publish unit'
fi
unit_moved=1

health_response=''
if ! "$SYSTEMCTL_BIN" --user daemon-reload >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user enable codex-info.service >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user restart codex-info.service >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user is-active --quiet codex-info.service >/dev/null 2>&1 ||
    ! health_response="$("$CURL_BIN" --fail --silent --show-error --max-time 5 "$HEALTH_URL")";
then
    rollback_install
    cleanup_install_staging
    die 'systemd activation or health check failed; previous binary/unit retained'
fi
health_response_file="$stage_root/health-response.json"
printf '%s' "$health_response" > "$health_response_file"
if ! python3 - "$bundle_version" "$health_response_file" <<'PY'
import json
import pathlib
import sys

expected = sys.argv[1]
try:
    with pathlib.Path(sys.argv[2]).open("r", encoding="utf-8") as stream:
        document = json.load(stream)
except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid health response: {error}")
if not isinstance(document, dict) or document.get("api_version") != "v1" or \
   document.get("service") != "codex-info" or document.get("product_version") != expected:
    raise SystemExit("health response does not identify the installed version")
PY
then
    rollback_install
    cleanup_install_staging
    die 'health response version does not match the installed bundle; previous binary/unit retained'
fi

rm -f -- "$binary_backup" "$unit_backup"
cleanup_install_staging
printf 'installed and healthy unit=%s binary=%s target=%s\n' \
    "$unit_destination" "$binary_destination" "$TARGET"

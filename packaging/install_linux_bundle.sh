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
HEALTH_READY_ATTEMPTS=10
REPOSITORY="salty919/codex_info_v2"
RELEASES_URL="https://api.github.com/repos/salty919/codex_info_v2/releases?per_page=100"

usage() {
    cat <<'EOF'
usage: install.sh --bundle ARCHIVE [--manifest FILE] [--sha256 FILE]
       install.sh --update
       install.sh --remove

The install path consumes only the supplied tar.gz, its external SHA-256
file, and its manifest.  --update checks the fixed public stable release
channel and uses the same validated install path.  --remove removes user
units while retaining the installed binary, updater, manifest, and profile
data.
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
        --update)
            [[ "$ACTION" == install && -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] ||
                die '--update cannot be combined with bundle options'
            ACTION="update"
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

unit_dir="$HOME/.config/systemd/user"
local_bin="$HOME/.local/bin"
local_libexec="$HOME/.local/libexec"
share_dir="$HOME/.local/share/codex-info"
binary_destination="$local_bin/codex_info"
unit_destination="$unit_dir/codex-info.service"
installer_destination="$local_libexec/codex-info-install.sh"
manifest_destination="$share_dir/manifest.json"
update_service_destination="$unit_dir/codex-info-update.service"
update_timer_destination="$unit_dir/codex-info-update.timer"
install_lock="$share_dir/.install.lock"

command -v "$SYSTEMCTL_BIN" >/dev/null 2>&1 || die "$SYSTEMCTL_BIN is required"
command -v flock >/dev/null 2>&1 || die 'flock is required'
mkdir -p -- "$share_dir"
if [[ "${CODEX_INFO_INSTALL_LOCKED:-}" == 1 ]]; then
    [[ -e /proc/self/fd/9 ]] || die 'inherited installer lock is unavailable'
    lock_identity="$(stat -Lc '%d:%i' -- "$install_lock")" ||
        die 'could not identify the installer lock file'
    inherited_lock_identity="$(stat -Lc '%d:%i' -- /proc/self/fd/9)" ||
        die 'could not identify the inherited installer lock'
    [[ "$inherited_lock_identity" == "$lock_identity" ]] ||
        die 'inherited installer lock does not match the installation lock'
    flock --exclusive --nonblock 9 || die 'inherited installer lock is not held'
else
    exec 9>"$install_lock"
    flock --exclusive --nonblock 9 || die 'another install, update, or remove is already running'
    export CODEX_INFO_INSTALL_LOCKED=1
fi

require_user_manager() {
    "$SYSTEMCTL_BIN" --user show-environment >/dev/null 2>&1 ||
        die 'systemd user manager is unavailable'
}

remove_installation() {
    require_user_manager
    local main_load_state update_service_load_state update_timer_load_state
    main_load_state="$("$SYSTEMCTL_BIN" --user show --property=LoadState --value codex-info.service)" ||
        die 'could not inspect codex-info.service during remove'
    update_service_load_state="$("$SYSTEMCTL_BIN" --user show --property=LoadState --value codex-info-update.service)" ||
        die 'could not inspect codex-info-update.service during remove'
    update_timer_load_state="$("$SYSTEMCTL_BIN" --user show --property=LoadState --value codex-info-update.timer)" ||
        die 'could not inspect codex-info-update.timer during remove'
    for load_state in "$main_load_state" "$update_service_load_state" "$update_timer_load_state"; do
        [[ "$load_state" =~ ^(loaded|not-found|masked|error|bad-setting)$ ]] ||
            die "unexpected systemd unit load state during remove: $load_state"
    done
    # This is intentionally the complete deletion allowlist.  The installed
    # binary, persistent installer/manifest, profile DB, verified backups,
    # reset hints, session JSONL, and configuration do not occur in this
    # transaction.
    if [[ "$update_timer_load_state" != not-found ]]; then
        "$SYSTEMCTL_BIN" --user disable --now codex-info-update.timer >/dev/null 2>&1 ||
            die 'could not stop and disable codex-info-update.timer during remove'
    fi
    if [[ "$update_service_load_state" != not-found ]]; then
        "$SYSTEMCTL_BIN" --user stop codex-info-update.service >/dev/null 2>&1 ||
            die 'could not stop codex-info-update.service during remove'
    fi
    if [[ "$main_load_state" != not-found ]]; then
        "$SYSTEMCTL_BIN" --user disable --now codex-info.service >/dev/null 2>&1 ||
            die 'could not stop and disable codex-info.service during remove'
    fi
    mkdir -p -- "$unit_dir"
    [[ ! -d "$unit_destination" ]] || die "unit path is a directory: $unit_destination"
    [[ ! -d "$update_service_destination" ]] ||
        die "unit path is a directory: $update_service_destination"
    [[ ! -d "$update_timer_destination" ]] ||
        die "unit path is a directory: $update_timer_destination"
    rm -f -- "$unit_destination"
    rm -f -- "$update_service_destination" "$update_timer_destination"
    "$SYSTEMCTL_BIN" --user daemon-reload >/dev/null 2>&1 ||
        die 'systemd user daemon-reload failed during remove'
    if [[ "$main_load_state" != not-found ]]; then
        "$SYSTEMCTL_BIN" --user reset-failed codex-info.service >/dev/null 2>&1 ||
            die 'systemd user reset-failed failed during remove'
    fi
    if [[ "$update_service_load_state" != not-found ]]; then
        "$SYSTEMCTL_BIN" --user reset-failed codex-info-update.service >/dev/null 2>&1 ||
            die 'systemd user reset-failed failed for update service during remove'
    fi
    printf 'removed units=%s,%s,%s (binary, updater, manifest, and profile data preserved)\n' \
        "$unit_destination" "$update_service_destination" "$update_timer_destination"
}

read_installed_version() {
    [[ -f "$manifest_destination" && ! -L "$manifest_destination" ]] ||
        die "installed manifest is not a regular file: $manifest_destination"
    python3 - "$manifest_destination" "$SCHEMA" "$PRODUCT" "$TARGET" \
        "$COMPATIBILITY" <<'PY'
import json
import pathlib
import re
import sys

manifest_name, expected_schema, expected_product, expected_target, expected_compatibility = sys.argv[1:]

def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result

try:
    with pathlib.Path(manifest_name).open("r", encoding="utf-8") as stream:
        document = json.load(stream, object_pairs_hook=unique_pairs)
except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
    raise SystemExit(f"installed manifest is invalid: {error}")
if not isinstance(document, dict):
    raise SystemExit("installed manifest root is not an object")
if document.get("schema") != expected_schema or document.get("product") != expected_product:
    raise SystemExit("installed manifest identity is invalid")
if document.get("target") != expected_target or document.get("compatibility") != expected_compatibility:
    raise SystemExit("installed manifest target is invalid")
version = document.get("version")
if not isinstance(version, str) or not re.fullmatch(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", version
):
    raise SystemExit("installed manifest version is invalid")
print(version)
PY
}

run_update() {
    command -v "$CURL_BIN" >/dev/null 2>&1 || die "$CURL_BIN is required"
    require_user_manager
    local current_version release_json selection update_state newest_version
    current_version="$(read_installed_version)"
    update_root="$(mktemp -d "${TMPDIR:-/tmp}/codex-info-update.XXXXXX")"
    # shellcheck disable=SC2329  # Invoked indirectly by the EXIT trap below.
    cleanup_update() {
        rm -r -- "$update_root"
    }
    trap cleanup_update EXIT
    release_json="$update_root/releases.json"
    selection="$update_root/selection"

    "$CURL_BIN" --fail --silent --show-error --proto '=https' --max-time 30 \
        --header 'Accept: application/vnd.github+json' \
        --header 'X-GitHub-Api-Version: 2022-11-28' \
        "$RELEASES_URL" >"$release_json" || die 'could not query the public stable release channel'
    python3 - "$release_json" "$current_version" "$REPOSITORY" "$TARGET" <<'PY' >"$selection"
import json
import pathlib
import re
import sys

release_name, current_text, repository, target = sys.argv[1:]
version_pattern = re.compile(
    r"windows-v((?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*))"
)
asset_names = (
    "CodexInfo.WindowsClient.Setup.exe",
    "CodexInfo.WindowsClient.update.json",
)

def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result

def reject(message):
    raise SystemExit(f"release metadata validation failed: {message}")

try:
    with pathlib.Path(release_name).open("r", encoding="utf-8") as stream:
        releases = json.load(stream, object_pairs_hook=unique_pairs)
except (OSError, UnicodeError, json.JSONDecodeError, ValueError) as error:
    reject(str(error))
if not isinstance(releases, list):
    reject("release response is not an array")
try:
    current = tuple(int(part) for part in current_text.split("."))
except ValueError:
    reject("installed version is invalid")
stable = []
for index, release in enumerate(releases):
    if not isinstance(release, dict):
        reject(f"release {index} is not an object")
    draft = release.get("draft")
    prerelease = release.get("prerelease")
    tag = release.get("tag_name")
    if type(draft) is not bool or type(prerelease) is not bool or not isinstance(tag, str):
        reject(f"release {index} has malformed identity")
    if draft or prerelease:
        continue
    match = version_pattern.fullmatch(tag)
    if match is None:
        reject(f"stable release {index} has malformed tag")
    version_text = match.group(1)
    version = tuple(int(part) for part in version_text.split("."))
    stable.append((version, version_text, release))

if not stable:
    print("no-update\t" + current_text)
    raise SystemExit(0)
stable.sort(key=lambda item: item[0], reverse=True)
if len(stable) > 1 and stable[0][0] == stable[1][0]:
    reject("duplicate stable release version")
newest, newest_text, release = stable[0]
if newest <= current:
    print("no-update\t" + newest_text)
    raise SystemExit(0)
published_at = release.get("published_at")
if not isinstance(published_at, str) or not published_at:
    reject("newest stable release is not published")

assets = release.get("assets")
if not isinstance(assets, list) or len(assets) != 5:
    reject("newest stable release must contain exactly five assets")
asset_by_name = {}
for index, asset in enumerate(assets):
    if not isinstance(asset, dict):
        reject(f"asset {index} is not an object")
    name = asset.get("name")
    url = asset.get("browser_download_url")
    state = asset.get("state")
    size = asset.get("size")
    digest = asset.get("digest")
    if (
        not isinstance(name, str) or not isinstance(url, str) or name in asset_by_name or
        state != "uploaded" or type(size) is not int or size <= 0 or
        not isinstance(digest, str) or not re.fullmatch(r"sha256:[0-9a-f]{64}", digest)
    ):
        reject(f"asset {index} has malformed identity")
    asset_by_name[name] = (url, size, digest)
archive_name = f"codex-info-{newest_text}-{target}.tar.gz"
linux_names = (archive_name, archive_name + ".sha256", archive_name[:-7] + ".manifest.json")
expected_names = set(asset_names + linux_names)
if set(asset_by_name) != expected_names:
    reject("newest stable release asset names are not the exact five required assets")
for name in expected_names:
    expected_url = (
        f"https://github.com/{repository}/releases/download/windows-v{newest_text}/{name}"
    )
    asset_url, asset_size, asset_digest = asset_by_name[name]
    if asset_url != expected_url:
        reject(f"asset URL is not canonical for {name}")
print("update\t" + newest_text)
for name in linux_names:
    asset_url, asset_size, asset_digest = asset_by_name[name]
    print(f"{name}\t{asset_url}\t{asset_size}\t{asset_digest}")
PY

    IFS=$'\t' read -r update_state newest_version < "$selection" ||
        die 'release selection did not complete'
    [[ "$update_state" == no-update || "$update_state" == update ]] ||
        die 'release selection returned an unknown state'
    if [[ "$update_state" == no-update ]]; then
        printf 'no update current=%s newest=%s\n' "$current_version" "$newest_version"
        return 0
    fi

    local archive_name checksum_name manifest_name archive_url checksum_url manifest_url
    local archive_size checksum_size manifest_size archive_digest checksum_digest manifest_digest
    IFS=$'\t' read -r archive_name archive_url archive_size archive_digest < <(sed -n '2p' "$selection")
    IFS=$'\t' read -r checksum_name checksum_url checksum_size checksum_digest < <(sed -n '3p' "$selection")
    IFS=$'\t' read -r manifest_name manifest_url manifest_size manifest_digest < <(sed -n '4p' "$selection")
    [[ "$archive_name" == codex-info-${newest_version}-${TARGET}.tar.gz ]] ||
        die 'release archive name is invalid'
    [[ "$checksum_name" == "$archive_name.sha256" ]] ||
        die 'release checksum name is invalid'
    [[ "$manifest_name" == "${archive_name%.tar.gz}.manifest.json" ]] ||
        die 'release manifest name is invalid'
    local archive_path="$update_root/$archive_name"
    local checksum_path="$update_root/$checksum_name"
    local manifest_path="$update_root/$manifest_name"
    download_update_asset "$archive_url" "$archive_path" "$archive_size" "$archive_digest"
    download_update_asset "$checksum_url" "$checksum_path" "$checksum_size" "$checksum_digest"
    download_update_asset "$manifest_url" "$manifest_path" "$manifest_size" "$manifest_digest"

    python3 - "$manifest_path" "$newest_version" "$SCHEMA" "$PRODUCT" <<'PY'
import json
import pathlib
import re
import sys

manifest_name, expected_version, expected_schema, expected_product = sys.argv[1:]
try:
    with pathlib.Path(manifest_name).open("r", encoding="utf-8") as stream:
        document = json.load(stream)
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise SystemExit(f"downloaded Linux manifest is invalid: {error}")
if (
    not isinstance(document, dict) or
    document.get("schema") != expected_schema or
    document.get("product") != expected_product or
    document.get("version") != expected_version or
    not isinstance(document.get("version"), str) or
    not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", document["version"])
):
    raise SystemExit("downloaded Linux manifest identity/version does not match release")
PY

    local install_status=0
    "$0" --bundle "$archive_path" --manifest "$manifest_path" --sha256 "$checksum_path" ||
        install_status=$?
    if ((install_status != 0)); then
        echo 'linux-bundle-install: unattended update installation failed' >&2
        return "$install_status"
    fi
    printf 'updated from=%s to=%s\n' "$current_version" "$newest_version"
}

download_update_asset() {
    local url="$1" destination="$2" expected_size="$3" expected_digest="$4" effective_url
    [[ "$url" == https://github.com/${REPOSITORY}/releases/download/*/* ]] ||
        die 'release asset URL is not canonical'
    effective_url="$("$CURL_BIN" --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' --max-redirs 3 \
        --max-time 300 --output "$destination" --write-out '%{url_effective}' "$url")" ||
        die "could not download release asset: $(basename -- "$destination")"
    case "$effective_url" in
        "$url"|https://release-assets.githubusercontent.com/*) ;;
        *) die 'release asset redirected outside the GitHub release asset boundary' ;;
    esac
    [[ -f "$destination" && ! -L "$destination" ]] ||
        die "downloaded release asset is not a regular file: $(basename -- "$destination")"
    [[ "$expected_size" =~ ^[1-9][0-9]*$ ]] || die 'release asset size is invalid'
    [[ "$expected_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || die 'release asset digest is invalid'
    [[ "$(stat -c %s -- "$destination")" == "$expected_size" ]] ||
        die "downloaded release asset size does not match: $(basename -- "$destination")"
    [[ "sha256:$(sha256sum -- "$destination" | awk '{print $1}')" == "$expected_digest" ]] ||
        die "downloaded release asset digest does not match: $(basename -- "$destination")"
}

if [[ "$ACTION" == remove ]]; then
    [[ -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] ||
        die '--remove cannot be combined with bundle options'
    remove_installation
    exit 0
fi

if [[ "$ACTION" == update ]]; then
    [[ -z "$ARCHIVE" && -z "$MANIFEST" && -z "$CHECKSUM" ]] ||
        die '--update cannot be combined with bundle options'
    run_update
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
# shellcheck disable=SC2329  # Invoked indirectly by the EXIT trap below.
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
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", manifest["version"]
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

for required_path in (
    "codex_info", "codex-info.service", "codex-info-update.service",
    "codex-info-update.timer", "install.sh", "LICENSE", "COPYRIGHT",
):
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
mkdir -p -- "$local_bin" "$local_libexec" "$share_dir" "$unit_dir"
[[ ! -d "$binary_destination" ]] || die "binary path is a directory: $binary_destination"
[[ ! -d "$unit_destination" ]] || die "unit path is a directory: $unit_destination"
[[ ! -d "$installer_destination" ]] ||
    die "installer path is a directory: $installer_destination"
[[ ! -d "$manifest_destination" ]] ||
    die "manifest path is a directory: $manifest_destination"
[[ ! -d "$update_service_destination" ]] ||
    die "unit path is a directory: $update_service_destination"
[[ ! -d "$update_timer_destination" ]] ||
    die "unit path is a directory: $update_timer_destination"

stage_root=''
binary_stage=''
unit_stage=''
installer_stage=''
manifest_stage=''
update_service_stage=''
update_timer_stage=''
binary_backup="$local_bin/.codex_info.previous.$$"
unit_backup="$unit_dir/.codex-info.previous.$$"
installer_backup="$local_libexec/.codex-info-installer.previous.$$"
manifest_backup="$share_dir/.codex-info-manifest.previous.$$"
update_service_backup="$unit_dir/.codex-info-update-service.previous.$$"
update_timer_backup="$unit_dir/.codex-info-update-timer.previous.$$"
had_unit=0
unit_was_enabled=0
unit_was_active=0
had_update_timer=0
update_timer_was_enabled=0
update_timer_was_active=0
binary_backed_up=0
unit_backed_up=0
installer_backed_up=0
manifest_backed_up=0
update_service_backed_up=0
update_timer_backed_up=0
binary_moved=0
unit_moved=0
installer_moved=0
manifest_moved=0
update_service_moved=0
update_timer_moved=0

cleanup_install_staging() {
    if [[ -n "$binary_stage" ]]; then
        rm -f -- "$binary_stage"
        binary_stage=''
    fi
    if [[ -n "$unit_stage" ]]; then
        rm -f -- "$unit_stage"
        unit_stage=''
    fi
    if [[ -n "$installer_stage" ]]; then
        rm -f -- "$installer_stage"
        installer_stage=''
    fi
    if [[ -n "$manifest_stage" ]]; then
        rm -f -- "$manifest_stage"
        manifest_stage=''
    fi
    if [[ -n "$update_service_stage" ]]; then
        rm -f -- "$update_service_stage"
        update_service_stage=''
    fi
    if [[ -n "$update_timer_stage" ]]; then
        rm -f -- "$update_timer_stage"
        update_timer_stage=''
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
installer_stage="$(mktemp "$local_libexec/.codex-info-linux-installer.XXXXXX")"
manifest_stage="$(mktemp "$share_dir/.codex-info-linux-manifest.XXXXXX")"
update_service_stage="$(mktemp "$unit_dir/.codex-info-linux-update-service.XXXXXX")"
update_timer_stage="$(mktemp "$unit_dir/.codex-info-linux-update-timer.XXXXXX")"

rollback_install() {
    local rollback_ok=1
    if ((update_timer_moved)); then
        rm -f -- "$update_timer_destination" || rollback_ok=0
    fi
    if ((update_service_moved)); then
        rm -f -- "$update_service_destination" || rollback_ok=0
    fi
    if ((manifest_moved)); then
        rm -f -- "$manifest_destination" || rollback_ok=0
    fi
    if ((installer_moved)); then
        rm -f -- "$installer_destination" || rollback_ok=0
    fi
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
    if ((installer_backed_up)); then
        mv -- "$installer_backup" "$installer_destination" || rollback_ok=0
    fi
    if ((manifest_backed_up)); then
        mv -- "$manifest_backup" "$manifest_destination" || rollback_ok=0
    fi
    if ((update_service_backed_up)); then
        mv -- "$update_service_backup" "$update_service_destination" || rollback_ok=0
    fi
    if ((update_timer_backed_up)); then
        mv -- "$update_timer_backup" "$update_timer_destination" || rollback_ok=0
    fi
    # Reload only after the previous unit files have been restored.  Do not
    # stop codex-info-update.service here: rollback can run inside that
    # oneshot service, and stopping itself can terminate or deadlock recovery.
    "$SYSTEMCTL_BIN" --user daemon-reload >/dev/null 2>&1 || rollback_ok=0
    if ((update_timer_moved || had_update_timer)); then
        if ((had_update_timer)); then
            if ((update_timer_was_enabled)); then
                "$SYSTEMCTL_BIN" --user enable codex-info-update.timer >/dev/null 2>&1 || rollback_ok=0
            else
                "$SYSTEMCTL_BIN" --user disable codex-info-update.timer >/dev/null 2>&1 || rollback_ok=0
            fi
            if ((update_timer_was_active)); then
                "$SYSTEMCTL_BIN" --user start codex-info-update.timer >/dev/null 2>&1 || rollback_ok=0
            else
                "$SYSTEMCTL_BIN" --user stop codex-info-update.timer >/dev/null 2>&1 || rollback_ok=0
            fi
        else
            "$SYSTEMCTL_BIN" --user disable --now codex-info-update.timer >/dev/null 2>&1 || rollback_ok=0
        fi
    fi
    # If the old unit was running, ask systemd to put that exact old unit back
    # into service.  A failing restart remains a failed installation, but it
    # must never turn into a successful report.
    if ((had_unit)); then
        if ((unit_was_enabled)); then
            "$SYSTEMCTL_BIN" --user enable codex-info.service >/dev/null 2>&1 || rollback_ok=0
        else
            "$SYSTEMCTL_BIN" --user disable codex-info.service >/dev/null 2>&1 || rollback_ok=0
        fi
        if ((unit_was_active)); then
            "$SYSTEMCTL_BIN" --user restart codex-info.service >/dev/null 2>&1 || rollback_ok=0
        else
            "$SYSTEMCTL_BIN" --user stop codex-info.service >/dev/null 2>&1 || rollback_ok=0
        fi
    else
        # A first install may have enabled and started a new unit before its
        # health check failed.  There is no previous unit to restart, so
        # disable and stop the failed generation before returning.
        "$SYSTEMCTL_BIN" --user disable --now codex-info.service >/dev/null 2>&1 || rollback_ok=0
    fi
    if ((rollback_ok)); then
        return 0
    fi
    echo 'linux-bundle-install: rollback could not be fully confirmed' >&2
    return 1
}

rollback_and_die() {
    local failure_message="$1"
    if ! rollback_install; then
        cleanup_install_staging
        die "$failure_message; manual recovery may be required and remaining backups were retained"
    fi
    cleanup_install_staging
    if ((binary_backed_up || unit_backed_up || installer_backed_up || manifest_backed_up)); then
        die "$failure_message; previous generation restored"
    fi
    die "$failure_message; failed generation removed"
}

install -m 0755 -- /dev/null "$binary_stage"
install -m 0644 -- /dev/null "$unit_stage"
install -m 0755 -- /dev/null "$installer_stage"
install -m 0644 -- /dev/null "$manifest_stage"
install -m 0644 -- /dev/null "$update_service_stage"
install -m 0644 -- /dev/null "$update_timer_stage"
tar --extract --gzip --file="$ARCHIVE" --directory="$stage_root" --no-same-owner
install -m 0755 -- "$stage_root/codex_info" "$binary_stage"
install -m 0644 -- "$stage_root/codex-info.service" "$unit_stage"
install -m 0755 -- "$stage_root/install.sh" "$installer_stage"
install -m 0644 -- "$MANIFEST" "$manifest_stage"
install -m 0644 -- "$stage_root/codex-info-update.service" "$update_service_stage"
install -m 0644 -- "$stage_root/codex-info-update.timer" "$update_timer_stage"

if [[ -e "$binary_destination" || -L "$binary_destination" ]]; then
    if ! mv -- "$binary_destination" "$binary_backup"; then
        cleanup_install_staging
        die 'could not stage existing binary for atomic update'
    fi
    binary_backed_up=1
fi
if [[ -e "$unit_destination" || -L "$unit_destination" ]]; then
    had_unit=1
    if "$SYSTEMCTL_BIN" --user is-enabled --quiet codex-info.service >/dev/null 2>&1; then
        unit_was_enabled=1
    fi
    if "$SYSTEMCTL_BIN" --user is-active --quiet codex-info.service >/dev/null 2>&1; then
        unit_was_active=1
    fi
    if ! mv -- "$unit_destination" "$unit_backup"; then
        rollback_and_die 'could not stage existing unit for atomic update'
    fi
    unit_backed_up=1
fi
if [[ -e "$installer_destination" || -L "$installer_destination" ]]; then
    if ! mv -- "$installer_destination" "$installer_backup"; then
        rollback_and_die 'could not stage existing installer for atomic update'
    fi
    installer_backed_up=1
fi
if [[ -e "$manifest_destination" || -L "$manifest_destination" ]]; then
    if ! mv -- "$manifest_destination" "$manifest_backup"; then
        rollback_and_die 'could not stage existing manifest for atomic update'
    fi
    manifest_backed_up=1
fi
if [[ -e "$update_service_destination" || -L "$update_service_destination" ]]; then
    if ! mv -- "$update_service_destination" "$update_service_backup"; then
        rollback_and_die 'could not stage existing update service for atomic update'
    fi
    update_service_backed_up=1
fi
if [[ -e "$update_timer_destination" || -L "$update_timer_destination" ]]; then
    had_update_timer=1
    if "$SYSTEMCTL_BIN" --user is-enabled --quiet codex-info-update.timer >/dev/null 2>&1; then
        update_timer_was_enabled=1
    fi
    if "$SYSTEMCTL_BIN" --user is-active --quiet codex-info-update.timer >/dev/null 2>&1; then
        update_timer_was_active=1
    fi
    if ! mv -- "$update_timer_destination" "$update_timer_backup"; then
        rollback_and_die 'could not stage existing update timer for atomic update'
    fi
    update_timer_backed_up=1
fi
if ! mv -- "$binary_stage" "$binary_destination"; then
    rollback_and_die 'could not atomically publish binary'
fi
binary_moved=1
if ! mv -- "$unit_stage" "$unit_destination"; then
    rollback_and_die 'could not atomically publish unit'
fi
unit_moved=1
if ! mv -- "$installer_stage" "$installer_destination"; then
    rollback_and_die 'could not atomically publish persistent installer'
fi
installer_moved=1
if ! mv -- "$manifest_stage" "$manifest_destination"; then
    rollback_and_die 'could not atomically publish installed manifest'
fi
manifest_moved=1
if ! mv -- "$update_service_stage" "$update_service_destination"; then
    rollback_and_die 'could not atomically publish update service'
fi
update_service_moved=1
if ! mv -- "$update_timer_stage" "$update_timer_destination"; then
    rollback_and_die 'could not atomically publish update timer'
fi
update_timer_moved=1

health_response=''
wait_for_health() {
    local attempt
    for ((attempt = 1; attempt <= HEALTH_READY_ATTEMPTS; attempt++)); do
        if health_response="$("$CURL_BIN" --fail --silent --max-time 1 "$HEALTH_URL")"; then
            return 0
        fi
        if ((attempt < HEALTH_READY_ATTEMPTS)); then
            sleep 1
        fi
    done
    return 1
}

if ! "$SYSTEMCTL_BIN" --user daemon-reload >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user enable codex-info.service >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user enable --now codex-info-update.timer >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user restart codex-info.service >/dev/null 2>&1 ||
    ! "$SYSTEMCTL_BIN" --user is-active --quiet codex-info.service >/dev/null 2>&1 ||
    ! wait_for_health;
then
    rollback_and_die 'systemd activation or health check failed'
fi
health_response_file="$stage_root/health-response.json"
if ! printf '%s' "$health_response" > "$health_response_file"; then
    rollback_and_die 'could not store the health response for validation'
fi
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
    rollback_and_die 'health response version does not match the installed bundle'
fi

if ! rm -f -- "$binary_backup" "$unit_backup" "$installer_backup" "$manifest_backup" \
    "$update_service_backup" "$update_timer_backup"; then
    echo 'linux-bundle-install: update committed; one or more recovery backups could not be removed' >&2
fi
cleanup_install_staging
if ! printf 'installed and healthy unit=%s binary=%s target=%s\n' \
    "$unit_destination" "$binary_destination" "$TARGET"; then
    : # A reporting sink failure cannot undo a health-confirmed commit.
fi
exit 0

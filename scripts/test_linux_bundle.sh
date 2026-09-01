#!/usr/bin/env bash
set -euo pipefail

# Finite local contract test.  The fake systemctl/curl commands model only the
# user-manager, enable/restart/active, health, and public Release observations
# needed by the bundle installer/updater; no host service, HOME, DB, session, or
# network is touched.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BUILD_SCRIPT="$SCRIPT_DIR/build_linux_bundle.sh"
ORIGINAL_PATH="$PATH"
TEST_ROOT="$(mktemp -d /tmp/codex-info-linux-bundle-test.XXXXXX)"
BUNDLE_DIR=""

while (($# > 0)); do
    case "$1" in
        --bundle-dir)
            (($# >= 2)) || { echo 'linux-bundle-test: --bundle-dir requires a path' >&2; exit 2; }
            BUNDLE_DIR="$2"
            shift 2
            ;;
        -h|--help)
            printf 'usage: test_linux_bundle.sh [--bundle-dir DIRECTORY]\n'
            exit 0
            ;;
        *)
            echo "linux-bundle-test: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

cleanup() {
    rm -r -- "$TEST_ROOT"
}
trap cleanup EXIT

fail() {
    echo "linux-bundle-test: $*" >&2
    exit 1
}

fake_bin="$TEST_ROOT/fake-bin"
fake_home="$TEST_ROOT/home"
fixture_root="$TEST_ROOT/fixture"
output_root="$TEST_ROOT/output"
log="$TEST_ROOT/commands.log"
release_json="$TEST_ROOT/release.json"
release_asset_root="$TEST_ROOT/release-assets"
update_tmp="$TEST_ROOT/update-tmp"
release_version=''
mkdir -p -- "$fake_bin" "$fake_home" "$fixture_root" "$output_root"
mkdir -p -- "$release_asset_root" "$update_tmp"

cat > "$fake_bin/systemctl" <<'FAKE_SYSTEMCTL'
#!/usr/bin/env bash
set -euo pipefail
printf 'systemctl %s\n' "$*" >> "$FAKE_LOG"
if [[ " $* " == *' enable --now codex-info-update.timer '* &&
      "${FAKE_SYSTEMCTL_MANAGER_UP_LONG:-0}" == 1 ]]; then
    timer_path="$HOME/.config/systemd/user/codex-info-update.timer"
    if [[ -f "$timer_path" ]] && grep -Fq -- 'OnBootSec=5min' "$timer_path"; then
        timer_status=0
        timer_output=''
        if timer_output="$(env -u CODEX_INFO_INSTALL_LOCKED \
            SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
            bash "$HOME/.local/libexec/codex-info-install.sh" --update 9>&- 2>&1)"; then
            timer_status=0
        else
            timer_status=$?
        fi
        printf 'timer-expired-immediately status=%s output=%s\n' \
            "$timer_status" "$timer_output" >> "$FAKE_LOG"
    fi
fi
if [[ " $* " == *' show-environment '* ]]; then
    exit "${FAKE_SYSTEMCTL_SHOW_ENVIRONMENT_STATUS:-0}"
fi
if [[ " $* " == *' show --property=LoadState --value '* ]]; then
    printf '%s\n' "${FAKE_SYSTEMCTL_LOAD_STATE:-loaded}"
    exit 0
fi
if [[ " $* " == *' daemon-reload '* && "${FAKE_SYSTEMCTL_FAIL_DAEMON_RELOAD:-0}" == 1 ]]; then
    exit 1
fi
if [[ " $* " == *' restart '* ]]; then
    count_file="$FAKE_LOG.restart-count"
    count=0
    if [[ -f "$count_file" ]]; then
        count="$(<"$count_file")"
    fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$count_file"
    if [[ "${FAKE_SYSTEMCTL_FAIL_RESTART_ONCE:-0}" == 1 && ! -e "$FAKE_LOG.restart-failed" ]]; then
        : > "$FAKE_LOG.restart-failed"
        exit 1
    fi
fi
if [[ " $* " == *' is-enabled '* ]]; then
    if [[ " $* " == *' codex-info-update.timer '* ]]; then
        if [[ "${FAKE_SYSTEMCTL_TIMER_ENABLED_PROBE_ERROR:-0}" == 1 ]]; then
            exit 2
        fi
        if [[ "${FAKE_SYSTEMCTL_TIMER_ENABLED:-1}" == 1 ]]; then
            printf 'enabled\n'
            exit 0
        fi
        printf 'disabled\n'
        exit 1
    else
        if [[ "${FAKE_SYSTEMCTL_MAIN_ENABLED_PROBE_ERROR:-0}" == 1 ]]; then
            exit 2
        fi
        if [[ "${FAKE_SYSTEMCTL_MAIN_ENABLED:-1}" == 1 ]]; then
            printf 'enabled\n'
            exit 0
        fi
        printf 'disabled\n'
        exit 1
    fi
fi
if [[ " $* " == *' is-active '* ]]; then
    if [[ " $* " == *' codex-info-update.timer '* ]]; then
        if [[ "${FAKE_SYSTEMCTL_TIMER_ACTIVE_PROBE_ERROR:-0}" == 1 ]]; then
            exit 2
        fi
        if [[ "${FAKE_SYSTEMCTL_TIMER_ACTIVE:-1}" == 1 ]]; then
            printf 'active\n'
            exit 0
        fi
        printf 'inactive\n'
        exit 3
    else
        if [[ "${FAKE_SYSTEMCTL_MAIN_ACTIVE_PROBE_ERROR:-0}" == 1 ]]; then
            exit 2
        fi
        if [[ "${FAKE_SYSTEMCTL_MAIN_ACTIVE:-${FAKE_SYSTEMCTL_ACTIVE:-1}}" == 1 ]]; then
            printf 'active\n'
            exit 0
        fi
        printf 'inactive\n'
        exit 3
    fi
fi
exit 0
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
        -o|--output)
            (($# >= 2)) || exit 2
            output="$2"
            shift 2
            ;;
        --output=*)
            output="${1#*=}"
            shift
            ;;
        --url)
            (($# >= 2)) || exit 2
            url="$2"
            shift 2
            ;;
        --url=*)
            url="${1#*=}"
            shift
            ;;
        -w|--write-out)
            (($# >= 2)) || exit 2
            write_out="$2"
            shift 2
            ;;
        -H|--header|-A|--user-agent|--max-time|--connect-timeout|--retry|--max-redirs|--proto|--proto-redir)
            (($# >= 2)) || exit 2
            shift 2
            ;;
        --)
            shift
            (($# > 0)) && url="$1"
            break
            ;;
        -*)
            shift
            ;;
        *)
            url="$1"
            shift
            ;;
    esac
done

payload=''
payload_file=''
if [[ "$url" == */releases/latest* ]]; then
    [[ -f "$FAKE_RELEASE_JSON" ]] || exit 1
    payload="$(python3 - "$FAKE_RELEASE_JSON" <<'PY'
import json
import pathlib
import sys

value = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if not isinstance(value, list) or not value:
    raise SystemExit(1)
print(json.dumps(value[0], separators=(",", ":")))
PY
)"
elif [[ "$url" == */releases* && "$url" != */download/* ]]; then
    [[ -f "$FAKE_RELEASE_JSON" ]] || exit 1
    payload="$(<"$FAKE_RELEASE_JSON")"
elif [[ "$url" == */download/* ]]; then
    asset_name="${url##*/}"
    asset_name="${asset_name%%\?*}"
    payload_file="$FAKE_RELEASE_ASSET_ROOT/$asset_name"
    [[ -f "$payload_file" ]] || exit 1
else
    health_count_file="$FAKE_LOG.health-count"
    health_count=0
    if [[ -f "$health_count_file" ]]; then
        health_count="$(<"$health_count_file")"
    fi
    health_count=$((health_count + 1))
    printf '%s\n' "$health_count" > "$health_count_file"
    if ((health_count <= ${FAKE_CURL_HEALTH_TRANSIENT_FAILURES:-0})); then
        exit 1
    fi
    payload="$(printf '{\"api_version\":\"v1\",\"service\":\"codex-info\",\"product_version\":\"%s\"}\n' "$FAKE_HEALTH_VERSION")"
fi

if [[ -n "${FAKE_CURL_FAIL:-}" && "${FAKE_CURL_FAIL}" == 1 ]] ||
   [[ -n "${FAKE_CURL_FAIL_RELEASE:-}" && "${FAKE_CURL_FAIL_RELEASE}" == 1 &&
      "$url" == */releases* ]] ||
   [[ -n "${FAKE_CURL_FAIL_DOWNLOAD:-}" && "${FAKE_CURL_FAIL_DOWNLOAD}" == 1 &&
      "$url" == */download/* ]] ||
   [[ -n "${FAKE_CURL_FAIL_HEALTH:-}" && "${FAKE_CURL_FAIL_HEALTH}" == 1 &&
      "$url" != */releases* && "$url" != */download/* ]]; then
    exit 1
fi
if [[ -n "$output" ]]; then
    if [[ -n "$payload_file" ]]; then
        cp -- "$payload_file" "$output"
    else
        printf '%s' "$payload" > "$output"
    fi
else
    if [[ -n "$payload_file" ]]; then
        cat -- "$payload_file"
    else
        printf '%s' "$payload"
    fi
fi
if [[ "$write_out" == '%{url_effective}' ]]; then
    printf '%s' "${FAKE_CURL_EFFECTIVE_URL:-$url}"
fi
exit 0
FAKE_CURL
chmod 0755 "$fake_bin/curl"

cat > "$fake_bin/sleep" <<'FAKE_SLEEP'
#!/usr/bin/env bash
set -euo pipefail
printf 'sleep %s\n' "$*" >> "$FAKE_LOG"
exit 0
FAKE_SLEEP
chmod 0755 "$fake_bin/sleep"

cat > "$fake_bin/objdump" <<'FAKE_OBJDUMP'
#!/usr/bin/env bash
set -euo pipefail
printf 'fake objdump GLIBC_2.31\n'
FAKE_OBJDUMP
chmod 0755 "$fake_bin/objdump"

write_fixture_binary() {
    local marker="$1"
    printf 'fixture binary %s\n' "$marker" > "$fixture_root/codex_info"
    chmod 0755 "$fixture_root/codex_info"
}

write_fixture_binary 'fixture binary generation one'

build_bundle() {
    local source_sha="$1" version="$2"
    SOURCE_SHA="$source_sha" RUN_ID=92001 RUN_ATTEMPT=1 OBJDUMP_BIN="$fake_bin/objdump" \
        bash "$BUILD_SCRIPT" --binary "$fixture_root/codex_info" \
        --version "$version" --output-dir "$output_root" >/dev/null
    printf '%s/%s-%s-%s.tar.gz\n' "$output_root" 'codex-info' "$version" \
        'x86_64-unknown-linux-gnu'
}

archive_version() {
    local archive="$1"
    basename -- "$archive" |
        sed 's/^codex-info-//; s/-x86_64-unknown-linux-gnu\.tar\.gz$//'
}

write_release_fixture() {
    local archive="$1" shape="${2:-complete}" version archive_name
    version="$(archive_version "$archive")"
    archive_name="$(basename -- "$archive")"
    release_version="$version"

    rm -f -- "$release_asset_root"/*
    if [[ "$shape" != windows-only ]]; then
        cp -- "$archive" "$release_asset_root/$archive_name"
        cp -- "$archive.sha256" "$release_asset_root/$archive_name.sha256"
        cp -- "${archive%.tar.gz}.manifest.json" \
            "$release_asset_root/${archive_name%.tar.gz}.manifest.json"
    fi
    if [[ "$shape" != linux-only ]]; then
        printf 'fixture Windows Setup %s\n' "$version" > \
            "$release_asset_root/CodexInfo.WindowsClient.Setup.exe"
        printf '{"schema_version":1,"version":"%s"}\n' "$version" > \
            "$release_asset_root/CodexInfo.WindowsClient.update.json"
    fi
    case "$shape" in
        bad-checksum)
            printf '%064d  %s\n' 0 "$archive_name" > \
                "$release_asset_root/$archive_name.sha256"
            ;;
        bad-manifest)
            printf '%s\n' '{not-json' > \
                "$release_asset_root/${archive_name%.tar.gz}.manifest.json"
            ;;
        bad-manifest-version)
            python3 - "$release_asset_root/${archive_name%.tar.gz}.manifest.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
document = json.loads(path.read_text(encoding="utf-8"))
document["version"] = "9.9.9"
path.write_text(json.dumps(document, separators=(",", ":")) + "\n", encoding="utf-8")
PY
            ;;
        bad-archive-content)
            tamper_root="$TEST_ROOT/tamper-$version"
            [[ ! -e "$tamper_root" ]] || rm -r -- "$tamper_root"
            mkdir -- "$tamper_root"
            mapfile -t archive_members < <(tar -tzf "$release_asset_root/$archive_name")
            tar -xzf "$release_asset_root/$archive_name" -C "$tamper_root"
            printf '%s\n' 'tampered after manifest generation' >> "$tamper_root/codex_info"
            tar -czf "$release_asset_root/$archive_name" -C "$tamper_root" -- "${archive_members[@]}"
            archive_hash="$(sha256sum -- "$release_asset_root/$archive_name" | awk '{print $1}')"
            printf '%s  %s\n' "$archive_hash" "$archive_name" > \
                "$release_asset_root/$archive_name.sha256"
            rm -r -- "$tamper_root"
            ;;
        extra)
            printf '%s\n' 'unexpected release asset' > \
                "$release_asset_root/unexpected.txt"
            ;;
        complete|linux-only|windows-only|draft|prerelease|unpublished|malformed-json|bad-tag|bad-url|bad-state|bad-size|bad-digest)
            ;;
        *)
            fail "unknown Release fixture shape: $shape"
            ;;
    esac

    python3 - "$release_asset_root" "$version" "$shape" "$release_json" <<'PY'
import hashlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
version = sys.argv[2]
shape = sys.argv[3]
output = pathlib.Path(sys.argv[4])
linux_names = [
    f"codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz",
    f"codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz.sha256",
    f"codex-info-{version}-x86_64-unknown-linux-gnu.manifest.json",
]
windows_names = [
    "CodexInfo.WindowsClient.Setup.exe",
    "CodexInfo.WindowsClient.update.json",
]
if shape == "linux-only":
    names = linux_names
elif shape == "windows-only":
    names = windows_names
else:
    names = windows_names + linux_names
    if shape == "extra":
        names.append("unexpected.txt")
assets = []
for index, name in enumerate(names, start=1):
    path = root / name
    if not path.is_file():
        raise SystemExit(f"missing fixture asset: {path}")
    data = path.read_bytes()
    assets.append({
        "id": index,
        "name": name,
        "label": "",
        "state": "uploaded",
        "size": len(data),
        "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
        "content_type": "application/octet-stream",
        "browser_download_url": (
            "https://github.com/salty919/codex_info_v2/releases/download/"
            f"windows-v{version}/{name}"
        ),
    })
release = {
    "id": 92001,
    "tag_name": f"windows-v{version}",
    "name": f"Codex Info Monitor {version}",
    "draft": False,
    "prerelease": False,
    "published_at": "2026-09-01T00:00:00Z",
    "assets": assets,
}
if shape == "malformed-json":
    output.write_text("{not-json\n", encoding="utf-8")
    raise SystemExit(0)
if shape == "draft":
    release["draft"] = True
elif shape == "prerelease":
    release["prerelease"] = True
elif shape == "unpublished":
    release["published_at"] = None
elif shape == "bad-tag":
    release["tag_name"] = f"v{version}"
elif shape in {"bad-url", "bad-state", "bad-size", "bad-digest"}:
    linux_archive = next(asset for asset in assets if asset["name"].endswith(".tar.gz"))
    if shape == "bad-url":
        linux_archive["browser_download_url"] = f"https://example.invalid/{linux_archive['name']}"
    elif shape == "bad-state":
        linux_archive["state"] = "not-uploaded-UPDATE_SECRET_SENTINEL"
    elif shape == "bad-size":
        linux_archive["size"] += 1
    else:
        linux_archive["digest"] = "sha256:" + "0" * 64
output.write_text(json.dumps([release], separators=(",", ":")) + "\n", encoding="utf-8")
PY
}

extract_install_script() {
    local archive="$1"
    local version script_dir install_script
    version="$(basename -- "$archive")"
    version="$(printf '%s' "$version" |
        sed 's/^codex-info-//; s/-x86_64-unknown-linux-gnu\.tar\.gz$//')"
    script_dir="$TEST_ROOT/install-${version}"
    mkdir -p -- "$script_dir"
    install_script="$script_dir/install.sh"
    tar -xOf "$archive" install.sh > "$install_script"
    chmod 0755 "$install_script"
    printf '%s\n' "$install_script"
}

run_install() {
    local archive="$1"
    local version health_version='' install_script
    if (( $# >= 2 )); then
        health_version="$2"
    fi
    version="$(basename -- "$archive")"
    version="$(printf '%s' "$version" |
        sed 's/^codex-info-//; s/-x86_64-unknown-linux-gnu\.tar\.gz$//')"
    if [[ -z "$health_version" ]]; then
        health_version="$version"
    fi
    install_script="$(extract_install_script "$archive")"
    HOME="$fake_home" PATH="$fake_bin:$ORIGINAL_PATH" \
        FAKE_LOG="$log" FAKE_HEALTH_VERSION="$health_version" \
        FAKE_CURL_HEALTH_TRANSIENT_FAILURES="${FAKE_CURL_HEALTH_TRANSIENT_FAILURES:-0}" \
        FAKE_SYSTEMCTL_MANAGER_UP_LONG="${FAKE_SYSTEMCTL_MANAGER_UP_LONG:-0}" \
        FAKE_SYSTEMCTL_MAIN_ENABLED_PROBE_ERROR="${FAKE_SYSTEMCTL_MAIN_ENABLED_PROBE_ERROR:-0}" \
        FAKE_SYSTEMCTL_MAIN_ACTIVE_PROBE_ERROR="${FAKE_SYSTEMCTL_MAIN_ACTIVE_PROBE_ERROR:-0}" \
        FAKE_SYSTEMCTL_TIMER_ENABLED_PROBE_ERROR="${FAKE_SYSTEMCTL_TIMER_ENABLED_PROBE_ERROR:-0}" \
        FAKE_SYSTEMCTL_TIMER_ACTIVE_PROBE_ERROR="${FAKE_SYSTEMCTL_TIMER_ACTIVE_PROBE_ERROR:-0}" \
        SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$install_script" --bundle "$archive"
}

run_update() {
    local _installed_from_archive="$1" health_version='' install_script
    if (( $# >= 2 )); then
        health_version="$2"
    fi
    if [[ -z "$health_version" ]]; then
        health_version="$release_version"
    fi
    install_script="$fake_home/.local/libexec/codex-info-install.sh"
    [[ -x "$install_script" ]] || fail 'persistent installer is missing or not executable'
    HOME="$fake_home" TMPDIR="$update_tmp" PATH="$fake_bin:$ORIGINAL_PATH" \
        FAKE_LOG="$log" FAKE_HEALTH_VERSION="$health_version" \
        FAKE_CURL_HEALTH_TRANSIENT_FAILURES="${FAKE_CURL_HEALTH_TRANSIENT_FAILURES:-0}" \
        FAKE_RELEASE_JSON="$release_json" FAKE_RELEASE_ASSET_ROOT="$release_asset_root" \
        FAKE_CURL_EFFECTIVE_URL="${FAKE_CURL_EFFECTIVE_URL:-}" \
        FAKE_SYSTEMCTL_MAIN_ENABLED_PROBE_ERROR="${FAKE_SYSTEMCTL_MAIN_ENABLED_PROBE_ERROR:-0}" \
        FAKE_SYSTEMCTL_MAIN_ACTIVE_PROBE_ERROR="${FAKE_SYSTEMCTL_MAIN_ACTIVE_PROBE_ERROR:-0}" \
        FAKE_SYSTEMCTL_TIMER_ENABLED_PROBE_ERROR="${FAKE_SYSTEMCTL_TIMER_ENABLED_PROBE_ERROR:-0}" \
        FAKE_SYSTEMCTL_TIMER_ACTIVE_PROBE_ERROR="${FAKE_SYSTEMCTL_TIMER_ACTIVE_PROBE_ERROR:-0}" \
        CODEX_INFO_INSTALL_LOCKED="${CODEX_INFO_INSTALL_LOCKED:-}" \
        SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$install_script" --update
}

run_remove() {
    local _installed_from_archive="$1" install_script
    install_script="$fake_home/.local/libexec/codex-info-install.sh"
    [[ -x "$install_script" ]] || fail 'persistent installer is missing or not executable'
    HOME="$fake_home" PATH="$fake_bin:$ORIGINAL_PATH" \
        FAKE_LOG="$log" SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$install_script" --remove
}

if [[ -n "$BUNDLE_DIR" ]]; then
    [[ -d "$BUNDLE_DIR" ]] || fail "bundle directory is missing: $BUNDLE_DIR"
    mapfile -t supplied_archives < <(find "$BUNDLE_DIR" -maxdepth 1 -type f \
        -name 'codex-info-*-x86_64-unknown-linux-gnu.tar.gz' -print | LC_ALL=C sort)
    [[ "${#supplied_archives[@]}" -eq 1 ]] || fail 'bundle directory must contain exactly one Linux archive'
    supplied_archive="${supplied_archives[0]}"
    supplied_manifest="${supplied_archive%.tar.gz}.manifest.json"
    [[ -f "$supplied_archive.sha256" && -f "$supplied_manifest" ]] ||
        fail 'bundle directory is missing external checksum or manifest'
    mapfile -t supplied_paths < <(tar -tzf "$supplied_archive")
    for required_path in codex_info codex-info.service codex-info-update.service \
        codex-info-update.timer install.sh LICENSE COPYRIGHT THIRD_PARTY_NOTICES.md \
        manifest.json SHA256SUMS; do
        printf '%s\n' "${supplied_paths[@]}" | grep -Fxq -- "$required_path" ||
            fail "supplied archive is missing $required_path"
    done
    [[ "$(printf '%s\n' "${supplied_paths[@]}" | grep -Fxc -- install.sh)" -eq 1 ]] ||
        fail 'supplied install.sh inventory is invalid'
    printf 'case supplied-bundle-inventory: PASS\n'
fi

if SOURCE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa RUN_ID=92001 \
    OBJDUMP_BIN="$fake_bin/objdump" bash "$BUILD_SCRIPT" \
    --binary "$fixture_root/codex_info" --version 1.0.19 --output-dir "$output_root" \
    --target aarch64-unknown-linux-gnu >/dev/null 2>&1; then
    fail 'unsupported target was accepted'
fi
printf 'case target-rejection: PASS\n'
if SOURCE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa RUN_ID=92001 \
    OBJDUMP_BIN="$fake_bin/objdump" bash "$BUILD_SCRIPT" \
    --binary "$fixture_root/codex_info" --version 1.0.19 --output-dir "$output_root" \
    --compatibility musl >/dev/null 2>&1; then
    fail 'unsupported compatibility baseline was accepted'
fi
printf 'case compatibility-rejection: PASS\n'
if SOURCE_SHA=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa RUN_ID=92001 \
    OBJDUMP_BIN="$fake_bin/objdump" bash "$BUILD_SCRIPT" \
    --binary "$fixture_root/codex_info" --version 01.0.19 --output-dir "$output_root" \
    >/dev/null 2>&1; then
    fail 'noncanonical product version was accepted'
fi
printf 'case noncanonical-version-rejection: PASS\n'

assert_binary_marker() {
    local path="$1" expected="$2"
    [[ -f "$path" ]] || fail "missing file: $path"
    grep -aFq -- "$expected" "$path" || fail "unexpected binary contents: $path"
}

installation_state_digest() {
    local relative path
    for relative in \
        .local/bin/codex_info \
        .config/systemd/user/codex-info.service \
        .config/systemd/user/codex-info-update.service \
        .config/systemd/user/codex-info-update.timer \
        .local/libexec/codex-info-install.sh \
        .local/share/codex-info/manifest.json \
        .codex/usage_history.sqlite3 \
        .codex/usage_history.sqlite3.bak.1 \
        .codex/usage_reset_hint.json \
        .codex/session.jsonl \
        .config/codex-info/settings.json; do
        path="$fake_home/$relative"
        [[ -f "$path" && ! -L "$path" ]] || fail "state sentinel is missing: $path"
        printf '%s\t%s\t%s\t%s\n' "$relative" "$(stat -c %a -- "$path")" \
            "$(stat -c %s -- "$path")" "$(sha256sum -- "$path" | awk '{print $1}')"
    done | sha256sum | awk '{print $1}'
}

profile_state_digest() {
    local relative path
    for relative in \
        .codex/usage_history.sqlite3 \
        .codex/usage_history.sqlite3.bak.1 \
        .codex/usage_reset_hint.json \
        .codex/session.jsonl \
        .config/codex-info/settings.json; do
        path="$fake_home/$relative"
        [[ -f "$path" && ! -L "$path" ]] || fail "profile sentinel is missing: $path"
        printf '%s\t%s\t%s\t%s\n' "$relative" "$(stat -c %a -- "$path")" \
            "$(stat -c %s -- "$path")" "$(sha256sum -- "$path" | awk '{print $1}')"
    done | sha256sum | awk '{print $1}'
}

assert_state_unchanged() {
    local expected="$1" label="$2"
    [[ "$(installation_state_digest)" == "$expected" ]] ||
        fail "$label changed installed or profile state"
}

assert_update_tmp_empty() {
    local label="$1"
    [[ -z "$(find "$update_tmp" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
        fail "$label left temporary update files"
}

assert_no_restart_since() {
    local first_line="$1" label="$2"
    if tail -n "+$first_line" "$log" | grep -Fq -- 'restart codex-info.service'; then
        fail "$label reached service restart"
    fi
}

assert_no_runtime_mutation_since() {
    local first_line="$1" label="$2" segment
    segment="$(tail -n "+$first_line" "$log")"
    ! grep -Eq -- 'systemctl --user (daemon-reload|enable|disable|start|stop|restart|reset-failed)( |$)' \
        <<<"$segment" || fail "$label reached runtime state mutation"
}

assert_rollback_did_not_stop_updater() {
    local first_line="$1" label="$2" segment reload_line timer_line
    segment="$(tail -n "+$first_line" "$log")"
    ! grep -Fq -- 'stop codex-info-update.service' <<<"$segment" ||
        fail "$label stopped its own update service during rollback"
    reload_line="$(grep -nF -- 'daemon-reload' <<<"$segment" | tail -n 1 | cut -d: -f1)"
    timer_line="$(grep -nE -- 'systemctl --user (enable|disable|start|stop)( --now)? codex-info-update.timer' \
        <<<"$segment" | tail -n 1 | cut -d: -f1)"
    [[ -n "$reload_line" && -n "$timer_line" && "$reload_line" -lt "$timer_line" ]] ||
        fail "$label did not reload restored units before restoring the timer"
}

assert_runtime_probe_error_case() {
    local label="$1" before_state before_update_log_lines
    before_state="$(installation_state_digest)"
    before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
    case "$label" in
        main-enabled)
            if FAKE_SYSTEMCTL_MAIN_ENABLED_PROBE_ERROR=1 run_update "$archive_v1" >/dev/null 2>&1; then
                fail 'main enabled probe error unexpectedly succeeded'
            fi
            ;;
        main-active)
            if FAKE_SYSTEMCTL_MAIN_ACTIVE_PROBE_ERROR=1 run_update "$archive_v1" >/dev/null 2>&1; then
                fail 'main active probe error unexpectedly succeeded'
            fi
            ;;
        timer-enabled)
            if FAKE_SYSTEMCTL_TIMER_ENABLED_PROBE_ERROR=1 run_update "$archive_v1" >/dev/null 2>&1; then
                fail 'timer enabled probe error unexpectedly succeeded'
            fi
            ;;
        timer-active)
            if FAKE_SYSTEMCTL_TIMER_ACTIVE_PROBE_ERROR=1 run_update "$archive_v1" >/dev/null 2>&1; then
                fail 'timer active probe error unexpectedly succeeded'
            fi
            ;;
        *)
            fail "unknown runtime probe error case: $label"
            ;;
    esac
    assert_state_unchanged "$before_state" "$label probe error"
    assert_no_runtime_mutation_since "$before_update_log_lines" "$label probe error"
    assert_update_tmp_empty "$label probe error"
    printf 'case runtime-%s-probe-error: PASS\n' "$label"
}

archive_v1="$(build_bundle 1111111111111111111111111111111111111111 1.0.19)"
bundle_install_script="$(extract_install_script "$archive_v1")"
bundle_help="$(bash "$bundle_install_script" --help)"
grep -Fq -- 'usage: install.sh --bundle' <<<"$bundle_help" ||
    fail 'bundled help does not name the install.sh payload'
grep -Fq -- 'install.sh --remove' <<<"$bundle_help" ||
    fail 'bundled help does not name the remove command'
grep -Fq -- 'install.sh --update' <<<"$bundle_help" ||
    fail 'bundled help does not name the update command'
printf 'case bundled-help: PASS\n'
FAKE_CURL_HEALTH_TRANSIENT_FAILURES=1 FAKE_SYSTEMCTL_MANAGER_UP_LONG=1 \
    run_install "$archive_v1" >/dev/null
assert_binary_marker "$fake_home/.local/bin/codex_info" 'fixture binary generation one'
[[ -f "$fake_home/.config/systemd/user/codex-info.service" ]] ||
    fail 'normal install did not publish unit'
[[ -f "$fake_home/.config/systemd/user/codex-info-update.service" ]] ||
    fail 'normal install did not publish update unit'
[[ -f "$fake_home/.config/systemd/user/codex-info-update.timer" ]] ||
    fail 'normal install did not publish update timer'
grep -Fq -- 'systemctl --user enable codex-info.service' "$log" ||
    fail 'normal install did not enable user unit'
grep -Eq -- 'systemctl --user enable( --now)? codex-info-update.timer($| )' "$log" ||
    fail 'normal install did not enable update timer'
grep -Fq -- 'systemctl --user is-active --quiet codex-info.service' "$log" ||
    fail 'normal install did not check active state'
[[ "$(grep -Fc -- 'curl --fail --silent --max-time 1 http://127.0.0.1:8787/v1/health' "$log")" -eq 2 ]] ||
    fail 'normal install did not retry delayed health readiness exactly once'
grep -Fq -- 'sleep 1' "$log" ||
    fail 'normal install did not wait between health readiness attempts'
printf 'case delayed-health-readiness: PASS\n'
! grep -Fq -- 'timer-expired-immediately' "$log" ||
    fail 'update timer fired inside the install transaction on a long-running user manager'
printf 'case delayed-timer-first-fire: PASS\n'
grep -Fq -- 'curl --fail --silent --max-time 1 http://127.0.0.1:8787/v1/health' "$log" ||
    fail 'normal install did not check health'
grep -Fq -- 'ExecStart=%h/.local/libexec/codex-info-install.sh --update' \
    "$fake_home/.config/systemd/user/codex-info-update.service" ||
    fail 'update service does not invoke the persistent installer'
grep -Fq -- 'TimeoutStartSec=20min' "$fake_home/.config/systemd/user/codex-info-update.service" ||
    fail 'update service timeout is shorter than the bounded download path'
grep -Fq -- 'OnActiveSec=5min' "$fake_home/.config/systemd/user/codex-info-update.timer" ||
    fail 'update timer does not check after activation'
! grep -Fq -- 'OnBootSec=' "$fake_home/.config/systemd/user/codex-info-update.timer" ||
    fail 'update timer can fire immediately when installed after its boot-relative deadline'
grep -Fq -- 'OnUnitActiveSec=1d' "$fake_home/.config/systemd/user/codex-info-update.timer" ||
    fail 'update timer does not check daily'
grep -Fq -- 'Unit=codex-info-update.service' "$fake_home/.config/systemd/user/codex-info-update.timer" ||
    fail 'update timer does not target the update service'

mkdir -p -- "$fake_home/.codex" "$fake_home/.config/codex-info" \
    "$fake_home/.local/share/codex-info" "$fake_home/.cache/codex-info"
printf '%s\n' 'database' > "$fake_home/.codex/usage_history.sqlite3"
printf '%s\n' 'backup' > "$fake_home/.codex/usage_history.sqlite3.bak.1"
printf '%s\n' 'reset hint' > "$fake_home/.codex/usage_reset_hint.json"
printf '%s\n' 'session' > "$fake_home/.codex/session.jsonl"
printf '%s\n' 'config' > "$fake_home/.config/codex-info/settings.json"
rm -r -- "$TEST_ROOT/install-1.0.19"
printf 'case normal: PASS\n'

write_release_fixture "$archive_v1" complete
before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
run_update "$archive_v1" >/dev/null
assert_state_unchanged "$before_state" 'equal-version no-update'
assert_no_restart_since "$before_update_log_lines" 'equal-version no-update'
if tail -n "+$before_update_log_lines" "$log" | grep -Fq -- '/download/'; then
    fail 'no-update downloaded release assets'
fi
assert_update_tmp_empty 'equal-version no-update'
printf 'case no-update: PASS\n'

exec {held_lock_fd}>"$fake_home/.local/share/codex-info/.install.lock"
flock --exclusive --nonblock "$held_lock_fd" || fail 'could not hold installer concurrency lock'
before_state="$(installation_state_digest)"
if concurrent_output="$(run_update "$archive_v1" 2>&1)"; then
    fail 'concurrent updater unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'concurrent updater rejection'
assert_update_tmp_empty 'concurrent updater rejection'
grep -Fq -- 'another install, update, or remove is already running' <<<"$concurrent_output" ||
    fail 'concurrent updater did not report lock contention'
flock --unlock "$held_lock_fd"
exec {held_lock_fd}>&-
printf 'case concurrent-operation-rejection: PASS\n'

before_state="$(installation_state_digest)"
if spoofed_lock_output="$(CODEX_INFO_INSTALL_LOCKED=1 run_update "$archive_v1" 2>&1 \
    9>"$TEST_ROOT/not-the-install-lock")"; then
    fail 'spoofed inherited updater lock unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'spoofed inherited updater lock rejection'
grep -Fq -- 'inherited installer lock does not match' <<<"$spoofed_lock_output" ||
    fail 'spoofed inherited updater lock did not report its identity mismatch'
printf 'case spoofed-inherited-lock-rejection: PASS\n'

write_fixture_binary 'fixture binary generation two'
archive_v2="$(build_bundle 2222222222222222222222222222222222222222 1.0.20)"
write_release_fixture "$archive_v2" complete
for probe_case in main-enabled main-active timer-enabled timer-active; do
    assert_runtime_probe_error_case "$probe_case"
done
before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
if health_failure_output="$(run_update "$archive_v1" 9.9.9 2>&1)"; then
    fail 'updater health failure unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'updater health failure rollback'
assert_update_tmp_empty 'updater health failure rollback'
assert_rollback_did_not_stop_updater "$before_update_log_lines" 'updater health failure rollback'
[[ "$(tail -n "+$before_update_log_lines" "$log" | grep -Fc -- 'restart codex-info.service')" -eq 2 ]] ||
    fail 'updater health failure did not perform one switch and one rollback restart'
grep -Fq -- 'previous generation restored' <<<"$health_failure_output" ||
    fail 'confirmed updater rollback did not report the restored generation'
printf 'case updater-health-failure: PASS\n'

before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
if FAKE_SYSTEMCTL_MAIN_ENABLED=0 FAKE_SYSTEMCTL_MAIN_ACTIVE=0 \
    FAKE_SYSTEMCTL_TIMER_ENABLED=0 FAKE_SYSTEMCTL_TIMER_ACTIVE=0 \
    run_update "$archive_v1" >/dev/null 2>&1; then
    fail 'inactive-state updater failure unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'inactive-state updater rollback'
assert_update_tmp_empty 'inactive-state updater rollback'
rollback_segment="$(tail -n "+$before_update_log_lines" "$log")"
rollback_reload_line="$(grep -nF -- 'daemon-reload' <<<"$rollback_segment" | tail -n 1 | cut -d: -f1)"
rollback_tail="$(tail -n "+$rollback_reload_line" <<<"$rollback_segment")"
for expected_command in \
    'disable codex-info-update.timer' 'stop codex-info-update.timer' \
    'disable codex-info.service' 'stop codex-info.service'; do
    grep -Fq -- "$expected_command" <<<"$rollback_tail" ||
        fail "inactive-state updater rollback missed: $expected_command"
done
! grep -Fq -- 'restart codex-info.service' <<<"$rollback_tail" ||
    fail 'inactive-state updater rollback restarted the prior stopped service'
! grep -Fq -- 'start codex-info-update.timer' <<<"$rollback_tail" ||
    fail 'inactive-state updater rollback started the prior stopped timer'
printf 'case updater-runtime-state-rollback: PASS\n'

before_profile_state="$(profile_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
rm -f -- "$log.health-count"
FAKE_CURL_HEALTH_TRANSIENT_FAILURES=1 run_update "$archive_v1" >/dev/null
assert_binary_marker "$fake_home/.local/bin/codex_info" 'fixture binary generation two'
[[ "$(tail -n "+$before_update_log_lines" "$log" | grep -Fc -- 'restart codex-info.service')" -eq 1 ]] ||
    fail 'successful update did not perform exactly one service restart'
[[ "$(tail -n "+$before_update_log_lines" "$log" |
    grep -Fc -- 'curl --fail --silent --max-time 1 http://127.0.0.1:8787/v1/health')" -eq 2 ]] ||
    fail 'successful update did not retry delayed health readiness exactly once'
[[ "$(profile_state_digest)" == "$before_profile_state" ]] ||
    fail 'successful delayed-health update changed profile data'
assert_update_tmp_empty 'complete five-asset update'
printf 'case delayed-update-health-readiness: PASS\n'
printf 'case complete-five-asset-update: PASS\n'

write_release_fixture "$archive_v1" complete
before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
run_update "$archive_v2" >/dev/null
assert_state_unchanged "$before_state" 'older-version no-update'
assert_no_restart_since "$before_update_log_lines" 'older-version no-update'
if tail -n "+$before_update_log_lines" "$log" | grep -Fq -- '/download/'; then
    fail 'older-version no-update downloaded release assets'
fi
assert_update_tmp_empty 'older-version no-update'
printf 'case older-version-no-update: PASS\n'

write_fixture_binary 'fixture binary generation three'
archive_v3="$(build_bundle 3333333333333333333333333333333333333333 1.0.21)"
for release_shape in draft prerelease; do
    write_release_fixture "$archive_v3" "$release_shape"
    before_state="$(installation_state_digest)"
    before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
    run_update "$archive_v2" >/dev/null
    assert_state_unchanged "$before_state" "$release_shape Release ignore"
    assert_no_restart_since "$before_update_log_lines" "$release_shape Release ignore"
    if tail -n "+$before_update_log_lines" "$log" | grep -Fq -- '/download/'; then
        fail "$release_shape Release downloaded assets"
    fi
    assert_update_tmp_empty "$release_shape Release ignore"
    printf 'case release-%s-ignore: PASS\n' "$release_shape"
done

for release_shape in \
    linux-only windows-only extra unpublished malformed-json bad-tag \
    bad-url bad-state bad-size bad-digest bad-checksum bad-manifest \
    bad-manifest-version bad-archive-content; do
    write_release_fixture "$archive_v3" "$release_shape"
    before_state="$(installation_state_digest)"
    before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
    if rejection_output="$(run_update "$archive_v2" 2>&1)"; then
        fail "$release_shape Release unexpectedly succeeded"
    fi
    assert_state_unchanged "$before_state" "$release_shape Release rejection"
    assert_no_restart_since "$before_update_log_lines" "$release_shape Release rejection"
    assert_update_tmp_empty "$release_shape Release rejection"
    [[ "$rejection_output" != *UPDATE_SECRET_SENTINEL* ]] ||
        fail "$release_shape Release exposed raw metadata in updater output"
    printf 'case release-%s-rejection: PASS\n' "$release_shape"
done

write_release_fixture "$archive_v3" complete
before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
if FAKE_CURL_EFFECTIVE_URL='https://example.invalid/redirected-asset' \
    run_update "$archive_v2" >/dev/null 2>&1; then
    fail 'off-boundary release redirect unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'off-boundary release redirect rejection'
assert_no_restart_since "$before_update_log_lines" 'off-boundary release redirect rejection'
assert_update_tmp_empty 'off-boundary release redirect rejection'
printf 'case release-redirect-boundary-rejection: PASS\n'

write_release_fixture "$archive_v3" complete
rm -f -- "$log.restart-failed"
before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
if FAKE_SYSTEMCTL_FAIL_RESTART_ONCE=1 run_update "$archive_v2" >/dev/null 2>&1; then
    fail 'updater restart failure unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'updater restart failure rollback'
assert_update_tmp_empty 'updater restart failure rollback'
assert_rollback_did_not_stop_updater "$before_update_log_lines" 'updater restart failure rollback'
[[ "$(tail -n "+$before_update_log_lines" "$log" | grep -Fc -- 'restart codex-info.service')" -eq 2 ]] ||
    fail 'updater restart failure did not perform one failed switch and one rollback restart'
printf 'case updater-installer-failure: PASS\n'

before_state="$(installation_state_digest)"
before_update_log_lines="$(( $(wc -l < "$log") + 1 ))"
if rollback_output="$(FAKE_SYSTEMCTL_FAIL_DAEMON_RELOAD=1 run_update "$archive_v2" 2>&1)"; then
    fail 'unconfirmed updater rollback unexpectedly succeeded'
fi
assert_state_unchanged "$before_state" 'unconfirmed updater rollback'
assert_update_tmp_empty 'unconfirmed updater rollback'
assert_rollback_did_not_stop_updater "$before_update_log_lines" 'unconfirmed updater rollback'
grep -Fq -- 'manual recovery may be required' <<<"$rollback_output" ||
    fail 'unconfirmed updater rollback did not report manual recovery'
[[ "$rollback_output" != *'previous generation restored'* ]] ||
    fail 'unconfirmed updater rollback incorrectly reported a restored generation'
printf 'case updater-rollback-confirmation-failure: PASS\n'

if FAKE_SYSTEMCTL_FAIL_DAEMON_RELOAD=1 run_remove "$archive_v3" >/dev/null 2>&1; then
    fail 'remove daemon-reload failure unexpectedly succeeded'
fi
for retained in \
    "$fake_home/.codex/usage_history.sqlite3" \
    "$fake_home/.codex/usage_history.sqlite3.bak.1" \
    "$fake_home/.codex/usage_reset_hint.json" \
    "$fake_home/.codex/session.jsonl" \
    "$fake_home/.config/codex-info/settings.json"; do
    [[ -f "$retained" ]] || fail "failed remove deleted retained data: $retained"
done
printf 'case remove-failure-propagation: PASS\n'
before_remove_log_lines="$(( $(wc -l < "$log") + 1 ))"
run_remove "$archive_v3" >/dev/null
for stale_command in \
    'disable --now codex-info-update.timer' 'stop codex-info-update.service' \
    'disable --now codex-info.service'; do
    tail -n "+$before_remove_log_lines" "$log" | grep -Fq -- "$stale_command" ||
        fail "remove did not clear loaded unit with a missing file: $stale_command"
done
for removed_unit in codex-info.service codex-info-update.service codex-info-update.timer; do
    [[ ! -e "$fake_home/.config/systemd/user/$removed_unit" ]] ||
        fail "remove retained unit: $removed_unit"
    if [[ "$removed_unit" == codex-info-update.service ]]; then
        grep -Eq -- "systemctl --user (stop|disable( --now)?) $removed_unit($| )" "$log" ||
            fail "remove did not stop/disable unit: $removed_unit"
    else
        grep -Eq -- "systemctl --user disable( --now)? $removed_unit($| )" "$log" ||
            fail "remove did not disable unit: $removed_unit"
    fi
done
FAKE_SYSTEMCTL_LOAD_STATE=not-found run_remove "$archive_v3" >/dev/null
[[ -f "$fake_home/.local/bin/codex_info" ]] || fail 'remove deleted installed binary'
[[ -f "$fake_home/.local/libexec/codex-info-install.sh" ]] ||
    fail 'remove deleted persistent installer'
[[ -f "$fake_home/.local/share/codex-info/manifest.json" ]] ||
    fail 'remove deleted installed manifest'
for retained in \
    "$fake_home/.codex/usage_history.sqlite3" \
    "$fake_home/.codex/usage_history.sqlite3.bak.1" \
    "$fake_home/.codex/usage_reset_hint.json" \
    "$fake_home/.codex/session.jsonl" \
    "$fake_home/.config/codex-info/settings.json"; do
    [[ -f "$retained" ]] || fail "remove deleted retained data: $retained"
done
printf 'case remove-retention: PASS\n'
printf 'linux-bundle-test: PASS (canonical version/target/compatibility, persistent boot/daily updater, exact-five release selection, integrity-before-replace, confirmed rollback, remove retention)\n'

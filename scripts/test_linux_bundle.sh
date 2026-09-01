#!/usr/bin/env bash
set -euo pipefail

# Finite local contract test.  The fake systemctl/curl commands model only the
# user-manager, enable/restart/active, and health observations needed by the
# bundle installer; no host service, HOME, DB, session, or network is touched.
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
mkdir -p -- "$fake_bin" "$fake_home" "$fixture_root" "$output_root"

cat > "$fake_bin/systemctl" <<'FAKE_SYSTEMCTL'
#!/usr/bin/env bash
set -euo pipefail
printf 'systemctl %s\n' "$*" >> "$FAKE_LOG"
if [[ " $* " == *' show-environment '* ]]; then
    exit "${FAKE_SYSTEMCTL_SHOW_ENVIRONMENT_STATUS:-0}"
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
if [[ " $* " == *' is-active '* ]]; then
    [[ "${FAKE_SYSTEMCTL_ACTIVE:-1}" == 1 ]]
fi
exit 0
FAKE_SYSTEMCTL
chmod 0755 "$fake_bin/systemctl"

cat > "$fake_bin/curl" <<'FAKE_CURL'
#!/usr/bin/env bash
set -euo pipefail
printf 'curl %s\n' "$*" >> "$FAKE_LOG"
printf '{"api_version":"v1","service":"codex-info","product_version":"%s"}\n' \
    "$FAKE_HEALTH_VERSION"
if [[ "${FAKE_CURL_FAIL:-0}" == 1 ]]; then
    exit 1
fi
exit 0
FAKE_CURL
chmod 0755 "$fake_bin/curl"

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
        SYSTEMCTL_BIN=systemctl CURL_BIN=curl \
        bash "$install_script" --bundle "$archive"
}

run_remove() {
    local archive="$1" install_script
    install_script="$(extract_install_script "$archive")"
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
    for required_path in codex_info codex-info.service install.sh LICENSE COPYRIGHT THIRD_PARTY_NOTICES.md manifest.json SHA256SUMS; do
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

assert_binary_marker() {
    local path="$1" expected="$2"
    [[ -f "$path" ]] || fail "missing file: $path"
    grep -aFq -- "$expected" "$path" || fail "unexpected binary contents: $path"
}

archive_v1="$(build_bundle 1111111111111111111111111111111111111111 1.0.19)"
bundle_install_script="$(extract_install_script "$archive_v1")"
bundle_help="$(bash "$bundle_install_script" --help)"
grep -Fq -- 'usage: install.sh --bundle' <<<"$bundle_help" ||
    fail 'bundled help does not name the install.sh payload'
grep -Fq -- 'install.sh --remove' <<<"$bundle_help" ||
    fail 'bundled help does not name the remove command'
printf 'case bundled-help: PASS\n'
run_install "$archive_v1" >/dev/null
assert_binary_marker "$fake_home/.local/bin/codex_info" 'fixture binary generation one'
[[ -f "$fake_home/.config/systemd/user/codex-info.service" ]] ||
    fail 'normal install did not publish unit'
grep -Fq -- 'systemctl --user enable codex-info.service' "$log" ||
    fail 'normal install did not enable user unit'
grep -Fq -- 'systemctl --user is-active --quiet codex-info.service' "$log" ||
    fail 'normal install did not check active state'
grep -Fq -- 'curl --fail --silent --show-error --max-time 5 http://127.0.0.1:8787/v1/health' "$log" ||
    fail 'normal install did not check health'
printf 'case normal: PASS\n'

write_fixture_binary 'fixture binary generation two'
archive_v2="$(build_bundle 2222222222222222222222222222222222222222 1.0.20)"
if run_install "$archive_v2" 9.9.9 >/dev/null 2>&1; then
    fail 'health version mismatch case unexpectedly succeeded'
fi
assert_binary_marker "$fake_home/.local/bin/codex_info" 'fixture binary generation one'
printf 'case health-version-mismatch: PASS\n'
run_install "$archive_v2" >/dev/null
assert_binary_marker "$fake_home/.local/bin/codex_info" 'fixture binary generation two'
printf 'case update: PASS\n'

old_binary_sha="$(sha256sum "$fake_home/.local/bin/codex_info" | awk '{print $1}')"
old_unit_sha="$(sha256sum "$fake_home/.config/systemd/user/codex-info.service" | awk '{print $1}')"
write_fixture_binary 'fixture binary generation three'
archive_v3="$(build_bundle 3333333333333333333333333333333333333333 1.0.21)"
if FAKE_SYSTEMCTL_FAIL_RESTART_ONCE=1 run_install "$archive_v3" >/dev/null 2>&1; then
    fail 'restart failure case unexpectedly succeeded'
fi
[[ "$(sha256sum "$fake_home/.local/bin/codex_info" | awk '{print $1}')" == "$old_binary_sha" ]] ||
    fail 'restart failure did not retain previous binary'
[[ "$(sha256sum "$fake_home/.config/systemd/user/codex-info.service" | awk '{print $1}')" == "$old_unit_sha" ]] ||
    fail 'restart failure did not retain previous unit'
printf 'case restart-failure: PASS\n'

mkdir -p -- "$fake_home/.codex" "$fake_home/.config/codex-info" \
    "$fake_home/.local/share/codex-info" "$fake_home/.cache/codex-info"
printf '%s\n' 'database' > "$fake_home/.codex/usage_history.sqlite3"
printf '%s\n' 'backup' > "$fake_home/.codex/usage_history.sqlite3.bak.1"
printf '%s\n' 'reset hint' > "$fake_home/.codex/usage_reset_hint.json"
printf '%s\n' 'session' > "$fake_home/.codex/session.jsonl"
printf '%s\n' 'config' > "$fake_home/.config/codex-info/settings.json"
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
run_remove "$archive_v3" >/dev/null
[[ ! -e "$fake_home/.config/systemd/user/codex-info.service" ]] || fail 'remove retained unit'
[[ -f "$fake_home/.local/bin/codex_info" ]] || fail 'remove deleted installed binary'
for retained in \
    "$fake_home/.codex/usage_history.sqlite3" \
    "$fake_home/.codex/usage_history.sqlite3.bak.1" \
    "$fake_home/.codex/usage_reset_hint.json" \
    "$fake_home/.codex/session.jsonl" \
    "$fake_home/.config/codex-info/settings.json"; do
    [[ -f "$retained" ]] || fail "remove deleted retained data: $retained"
done
printf 'case remove-retention: PASS\n'
printf 'linux-bundle-test: PASS (target/compatibility rejection, normal, health-version mismatch, update, restart failure rollback, remove failure propagation, remove retention)\n'

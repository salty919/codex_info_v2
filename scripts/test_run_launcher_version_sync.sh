#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="$(readlink -f -- "${BASH_SOURCE[0]}")"

case "$(basename -- "$0")" in
    cargo)
        manifest=''
        while (( $# > 0 )); do
            if [[ "$1" == '--manifest-path' ]]; then
                manifest="$2"
                shift 2
            else
                shift
            fi
        done
        root="$(cd -- "$(dirname -- "$manifest")" && pwd)"
        mkdir -p -- "$root/target/release"
        cp -- "$SCRIPT_SOURCE" "$root/target/release/codex_info"
        chmod +x -- "$root/target/release/codex_info"
        printf 'cargo build\n' >> "$FAKE_LOG"
        exit 0
        ;;
    systemctl)
        printf 'systemctl %s\n' "$*" >> "$FAKE_LOG"
        if [[ " $* " == *' is-enabled '* ]]; then
            [[ "${FAKE_SYSTEMD_ENABLED:-0}" == 1 ]]
        fi
        exit 0
        ;;
    codex_info)
        printf 'exec %s\n' "$*" >> "$FAKE_LOG"
        exit 0
        ;;
esac

SCRIPT_DIR="$(dirname -- "$SCRIPT_SOURCE")"
ORIGINAL_PATH="$PATH"
TEST_ROOT="$(mktemp -d)"
trap 'rm -r -- "$TEST_ROOT"' EXIT

FIXTURE_ROOT="$TEST_ROOT/repo"
RUN_SH="$FIXTURE_ROOT/run.sh"
FAKE_BIN="$TEST_ROOT/bin"
FAKE_HOME="$TEST_ROOT/home"
LOG="$TEST_ROOT/log"
mkdir -p -- "$FIXTURE_ROOT/scripts" "$FIXTURE_ROOT/packaging" "$FAKE_BIN" "$FAKE_HOME/.local/bin"
cp -- "$SCRIPT_DIR/../run.sh" "$RUN_SH"
cp -- "$SCRIPT_DIR/install_systemd_recorder.sh" "$FIXTURE_ROOT/scripts/install_systemd_recorder.sh"
cp -- "$SCRIPT_DIR/../packaging/codex-info.service" "$FIXTURE_ROOT/packaging/codex-info.service"
ln -s -- "$SCRIPT_SOURCE" "$FAKE_BIN/cargo"
ln -s -- "$SCRIPT_SOURCE" "$FAKE_BIN/systemctl"

run_launcher() {
    HOME="$FAKE_HOME" \
    PATH="$FAKE_BIN:$ORIGINAL_PATH" \
    FAKE_SYSTEMD_ENABLED=1 \
    FAKE_LOG="$LOG" \
    "$RUN_SH" "$@"
}

assert_one_build() {
    [[ "$(rg -c '^cargo ' "$LOG")" -eq 1 ]]
}

assert_one_exec() {
    [[ "$(rg -c '^exec' "$LOG")" -eq 1 ]]
}

assert_no_restart() {
    ! rg -qF -- 'restart codex-info.service' "$LOG"
}

assert_no_systemd() {
    ! rg -qF -- 'is-enabled' "$LOG"
}

printf 'old binary\n' > "$FAKE_HOME/.local/bin/codex_info"
: > "$LOG"
run_launcher --ui
assert_one_build
assert_one_exec
cmp -s -- "$FAKE_HOME/.local/bin/codex_info" "$FIXTURE_ROOT/target/release/codex_info"
rg -qF -- 'restart codex-info.service' "$LOG"

: > "$LOG"
run_launcher
assert_one_build
assert_one_exec
assert_no_restart

printf 'old binary\n' > "$FAKE_HOME/.local/bin/codex_info"
: > "$LOG"
run_launcher --help
assert_one_build
assert_one_exec
assert_no_systemd

printf 'old binary\n' > "$FAKE_HOME/.local/bin/codex_info"
: > "$LOG"
run_launcher --port 4321
assert_one_build
assert_one_exec
assert_no_systemd

: > "$LOG"
run_launcher --unknown >/dev/null 2>&1 || true
assert_one_build
assert_one_exec
assert_no_systemd

printf 'run.sh launcher sync cases passed\n'

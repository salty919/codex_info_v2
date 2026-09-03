#!/usr/bin/env bash
set -euo pipefail

# Regression test for the old launcher, which built target/release and copied
# it into ~/.local/bin. This uses an isolated HOME and fake installed
# authority; it never invokes Cargo, systemd, or a repository binary.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d /tmp/codex-info-launcher-test.XXXXXX)"
trap 'rm -r -- "$TEST_ROOT"' EXIT
HOME_FIXTURE="$TEST_ROOT/home"
mkdir -p -- "$HOME_FIXTURE/.local/bin" "$HOME_FIXTURE/.local/libexec" \
    "$HOME_FIXTURE/.local/share/codex-info/current"
LOG="$TEST_ROOT/launcher.log"

cat > "$HOME_FIXTURE/.local/share/codex-info/current/install.sh" <<'FAKE_INSTALLER'
#!/usr/bin/env bash
set -euo pipefail
printf 'installer %s\n' "$*" >> "$FAKE_LAUNCHER_LOG"
case "$1" in
    --start) exit 0 ;;
    --verify-runtime) exit 0 ;;
    --stop|--disable-autostart|--remove|--status|--update) exit 0 ;;
    *) exit 2 ;;
esac
FAKE_INSTALLER
chmod 0755 "$HOME_FIXTURE/.local/share/codex-info/current/install.sh"
ln -s -- '../share/codex-info/current/install.sh' \
    "$HOME_FIXTURE/.local/libexec/codex-info-install.sh"
cat > "$HOME_FIXTURE/.local/share/codex-info/current/codex_info" <<'FAKE_PAYLOAD'
#!/usr/bin/env bash
set -euo pipefail
printf 'payload %s\n' "$*" >> "$FAKE_LAUNCHER_LOG"
if [[ "${1:-}" == --help ]]; then
    [[ "${CODEX_INFO_LAUNCHER_HELP:-}" == 1 ]] || exit 3
    printf '%s\n' '--start --ui --stop --disable-autostart --remove --status --update --help'
fi
FAKE_PAYLOAD
chmod 0755 "$HOME_FIXTURE/.local/share/codex-info/current/codex_info"
ln -s -- '../share/codex-info/current/codex_info' \
    "$HOME_FIXTURE/.local/bin/codex_info"
cp -- "$ROOT_DIR/run.sh" "$TEST_ROOT/run.sh"
chmod 0755 "$TEST_ROOT/run.sh"
cmp -s -- "$ROOT_DIR/run.sh" "$TEST_ROOT/run.sh"

run_launcher() {
    HOME="$HOME_FIXTURE" FAKE_LAUNCHER_LOG="$LOG" "$TEST_ROOT/run.sh" "$@"
}
assert_log() { grep -Fq -- "$1" "$LOG" || { echo "missing log: $1" >&2; exit 1; }; }
assert_no_log() { ! grep -Fq -- "$1" "$LOG" || { echo "unexpected log: $1" >&2; exit 1; }; }

: > "$LOG"
run_launcher
assert_log 'installer --start'
! grep -Fq -- 'payload ' "$LOG" || { echo 'start unexpectedly launched payload' >&2; exit 1; }
: > "$LOG"
run_launcher --start
assert_log 'installer --start'
! grep -Fq -- 'payload ' "$LOG" || { echo '--start unexpectedly launched payload' >&2; exit 1; }
: > "$LOG"
run_launcher --ui
assert_log 'installer --start'
assert_log 'payload --ui'

for option in --stop --disable-autostart --remove --status --update; do
    : > "$LOG"
    run_launcher "$option"
    assert_log "installer $option"
    assert_no_log --verify-runtime
done

: > "$LOG"
help_output="$(run_launcher --help)"
for option in --start --ui --stop --disable-autostart --remove --status --update --help; do
    grep -Fq -- "$option" <<<"$help_output" \
        || { echo "launcher help omitted $option" >&2; exit 1; }
done
! grep -Fq -- '--port' <<<"$help_output" \
    || { echo 'launcher help exposed payload-only --port' >&2; exit 1; }
assert_log 'payload --help'
assert_no_log 'installer '

for bad in '--port' 8787 --unknown -h; do
    : > "$LOG"
    if run_launcher "$bad" >/dev/null 2>&1; then
        echo "unsupported option unexpectedly succeeded: $bad" >&2
        exit 1
    fi
    [[ ! -s "$LOG" ]] || { echo "unsupported option mutated authority: $bad" >&2; exit 1; }
done
: > "$LOG"
if run_launcher --ui --stop >/dev/null 2>&1; then
    echo 'mixed wrapper options unexpectedly succeeded' >&2
    exit 1
fi
[[ ! -s "$LOG" ]] || { echo 'mixed wrapper options mutated authority' >&2; exit 1; }

if grep -Eq -- '(^|[[:space:]])cargo([[:space:]]|$)' "$ROOT_DIR/run.sh"; then
    echo 'repository launcher still contains a cargo invocation' >&2
    exit 1
fi
printf 'run launcher anti-downgrade cases passed\n'

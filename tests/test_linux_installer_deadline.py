"""Finite deadline/propagation model; no live services, network or deployment."""

import contextlib
import pathlib
import re
import subprocess  # nosec B404 # fixed offline Bash fixture.
import tempfile
import unittest

INSTALLER = pathlib.Path(__file__).resolve().parents[1] / "packaging/install_linux_bundle.sh"

FUNCTIONS = (
    "initialize_mutating_action",
    "deadline_timeout",
    "systemctl_user",
    "systemctl_stop_user",
    "probe_active",
    "capture_runtime_state",
    "restore_runtime_state",
    "wait_inactive",
    "wait_runtime_ready",
    "activate_candidate",
    "verify_candidate",
    "rollback_transaction",
    "perform_install",
    "run_update",
)

MODEL = r'''
fixture_root="$FIXTURE_ROOT"
share_dir="$fixture_root/share"
backup_dir="$share_dir/legacy-backups"
generations_dir="$share_dir/generations"
install_lock="$share_dir/.install.lock"
transaction="$share_dir/transaction.json"
current_link="$share_dir/current"
clock="$fixture_root/clock"
trace="$fixture_root/trace"
mutations="$fixture_root/mutations"
journal="$fixture_root/journal"
TMPDIR="$fixture_root/tmp"
export TMPDIR
mkdir -p "$generations_dir/old" "$TMPDIR" "$fixture_root/entries"
printf '%s\n' "$CLOCK_START" > "$clock"
printf '{}\n' > "$generations_dir/old/manifest.json"
: > "$trace"
: > "$install_lock"
chmod 600 "$install_lock"
exec 9<"$install_lock"
flock --exclusive --nonblock 9
ln -s generations/old "$current_link"
for unit in codex-info-recorder.service codex-info-rest.service codex-info-update.timer; do
    printf '1\n' > "$fixture_root/$unit.enabled"
    printf '0\n' > "$fixture_root/$unit.active"
done
printf '1\n' > "$fixture_root/codex-info-recorder.service.active"
printf '1\n' > "$fixture_root/codex-info-rest.service.active"
printf '0\n' > "$fixture_root/codex-info-rest.service.enabled"
printf 'original-session\n' > "$fixture_root/Session"
printf 'original-database\n' > "$fixture_root/database"
printf 'original-control\n' > "$fixture_root/control"

for destination_variable in binary_destination recorder_binary_destination rest_binary_destination launcher_destination installer_destination manifest_destination unit_destination rest_unit_destination update_service_destination update_timer_destination; do
    printf -v "$destination_variable" '%s' "$fixture_root/entries/$destination_variable"
done

SYSTEMCTL_BIN=fixture_systemctl
CURL_BIN=fixture_curl
TRIGGER="$FIXTURE_TRIGGER"
ACTION=install
[[ "$FIXTURE_CASE" != update ]] || ACTION=update

now_unix() { cat "$clock"; }
sleep_interval() { printf '%s\n' "$(( $(now_unix) + $1 ))" > "$clock"; }
die() { printf 'INSTALL_FAILED: %s\n' "$*" >&2; exit 1; }
safe_blocked() { printf 'SAFE_BLOCKED: %s\n' "$*" >&2; exit 88; }
cleanup_candidate_stage() { :; }

timeout() {
    [[ "$1" == --foreground ]] || exit 99
    shift
    local limit="$1"
    shift
    local at
    at="$(now_unix)"
    printf '%s %s %s\n' "$at" "$limit" "$*" >> "$trace"
    if [[ "$1" == "$0" && "${2:-}" == --bundle ]]; then
        (
            ACTION=install
            operation_deadline=0
            requested_deadline="$CODEX_INFO_DEADLINE"
            initialize_mutating_action
            printf '%s %s %s %s\n' \
                "$CODEX_INFO_DEADLINE" "$operation_deadline" "$at" "$limit" \
                > "$fixture_root/child"
        )
        return
    fi
    # Only the candidate activation reaches the passed phase deadline.
    # This is the injected stimulus, never the expected deadline oracle.
    if [[ "$FIXTURE_CASE" != update &&
          "$1" == fixture_systemctl && "${2:-}" == --user &&
          "${3:-}" == daemon-reload &&
          "$(current_generation)" != old ]]; then
        printf '%s\n' "$((at + limit))" > "$clock"
        return 124
    fi
    "$@"
}

fixture_systemctl() {
    [[ "$1" == --user ]] || exit 99
    shift
    local action="$1" unit="${@: -1}" status=0
    case "$action" in
        is-active)
            [[ "$(cat "$fixture_root/$unit.active")" == 1 ]] || status=3
            ;;
        start|restart)
            printf '1\n' > "$fixture_root/$unit.active"
            ;;
        stop)
            printf '0\n' > "$fixture_root/$unit.active"
            ;;
        daemon-reload|show-environment|reset-failed) ;;
        *) exit 99 ;;
    esac
    printf '%s\n' "$(( $(now_unix) + 1 ))" > "$clock"
    return "$status"
}
probe_enabled() { [[ "$(cat "$fixture_root/$1.enabled")" == 1 ]]; }
enable_managed_unit() {
    printf '1\n' > "$fixture_root/$1.enabled"
    systemctl_user daemon-reload
}
disable_managed_unit() {
    printf '0\n' > "$fixture_root/$1.enabled"
    systemctl_user daemon-reload
}
require_user_manager() { systemctl_user show-environment; }
load_control_state() { desired_state=running; }
current_generation() {
    local target
    target="$(readlink "$current_link")"
    printf '%s\n' "${target#generations/}"
}
new_operation_id() { printf 'fixture-operation\n'; }
manifest_record() {
    printf '1.0.0\tsource\tmanifest\t%s\n' \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
}
validate_bundle() {
    printf '2.0.0\tsource\tmanifest\t%s\n' \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
}
check_glibc_compatibility() { :; }
recorder_execution_record() { printf 'canonical\n'; }
legacy_combined_present() { return 1; }
legacy_flat_present() { return 1; }
systemd_pid() { printf '123\n'; }
write_journal() {
    printf '%s\n' mutation >> "$mutations"
    printf '%s %s %s\n' "$1" "${2:-}" "$(now_unix)" >> "$journal"
}
retire_legacy_combined() { :; }
retire_known_unmanaged() { :; }
enforce_desired_state() { :; }
backup_legacy_path() { :; }
link_entrypoints() { :; }
extract_candidate() { :; }
publish_candidate() {
    mkdir -p "$2"
    for unit in codex-info-recorder.service codex-info-rest.service codex-info-update.timer; do
        printf '1\n' > "$fixture_root/$unit.enabled"
        printf '0\n' > "$fixture_root/$unit.active"
    done
    printf '%s\n' "$((operation_deadline - 1))" > "$clock"
}
verify_generation_files() { :; }
atomic_symlink() { ln -sfn -- "$1" "$2"; }
atomic_unlink() { rm -f -- "$1"; }
remove_published_entrypoints() { :; }
restore_backups() { :; }
ensure_entrypoints_for_generation() { :; }
converge_enable_links() { :; }
reset_failed_main() { :; }
rearm_update_timer() { :; }
recorder_artifact_matches_previous() { return 1; }
verify_local_generation() { :; }
verify_runtime() {
    [[ "$(current_generation)" == old &&
       "$(cat "$fixture_root/codex-info-recorder.service.active")" == 1 &&
       "$(cat "$fixture_root/codex-info-rest.service.active")" == 1 ]]
}
write_control_state() { :; }
prune_obsolete_generations() { :; }
preflight_listener_owner() { :; }
verify_fixed_links_local() { :; }
fixture_curl() { printf '[]\n'; }
select_release() {
    printf 'update\t2.0.0\n'
    printf 'archive\tfixture-archive\t1\tdigest\n'
    printf 'checksum\tfixture-checksum\t1\tdigest\n'
    printf 'manifest\tfixture-manifest\t1\tdigest\n'
}
download_asset() { :; }
update_failure_with_fallback() { die "$*"; }

initialize_mutating_action
if [[ "$FIXTURE_CASE" == update ]]; then
    run_update
else
    perform_install
fi
'''


class InstallerDeadlineTests(unittest.TestCase):
    @contextlib.contextmanager
    def fixture(self, case, start=0, trigger="manual"):
        source = INSTALLER.read_text()
        header = source.split('\nhome_dir="$HOME"', 1)[0]
        functions = []
        for name in FUNCTIONS:
            matches = re.findall(rf"(?ms)^{name}\(\) \{{\n.*?^\}}\n", source)
            self.assertEqual(len(matches), 1, f"fixture function: {name}")
            functions.append(matches[0])
        # An absent new helper is valid on the RED baseline.
        functions.extend(re.findall(
            r"(?ms)^reserve_rollback_budget\(\) \{\n.*?^\}\n", source
        ))
        with tempfile.TemporaryDirectory(prefix="issue455-deadline-") as directory:
            root = pathlib.Path(directory)
            script = header + "\n" + "\n".join(functions) + MODEL
            result = subprocess.run(  # nosec B603 # fixed Bash/env; offline repository functions.
                ["/bin/bash", "--noprofile", "--norc", "-s"],
                input=script,
                env={
                    "PATH": "/usr/bin:/bin",
                    "LC_ALL": "C",
                    "FIXTURE_ROOT": str(root),
                    "FIXTURE_CASE": case,
                    "FIXTURE_TRIGGER": trigger,
                    "CLOCK_START": str(start),
                    "CODEX_INFO_INSTALL_LOCKED": "1",
                    "CODEX_INFO_DEADLINE": "1230",
                },
                capture_output=True,
                text=True,
                check=False,
                timeout=5,
            )
            yield root, result

    def assert_old_state(self, root):
        self.assertEqual((root / "share/current").readlink().as_posix(), "generations/old")
        flags = []
        for unit in (
            "codex-info-recorder.service",
            "codex-info-rest.service",
            "codex-info-update.timer",
        ):
            flags.extend(
                int((root / f"{unit}.{field}").read_text())
                for field in ("enabled", "active")
            )
        # Literal initial service state; independent of capture/restore helpers.
        self.assertEqual(flags, [1, 1, 0, 1, 1, 0])
        self.assertEqual((root / "Session").read_text(), "original-session\n")
        self.assertEqual((root / "database").read_text(), "original-database\n")
        self.assertEqual((root / "control").read_text(), "original-control\n")

    def test_activation_timeout_restores_old_generation_inside_original_deadline(self):
        with self.fixture("activation") as (root, result):
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assert_old_state(root)
            journal = (root / "journal").read_text().splitlines()
            phases = [line.split()[0] for line in journal]
            self.assertIn("rollback_verified", phases)
            self.assertEqual(phases[-1], "committed")
            self.assertIn("committed rolled_back ", journal[-1])
            rollback_at = next(
                int(line.split()[-1])
                for line in journal if line.startswith("rollback_switched ")
            )
            self.assertLessEqual(rollback_at, 1170)
            self.assertLessEqual(int((root / "clock").read_text()), 1230)
            recovery = []
            for line in (root / "trace").read_text().splitlines():
                at, limit, command = line.split(" ", 2)
                if int(at) >= rollback_at:
                    self.assertGreater(int(limit), 0, line)
                    self.assertLessEqual(int(at) + int(limit), 1230, line)
                    recovery.append(command)
            self.assertIn("fixture_systemctl --user daemon-reload", recovery)
            self.assertIn(
                "fixture_systemctl --user restart --no-block codex-info-recorder.service",
                recovery,
            )
            self.assertIn(
                "fixture_systemctl --user restart --no-block codex-info-rest.service",
                recovery,
            )

    def test_insufficient_rollback_budget_refuses_before_mutation(self):
        with self.fixture("insufficient", start=1180) as (root, result):
            self.assertEqual(result.returncode, 88, result.stderr)
            self.assertFalse((root / "mutations").exists(), "old installation was mutated")
            self.assertFalse((root / "journal").exists())
            self.assert_old_state(root)
            self.assertLessEqual(int((root / "clock").read_text()), 1230)

    def test_update_child_never_extends_parent_deadline(self):
        for trigger in ("manual", "timer", "startup"):
            with (
                self.subTest(trigger=trigger),
                self.fixture("update", start=100, trigger=trigger) as (root, result),
            ):
                self.assertEqual(result.returncode, 0, result.stderr)
                passed, child, at, limit = map(
                    int, (root / "child").read_text().split()
                )
                self.assertEqual(passed, 1230)
                self.assertEqual(child, 1230)
                self.assertGreater(limit, 0)
                self.assertLessEqual(at + limit, 1230)


if __name__ == "__main__":
    unittest.main()

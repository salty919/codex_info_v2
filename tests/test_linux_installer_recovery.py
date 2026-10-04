"""Literal predecessor snapshots resumed across separate installer processes."""

import hashlib
import json
import pathlib
import re
import shlex
import subprocess  # nosec B404 # offline Bash fixture, no host services.
import tempfile
import unittest

INSTALLER = pathlib.Path(__file__).resolve().parents[1] / "packaging/install_linux_bundle.sh"
OPERATION = "literal-recovery-operation"
MEMBERS = {
    "codex_info": (b"literal old combined payload\n", 0o755),
    "install.sh": (b"#!/bin/bash\n# literal old installer\n", 0o755),
    "codex-info.service": (b"[Service]\nExecStart=%h/.local/bin/codex_info --daemon\n", 0o644),
    "codex-info-update.service": (b"[Service]\n# literal old updater\n", 0o644),
    "codex-info-update.timer": (b"[Timer]\nOnActiveSec=5min\n", 0o644),
}
FUNCTIONS = (
    "atomic_text", "atomic_symlink", "atomic_unlink", "write_journal", "read_journal",
    "journal_owner_stale", "current_generation", "legacy_combined_record_at",
    "legacy_combined_record", "legacy_combined_present", "validate_legacy_combined_enable_link",
    "capture_legacy_combined_state", "capture_runtime_state", "backup_legacy_path",
    "backup_legacy_combined_unit", "retire_legacy_combined", "restore_backups",
    "remove_published_entrypoints", "restore_legacy_combined_entrypoints",
    "restore_legacy_combined_runtime", "verify_legacy_combined_terminal",
    "recover_legacy_combined_state", "rollback_transaction", "resume_transaction",
    "perform_install", "enforce_desired_state",
)

MODEL = r"""
update_stage=
operation_id=
previous_id=
candidate_id=
desired_state="$DESIRED"
trace="$HOME/trace"
systemctl_user() {
    local action="$1" unit="${@: -1}"
    case "$action" in
        show) printf '4242\n' ;;
        daemon-reload) : ;;
        start|restart) printf '1\n' > "$HOME/$unit.active"; printf '%s %s\n' "$action" "$unit" >> "$trace" ;;
        disable)
            rm -f "$legacy_combined_enable_destination"
            printf 'disable %s\n' "$unit" >> "$trace"
            if [[ "$CRASH_STOP" == 1 && "$unit" == codex-info.service ]]; then kill -KILL "$$"; fi
            ;;
        *) exit 99 ;;
    esac
}
systemctl_stop_user() {
    printf '0\n' > "$HOME/${@: -1}.active"
    printf 'stop %s\n' "${@: -1}" >> "$trace"
}
probe_active() { [[ "$(cat "$HOME/$1.active" 2>/dev/null || true)" == 1 ]]; }
probe_enabled() { return 1; }
probe_legacy_combined_enabled() { [[ -L "$legacy_combined_enable_destination" ]]; }
disable_managed_unit() { :; }
wait_inactive() { ! probe_active "$1"; }
socket_pid() { if probe_active codex-info.service; then printf '4242\n'; fi; }
legacy_combined_listener_matches() { probe_active codex-info.service && legacy_combined_record >/dev/null; }
proc_starttime() { printf '1000\n'; }
owner_starttime() { printf '1000\n'; }
boot_id() { printf 'literal-boot\n'; }
now_unix() { cat "$HOME/clock"; }
sleep_interval() { printf '%s\n' "$(( $(now_unix) + $1 ))" > "$HOME/clock"; }
update_log() { :; }
die() { printf '%s\n' "$*" >&2; exit 1; }
safe_blocked() { printf 'SAFE_BLOCKED: %s\n' "$*" >&2; exit 88; }
require_user_manager() { :; }
load_control_state() { desired_state="$DESIRED"; }
new_operation_id() { printf 'literal-recovery-operation\n'; }
validate_bundle() { printf '2.0.0\t%s\t%s\t%s\n' "$(printf '2%.0s' {1..40})" "$(printf 'b%.0s' {1..64})" "$(printf 'c%.0s' {1..64})"; }
check_glibc_compatibility() { :; }
recorder_execution_record() { printf 'canonical\n'; }
legacy_combined_mixed_split_present() { return 1; }
legacy_flat_present() { return 1; }
reserve_rollback_budget() { :; }
retire_known_unmanaged() { :; }
ensure_entrypoints_for_generation() { :; }
verify_local_generation() { return 1; }
verify_runtime() { return 1; }
curl() {
    case "${@: -1}" in
        */v1/health) printf '%s\n' '{"api_version":"v1","service":"codex-info","product_version":"1.0.48"}' ;;
        */v1/details) printf '{"state":"%s","observed_at":100}\n' "$DETAILS_STATE" ;;
        *) return 99 ;;
    esac
}
python3() { /usr/bin/python3 "$SHIM" "$@"; }
if [[ "$STEP" == install ]]; then
    perform_install
else
    resume_transaction
fi
"""

SHIM = r"""import os, pathlib, signal, sys
assert sys.argv[1] == "-"
sys.argv = sys.argv[1:]
code = sys.stdin.read()
original_replace = os.replace
def replace(source, destination):
    original_replace(source, destination)
    if (os.environ["CRASH_RESTORE"] == "1" and
            pathlib.Path(source).name == "literal-recovery-operation-codex-info.service"):
        pathlib.Path(os.environ["HOME"], "partial-receipt").write_text("unit restored; old manifest/payload pending\n")
        os.kill(os.getppid(), signal.SIGKILL)
        raise SystemExit(75)
os.replace = replace
exec(compile(code, "installer-inline-python", "exec"))
"""


class LegacyRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="legacy-recovery-")
        self.addCleanup(self.temp.cleanup)
        self.home = pathlib.Path(self.temp.name)
        self.share = self.home / ".local/share/codex-info"
        self.backups = self.share / "legacy-backups"
        self.units = self.home / ".config/systemd/user"
        self.paths = {
            "codex_info": self.home / ".local/bin/codex_info",
            "install.sh": self.home / ".local/libexec/codex-info-install.sh",
            "manifest.json": self.share / "manifest.json",
            **{name: self.units / name for name in MEMBERS if name.endswith((".service", ".timer"))},
        }
        for path in (*self.paths.values(), self.backups / "unused", self.units / "default.target.wants/unused"):
            path.parent.mkdir(parents=True, exist_ok=True)
        self.backups.chmod(0o700)
        self.manifest = json.dumps({
            "schema": "codex-info-linux-bundle-v1", "product": "codex_info", "version": "1.0.48",
            "source_sha": "3" * 40, "run_id": "100", "run_attempt": 1,
            "target": "x86_64-unknown-linux-gnu", "compatibility": "glibc", "glibc_minimum": "2.31",
            "files": [{"path": name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                      for name, (data, _) in sorted(MEMBERS.items())],
        }, indent=2).encode() + b"\n"
        self.snapshot = {**MEMBERS, "manifest.json": (self.manifest, 0o644)}
        self.old_id = "1.0.48-" + "3" * 40 + "-" + hashlib.sha256(self.manifest).hexdigest()
        self.enable = self.units / "default.target.wants/codex-info.service"
        self.journal = self.share / "install-transaction.json"
        self.shim = self.home / "shim.py"
        self.shim.write_text(SHIM)
        (self.home / "clock").write_text("100\n")
        self.sentinels = {name: f"literal untouched {name}\n".encode() for name in ("database", "config", "Session")}
        for name, data in self.sentinels.items():
            (self.home / name).write_bytes(data)
        self.generation = False

    def seed(self, generation=False):
        self.generation = generation
        if generation:
            directory = self.share / "generations" / self.old_id
            directory.mkdir(parents=True)
            directory.chmod(0o700)
            (self.share / "current").symlink_to("generations/" + self.old_id)
        for name, (data, mode) in self.snapshot.items():
            actual = directory / name if generation else self.paths[name]
            actual.write_bytes(data)
            actual.chmod(mode)
            if generation:
                link = ("current/manifest.json" if name == "manifest.json" else
                        "../share/codex-info/current/" + name if name in {"codex_info", "install.sh"} else
                        "../../../.local/share/codex-info/current/" + name)
                self.paths[name].symlink_to(link)
        self.enable.symlink_to("../codex-info.service")
        (self.home / "codex-info.service.active").write_text("1\n")

    def run_step(self, step, *, desired="running", crash_stop=False, crash_restore=False, details="ready"):
        source = INSTALLER.read_text()
        prefix = source.split("\nusage() {", 1)[0]
        functions = []
        for name in FUNCTIONS:
            found = re.findall(rf"(?ms)^{re.escape(name)}\(\) \{{\n.*?^\}}\n", source)
            self.assertEqual(len(found), 1, name)
            functions.append(found[0])
        # Optional on the defect baseline; actual production functions after the fix.
        for name in ("capture_legacy_combined_prestate", "validate_legacy_combined_recovery", "wait_legacy_combined_ready"):
            functions.extend(re.findall(rf"(?ms)^{name}\(\) \{{\n.*?^\}}\n", source))
        script = prefix + "\n" + "\n".join(functions) + MODEL
        return subprocess.run(  # nosec B603 # fixed shell/env; literal fixture and repository functions.
            ["/bin/bash", "--noprofile", "--norc", "-s"], input=script,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(self.home), "SHIM": str(self.shim),
                 "STEP": step, "DESIRED": desired, "CRASH_STOP": str(int(crash_stop)),
                 "CRASH_RESTORE": str(int(crash_restore)), "DETAILS_STATE": details},
            capture_output=True, text=True, check=False, timeout=8,
        )

    def assert_snapshot(self, *, enabled=True, active=True):
        for name, (data, mode) in self.snapshot.items():
            path = self.paths[name]
            self.assertEqual(path.read_bytes(), data, name)
            self.assertEqual(path.stat().st_mode & 0o777, mode, name)
        self.assertEqual(self.enable.is_symlink(), enabled)
        self.assertEqual((self.home / "codex-info.service.active").read_text(), "1\n" if active else "0\n")
        for name, data in self.sentinels.items():
            self.assertEqual((self.home / name).read_bytes(), data)
        document = json.loads(self.journal.read_text())
        self.assertEqual(document["operation_id"], OPERATION)
        self.assertEqual(document["phase"], "committed")
        self.assertEqual(document["old_generation"], self.old_id if self.generation else "")

    def retired_runtime(self, generation):
        self.seed(generation)
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        self.assertEqual((self.home / "codex-info.service.active").read_text(), "0\n")
        self.assertFalse(self.enable.is_symlink())
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()
        trace = (self.home / "trace").read_text()
        document = self.journal.read_bytes()
        again = self.run_step("resume")
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(self.journal.read_bytes(), document)
        self.assertEqual((self.home / "trace").read_text(), trace)
        self.assertFalse(list(self.backups.iterdir()))

    def test_retired_active_combined_resumes_original_runtime(self):
        self.retired_runtime(False)

    def test_retired_generation_combined_resumes_original_runtime(self):
        self.retired_runtime(True)

    def partial_restore(self):
        self.seed()
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        for path in self.paths.values():
            path.replace(self.backups / (OPERATION + "-" + path.name))
        interrupted = self.run_step("resume", crash_restore=True)
        self.assertEqual(interrupted.returncode, -9, interrupted.stderr)
        self.assertEqual(self.paths["codex-info.service"].read_bytes(), MEMBERS["codex-info.service"][0])
        self.assertFalse(self.paths["manifest.json"].exists())
        self.assertFalse(self.paths["codex_info"].exists())
        self.assertTrue((self.home / "partial-receipt").exists())

    def test_partial_unit_restore_resumes_remaining_snapshot(self):
        self.partial_restore()
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()
        self.assertFalse(list(self.backups.iterdir()))

    def test_nonrunning_intent_survives_retired_prestate(self):
        for desired, enabled in (("stopped", True), ("disabled", False)):
            with self.subTest(desired=desired):
                if desired == "disabled":
                    self.doCleanups()
                    self.setUp()
                self.seed()
                self.assertEqual(self.run_step("install", desired=desired, crash_stop=True).returncode, -9)
                recovered = self.run_step("resume", desired=desired)
                self.assertEqual(recovered.returncode, 0, recovered.stderr)
                self.assert_snapshot(enabled=enabled, active=False)

    def test_foreign_file_blocks_before_remaining_backups_are_consumed(self):
        self.partial_restore()
        self.paths["codex_info"].write_bytes(b"foreign payload must remain\n")
        self.paths["codex_info"].chmod(0o755)
        before = {p.name: p.read_bytes() for p in self.backups.iterdir()}
        journal = self.journal.read_bytes()
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 88, recovered.stderr)
        self.assertEqual(self.paths["codex_info"].read_bytes(), b"foreign payload must remain\n")
        self.assertEqual({p.name: p.read_bytes() for p in self.backups.iterdir()}, before)
        self.assertEqual(self.journal.read_bytes(), journal)

    def test_foreign_generation_unit_blocks_before_rollback(self):
        self.seed(generation=True)
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        unit = self.paths["codex-info.service"]
        unit.unlink()  # Only this isolated HOME's known unit symlink.
        foreign = b"[Service]\nExecStart=/foreign/must-remain\n"
        unit.write_bytes(foreign)
        unit.chmod(0o644)
        journal = self.journal.read_bytes()
        backups = {p.name: p.read_bytes() for p in self.backups.iterdir()}
        trace = (self.home / "trace").read_bytes()
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 88, recovered.stderr)
        self.assertFalse(unit.is_symlink(), "foreign regular unit was overwritten")
        self.assertEqual(unit.read_bytes(), foreign)
        self.assertEqual(self.journal.read_bytes(), journal)
        self.assertEqual({p.name: p.read_bytes() for p in self.backups.iterdir()}, backups)
        self.assertEqual((self.home / "trace").read_bytes(), trace)
        for name, data in self.sentinels.items():
            self.assertEqual((self.home / name).read_bytes(), data)

    def test_initializing_old_runtime_never_commits_running_terminal(self):
        self.seed()
        self.assertEqual(self.run_step("install", crash_stop=True).returncode, -9)
        recovered = self.run_step("resume", details="initializing")
        self.assertEqual(recovered.returncode, 88, recovered.stderr)
        self.assertNotEqual(json.loads(self.journal.read_text())["phase"], "committed")


if __name__ == "__main__":
    unittest.main()

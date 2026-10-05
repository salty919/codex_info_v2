"""Literal predecessor snapshots resumed across separate installer processes."""

import base64
import gzip
import hashlib
import json
import pathlib
import re
import subprocess  # nosec B404 # offline Bash fixture, no host services.
import tempfile
import unittest

INSTALLER = pathlib.Path(__file__).resolve().parents[1] / "packaging/install_linux_bundle.sh"
OPERATION = "literal-recovery-operation"
PINNED_INSTALLER = INSTALLER.parents[1] / "tests/fixtures/linux_installer_before_456.sh.gz"
PINNED_INSTALLER_SHA256 = "9b88e9bdb9ab0fcf667ccbcd225c96fdef9e9aeabb36a652cadc5cabfcfe4126"
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
    "atomic_recovery_copy", "legacy_recovery_reader_source", "legacy_recovery_reader_valid",
    "legacy_recovery_reader_active", "prepare_legacy_recovery_reader",
    "validate_legacy_recovery_installer_binding", "publish_legacy_recovery_reader",
    "restore_legacy_recovery_installer", "validate_legacy_handoff_destination",
    "stage_legacy_recovery_handoff", "promote_legacy_recovery_handoff",
    "recover_legacy_reader_handoff", "finish_committed_legacy_recovery",
    "legacy_recovery_reader_image", "legacy_recovery_reader_prior_hash", "seal_legacy_reader_handoff",
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
legacy_recovery_reader_source() { printf '%s\n' "$CURRENT_INSTALLER"; }
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
original_fsync = os.fsync
cache_published = False
def replace(source, destination):
    global cache_published
    original_replace(source, destination)
    if pathlib.Path(destination).name == ".legacy-recovery-install.sh":
        cache_published = True
    if (os.environ["CRASH_RESTORE"] == "1" and
            pathlib.Path(source).name == sys.argv[1] + "-codex-info.service"):
        pathlib.Path(os.environ["HOME"], "partial-receipt").write_text("unit restored; old manifest/payload pending\n")
        os.kill(os.getppid(), signal.SIGKILL)
        raise SystemExit(75)
def fsync(descriptor):
    original_fsync(descriptor)
    if cache_published and os.environ.get("CRASH_CACHE") == "1":
        pathlib.Path(os.environ["HOME"], "cache-receipt").write_text("recovery cache file and directory fsynced\n")
        os.kill(os.getppid(), signal.SIGKILL)
        raise SystemExit(75)
os.replace = replace
os.fsync = fsync
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
        self.operation = None

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

    def run_step(self, step, *, desired="running", crash_stop=False, crash_restore=False, crash_cache=False, details="ready", interrupt_phase="", reader_source=None):
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
        result = subprocess.run(  # nosec B603 # fixed shell/env; literal fixture and repository functions.
            ["/bin/bash", "--noprofile", "--norc", "-s"], input=script,
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(self.home), "SHIM": str(self.shim),
                 "STEP": step, "DESIRED": desired, "CRASH_STOP": str(int(crash_stop)),
                 "CRASH_RESTORE": str(int(crash_restore)), "CRASH_CACHE": str(int(crash_cache)), "DETAILS_STATE": details,
                 "CODEX_INFO_INTERRUPT_PHASE": interrupt_phase, "CURRENT_INSTALLER": str(reader_source or INSTALLER)},
            capture_output=True, text=True, check=False, timeout=8,
        )

        if self.operation is None and self.journal.exists():
            self.operation = json.loads(self.journal.read_text())["operation_id"]
            self.assertEqual(self.operation.split("~lc1~", 1)[0], OPERATION)
        return result

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
        self.assertEqual(document["operation_id"], self.operation)
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

    def seed_pinned_installer(self):
        # Frozen pre-PR executable; no runtime dependency on Git history or HEAD.
        installed = gzip.decompress(PINNED_INSTALLER.read_bytes())
        self.assertEqual(hashlib.sha256(installed).hexdigest(), PINNED_INSTALLER_SHA256)
        self.snapshot["install.sh"] = (installed, 0o755)
        self.snapshot["codex-info.service"] = (
            (b"[Service]\nExecStartPre=%h/.local/libexec/codex-info-install.sh --startup-reconcile --quiet\n"
             b"ExecStart=%h/.local/bin/codex_info --daemon\n"), 0o644,
        )
        manifest = json.loads(self.manifest)
        manifest["source_sha"] = "f331961bc11dc090160ab86f9dc28c5683919fc3"
        manifest["files"] = [
            {"path": name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            for name, (data, _) in sorted(self.snapshot.items()) if name != "manifest.json"
        ]
        self.manifest = json.dumps(manifest, indent=2).encode() + b"\n"
        self.snapshot["manifest.json"] = (self.manifest, 0o644)
        self.old_id = "1.0.48-" + manifest["source_sha"] + "-" + hashlib.sha256(self.manifest).hexdigest()
        self.seed(generation=True)
        self.share.chmod(0o700)
        lock = self.share / ".install.lock"
        lock.write_bytes(b"")
        lock.chmod(0o600)

    def run_installed_startup(self):
        # The fixed executable runs its actual argv parser and startup path.
        # Override external runtime/transport operations after its definitions;
        # journal readers and recovery functions remain the executable's own.
        runtime = MODEL.split('\nif [[ "$STEP" == install', 1)[0]
        runtime += '\nrun_update() { printf "startup resolver reached\\n" >> "$trace"; }\n'
        (self.home / "startup-model.sh").write_text(runtime)
        hook = self.home / "startup-hook.sh"
        hook.write_text(
            'trap \'if [[ "$BASH_COMMAND" == initialize_mutating_action ]]; then '
            'trap - DEBUG; source "$HOME/startup-model.sh"; fi\' DEBUG\n'
        )
        return subprocess.run(  # nosec B603 # digest-pinned isolated executable, stub host effects.
            ["/bin/bash", str(self.paths["install.sh"]), "--startup-reconcile", "--quiet"],
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(self.home),
                 "BASH_ENV": str(hook), "SHIM": str(self.shim), "DESIRED": "running",
                 "CRASH_STOP": "0", "CRASH_RESTORE": "0", "DETAILS_STATE": "ready",
                 "CURRENT_INSTALLER": str(INSTALLER)},
            capture_output=True, text=True, check=False, timeout=8,
        )

    def test_prepared_recovery_uses_installed_startup_executable(self):
        for checkpoint in ("prepared", "retired"):
            with self.subTest(checkpoint=checkpoint):
                if checkpoint == "retired":
                    self.doCleanups()
                    self.setUp()
                self.seed_pinned_installer()
                first = self.run_step(
                    "install", crash_stop=checkpoint == "retired",
                    interrupt_phase="prepared" if checkpoint == "prepared" else "",
                )
                self.assertEqual(first.returncode, 75 if checkpoint == "prepared" else -9, first.stderr)
                self.assertEqual(json.loads(self.journal.read_text())["phase"], "prepared")
                old_executable = self.share / "generations" / self.old_id / "install.sh"
                self.assertEqual(hashlib.sha256(old_executable.read_bytes()).hexdigest(), PINNED_INSTALLER_SHA256)
                recovered = self.run_installed_startup()
                self.assertEqual(recovered.returncode, 0, recovered.stderr)
                self.assert_snapshot()
                self.assertIn("startup resolver reached", (self.home / "trace").read_text())

    def test_prepared_inactive_startup_preserves_active_prestate(self):
        self.seed_pinned_installer()
        first = self.run_step("install", interrupt_phase="prepared")
        self.assertEqual(first.returncode, 75, first.stderr)
        self.assertEqual(json.loads(self.journal.read_text())["phase"], "prepared")
        # A fresh ExecStartPre runs before the old daemon is active. Original
        # active intent is the literal pre-interruption oracle, not a live probe.
        (self.home / "codex-info.service.active").write_text("0\n")
        recovered = self.run_installed_startup()
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()

    def test_restored_installer_can_read_rollback_journal(self):
        self.seed_pinned_installer()
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        interrupted = self.run_step("resume", interrupt_phase="rollback_verified")
        self.assertEqual(interrupted.returncode, 75, interrupted.stderr)
        self.assertEqual(json.loads(self.journal.read_text())["phase"], "rollback_verified")
        # Keep the new reader selected until the transaction is committed;
        # the exact frozen old installer is restored at the final boundary.
        self.assertEqual(self.paths["install.sh"].read_bytes(), INSTALLER.read_bytes())
        recovered = self.run_installed_startup()
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()

    def test_rollback_verified_inactive_startup_preserves_active_prestate(self):
        self.seed_pinned_installer()
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        interrupted = self.run_step("resume", interrupt_phase="rollback_verified")
        self.assertEqual(interrupted.returncode, 75, interrupted.stderr)
        (self.home / "codex-info.service.active").write_text("0\n")
        recovered = self.run_installed_startup()
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()

    def test_staged_reader_handoff_resumes_the_same_prestate(self):
        for checkpoint in ("legacy_handoff_staged", "legacy_handoff_bound"):
            with self.subTest(checkpoint=checkpoint):
                if checkpoint == "legacy_handoff_bound":
                    self.doCleanups()
                    self.setUp()
                self.seed_pinned_installer()
                first = self.run_step("install", interrupt_phase=checkpoint)
                self.assertEqual(first.returncode, 75, first.stderr)
                pending = self.share / ".legacy-recovery-transaction.json"
                staged = pending.read_bytes()
                self.operation = json.loads(staged)["operation_id"]
                self.assertEqual(self.operation.split("~lc1~", 1)[0], OPERATION)
                self.assertFalse(self.journal.exists())
                self.assertEqual(pending.stat().st_mode & 0o777, 0o600)
                (self.home / "codex-info.service.active").write_text("0\n")
                if checkpoint == "legacy_handoff_staged":
                    # The old reader sees no pending canonical transaction;
                    # the untouched predecessor remains a valid startup target.
                    old_startup = self.run_installed_startup()
                    self.assertEqual(old_startup.returncode, 0, old_startup.stderr)
                    self.assertEqual(pending.read_bytes(), staged)
                    self.assertFalse(self.journal.exists())
                    recovered = self.run_step("resume")
                else:
                    recovered = self.run_installed_startup()
                self.assertEqual(recovered.returncode, 0, recovered.stderr)
                self.assert_snapshot()
                self.assertFalse(pending.exists())

    def test_committed_entrypoint_finish_keeps_journal_immutable(self):
        self.seed_pinned_installer()
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        interrupted = self.run_step("resume", interrupt_phase="committed")
        self.assertEqual(interrupted.returncode, 75, interrupted.stderr)
        committed = self.journal.read_bytes()
        self.assertEqual(json.loads(committed)["phase"], "committed")
        self.assertEqual(self.paths["install.sh"].read_bytes(), INSTALLER.read_bytes())
        (self.home / "codex-info.service.active").write_text("0\n")
        recovered = self.run_installed_startup()
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()
        self.assertEqual(self.journal.read_bytes(), committed)
        old_startup = self.run_installed_startup()
        self.assertEqual(old_startup.returncode, 0, old_startup.stderr)
        self.assertEqual(self.journal.read_bytes(), committed)
        self.assert_snapshot()

    def test_foreign_installer_preserves_staged_handoff(self):
        self.seed_pinned_installer()
        first = self.run_step("install", interrupt_phase="legacy_handoff_staged")
        self.assertEqual(first.returncode, 75, first.stderr)
        pending = self.share / ".legacy-recovery-transaction.json"
        staged = pending.read_bytes()
        self.paths["install.sh"].unlink()
        foreign = b"#!/bin/bash\n# foreign installer\n"
        self.paths["install.sh"].write_bytes(foreign)
        self.paths["install.sh"].chmod(0o755)
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 88, recovered.stderr)
        self.assertEqual(pending.read_bytes(), staged)
        self.assertEqual(self.paths["install.sh"].read_bytes(), foreign)
        self.assertFalse(self.journal.exists())
        for name, data in self.sentinels.items():
            self.assertEqual((self.home / name).read_bytes(), data)

    def test_unsettled_canonical_journal_blocks_handoff(self):
        self.seed_pinned_installer()
        first = self.run_step("install", interrupt_phase="legacy_handoff_staged")
        self.assertEqual(first.returncode, 75, first.stderr)
        pending = self.share / ".legacy-recovery-transaction.json"
        staged = pending.read_bytes()
        document = json.loads(staged)
        self.operation = document["operation_id"]
        document["operation_id"] = "foreign-operation"
        foreign = (json.dumps(document) + "\n").encode()
        self.journal.write_bytes(foreign)
        self.journal.chmod(0o600)
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 88, recovered.stderr)
        self.assertEqual(self.journal.read_bytes(), foreign)
        self.assertEqual(pending.read_bytes(), staged)
        self.assertEqual(self.paths["install.sh"].read_bytes(), self.snapshot["install.sh"][0])

    def test_operation_identity_fits_the_generation_quarantine_namespace(self):
        self.seed_pinned_installer()
        first = self.run_step("install", interrupt_phase="prepared")
        self.assertEqual(first.returncode, 75, first.stderr)
        document = json.loads(self.journal.read_text())
        # Exercise the actual Linux directory-entry constraint used by resume,
        # independently of journal parser acceptance or string-length guesses.
        quarantine = self.backups / (document["operation_id"] + "-generation-" + document["new_generation"])
        quarantine.mkdir(mode=0o700)
        quarantine.rmdir()

    def test_cache_copy_has_durable_authority_before_owner_death(self):
        self.seed_pinned_installer()
        first = self.run_step("install", crash_cache=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        self.assertEqual((self.home / "cache-receipt").read_text(), "recovery cache file and directory fsynced\n")
        cache = self.share / ".legacy-recovery-install.sh"
        self.assertEqual(cache.read_bytes(), INSTALLER.read_bytes())
        pending = self.share / ".legacy-recovery-transaction.json"
        self.assertTrue(pending.exists(), "fsynced recovery code has no durable authority")
        self.operation = json.loads(pending.read_text())["operation_id"]
        newer = self.home / "newer-installer.sh"
        newer.write_bytes(INSTALLER.read_bytes() + b"\n# different retry image\n")
        newer.chmod(0o755)
        recovered = self.run_step("resume", reader_source=newer)
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()

    def test_staged_handoff_recreates_missing_cache_with_different_retry_bytes(self):
        self.seed_pinned_installer()
        first = self.run_step("install", interrupt_phase="legacy_handoff_staged")
        self.assertEqual(first.returncode, 75, first.stderr)
        cache = self.share / ".legacy-recovery-install.sh"
        self.assertFalse(cache.exists(), "cache was published before staged authority")
        pending = self.share / ".legacy-recovery-transaction.json"
        self.operation = json.loads(pending.read_text())["operation_id"]
        newer = self.home / "newer-installer.sh"
        newer.write_bytes(INSTALLER.read_bytes() + b"\n# different retry image\n")
        newer.chmod(0o755)
        recovered = self.run_step("resume", reader_source=newer)
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()
        self.assertEqual(cache.read_bytes(), INSTALLER.read_bytes())

    def test_foreign_staged_code_image_preserves_snapshot_and_authority(self):
        self.seed_pinned_installer()
        first = self.run_step("install", interrupt_phase="legacy_handoff_staged")
        self.assertEqual(first.returncode, 75, first.stderr)
        pending = self.share / ".legacy-recovery-transaction.json"
        document = json.loads(pending.read_text())
        document["legacy_reader_image"] = base64.b64encode(gzip.compress(b"#!/bin/bash\n# foreign program\n", mtime=0)).decode()
        foreign = (json.dumps(document) + "\n").encode()
        pending.write_bytes(foreign)
        recovered = self.run_step("resume")
        self.assertEqual(recovered.returncode, 88, recovered.stderr)
        self.assertEqual(pending.read_bytes(), foreign)
        self.assertFalse(self.journal.exists())
        self.assertFalse((self.share / ".legacy-recovery-install.sh").exists())
        for name, (data, mode) in self.snapshot.items():
            self.assertEqual(self.paths[name].read_bytes(), data, name)
            self.assertEqual(self.paths[name].stat().st_mode & 0o777, mode, name)
        for name, data in self.sentinels.items():
            self.assertEqual((self.home / name).read_bytes(), data)

    def test_known_prior_cache_can_be_replaced_after_stage_interruption(self):
        self.seed_pinned_installer()
        cache = self.share / ".legacy-recovery-install.sh"
        prior = INSTALLER.read_bytes() + b"\n# prior operation recovery code\n"
        cache.write_bytes(prior)
        cache.chmod(0o755)
        def encode(data):
            return base64.urlsafe_b64encode(data).decode().rstrip("=")
        prior_operation = "prior-operation~lc1~111~" + encode(hashlib.sha256(self.manifest).digest()) + "~" + encode(hashlib.sha256(prior).digest())
        document = {
            "schema": "codex-info-install-transaction-v1", "operation_id": prior_operation,
            "owner_pid": 999999, "owner_starttime": 1000, "boot_id": "literal-boot",
            "phase": "committed", "old_generation": self.old_id, "new_generation": "",
            "desired_state": "running", "updated_at_unix": 100,
        }
        self.journal.write_text(json.dumps(document) + "\n")
        self.journal.chmod(0o600)
        # Suppress capture of the deliberately separate prior history record.
        self.operation = "waiting-for-current-stage"
        first = self.run_step("install", interrupt_phase="legacy_handoff_staged")
        self.assertEqual(first.returncode, 75, first.stderr)
        self.assertEqual(cache.read_bytes(), prior)
        pending = self.share / ".legacy-recovery-transaction.json"
        self.operation = json.loads(pending.read_text())["operation_id"]
        self.assertNotEqual(self.operation, prior_operation)
        newer = self.home / "newer-installer.sh"
        newer.write_bytes(INSTALLER.read_bytes() + b"\n# different retry image\n")
        newer.chmod(0o755)
        recovered = self.run_step("resume", reader_source=newer)
        self.assertEqual(recovered.returncode, 0, recovered.stderr)
        self.assert_snapshot()
        self.assertEqual(cache.read_bytes(), INSTALLER.read_bytes())

    def test_retired_active_combined_resumes_original_runtime(self):
        self.retired_runtime(False)

    def test_retired_generation_combined_resumes_original_runtime(self):
        self.retired_runtime(True)

    def partial_restore(self):
        self.seed()
        first = self.run_step("install", crash_stop=True)
        self.assertEqual(first.returncode, -9, first.stderr)
        for name, path in self.paths.items():
            if name == "install.sh": continue  # Its original copy already protects the recovery entrypoint.
            path.replace(self.backups / (self.operation + "-" + path.name))
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

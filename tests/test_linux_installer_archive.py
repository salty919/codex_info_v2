"""#454: real archive producer and pre-mutation install boundary, with literal fixtures."""

import hashlib
import io
import json
import os
import pathlib
import re
import stat
import subprocess  # nosec B404 # fixed offline Bash fixture; host effects are stubbed.
import tarfile
import tempfile
import unittest

INSTALLER = pathlib.Path(__file__).resolve().parents[1] / "packaging/install_linux_bundle.sh"
TARGET = "x86_64-unknown-linux-gnu"
PAYLOAD = {
    "codex_info": (b"literal UI payload\n", 0o755),
    "codex_info_recorder": (b"literal recorder payload\n", 0o755),
    "codex_info_rest": (b"literal REST payload\n", 0o755),
    "run.sh": (b"#!/bin/sh\n# literal launcher\n", 0o755),
    "install.sh": (b"#!/bin/sh\n# literal installer\n", 0o755),
    "codex-info-recorder.service": (b"[Service]\n# literal recorder\n", 0o644),
    "codex-info-rest.service": (b"[Service]\n# literal REST\n", 0o644),
    "codex-info-update.service": (b"[Service]\n# literal updater\n", 0o644),
    "codex-info-update.timer": (b"[Timer]\nOnActiveSec=5min\n", 0o644),
    "LICENSE": (b"literal license\n", 0o644),
    "COPYRIGHT": (b"literal copyright\n", 0o644),
    "NOTICE.txt": (b"literal notice\n", 0o644),
}

# Inject only at private fixture I/O. No production hook, resource exhaustion,
# process killing, network, host service, or real installation is used.
SHIM = r'''import os,pathlib,sys,tarfile
assert sys.argv[1] == "-"
sys.argv = sys.argv[1:]
code=sys.stdin.read()
original_close=tarfile.TarFile.close
original_replace=os.replace
home=pathlib.Path(os.environ["HOME"])
def close(archive):
    already_closed=archive.closed
    original_close(archive)
    if not already_closed and os.environ.get("SWAP_ARCHIVE") and not (home/"swap-receipt").exists():
        original_replace(os.environ["SWAP_ARCHIVE"],os.environ["FIXTURE_ARCHIVE"])
        (home/"swap-receipt").write_text("archive path replaced after the first archive reader\n")
def replace(source,destination):
    path=pathlib.Path(destination)
    if os.environ.get("FAIL_STAGE_WRITE") == "1" and path.name == "LICENSE" and ".candidate." in str(path):
        (home/"write-receipt").write_text("injected LICENSE stage replacement failure\n")
        raise OSError("literal stage write failure")
    return original_replace(source,destination)
tarfile.TarFile.close=close
os.replace=replace
exec(compile(code,"installer-inline-python","exec"))
'''

MODEL = r'''
trace="$HOME/mutations"
desired_state=stopped
update_stage=
die() { printf '%s\n' "$*" >&2; exit 1; }
safe_blocked() { printf 'SAFE_BLOCKED: %s\n' "$*" >&2; exit 88; }
python3() { /usr/bin/python3 "$HOME/shim.py" "$@"; }
check_glibc_compatibility() { :; }
current_generation() { :; }
new_operation_id() { printf 'literal-operation\n'; }
load_control_state() { :; }
require_user_manager() { :; }
legacy_combined_present() { return 1; }
legacy_flat_present() { return 1; }
capture_runtime_state() { :; }
probe_active() { return 1; }
reserve_rollback_budget() { :; }
retire_legacy_combined() { printf 'retire\n' >> "$trace"; printf 'inactive\n' > "$HOME/service-flags"; }
enforce_desired_state() { printf 'enforce\n' >> "$trace"; }
retire_known_unmanaged() { printf 'retire-unmanaged\n' >> "$trace"; }
backup_legacy_path() { printf 'backup\n' >> "$trace"; }
write_journal() { printf 'journal %s\n' "$1" >> "$trace"; printf '%s\n' "$1" > "$HOME/journal"; }
link_entrypoints() { printf 'link\n' >> "$trace"; }
publish_candidate() { printf 'publish\n' >> "$trace"; cp -- "$1/run.sh" "$HOME/published-run"; }
verify_generation_files() { :; }
atomic_symlink() { printf 'switch\n' >> "$trace"; }
activate_candidate() { printf 'activate\n' >> "$trace"; }
verify_candidate() { :; }
write_control_state() { :; }
verify_nonrunning_terminal() { :; }
prune_obsolete_generations() { :; }
update_log() { :; }
cleanup_for_test() {
    cleanup_candidate_stage
}
trap cleanup_for_test EXIT
if [[ "$STEP" == stage ]]; then
    candidate_stage="$generations_dir/.candidate.literal"
    mkdir -m 700 -- "$candidate_stage"
    if ! declare -F extract_candidate >/dev/null; then
        validate_bundle "$ARCHIVE" "$MANIFEST" "$candidate_stage" "$CHECKSUM" "${CODEX_INFO_RELEASE_ARCHIVE_DIGEST:-}"
    elif [[ "${CODEX_INFO_RELEASE_DIGEST_VERIFIED:-}" == 1 ]]; then
        published_release_identity "$MANIFEST"
        extract_candidate "$ARCHIVE" "$candidate_stage"
    else
        validate_bundle "$ARCHIVE" "$MANIFEST"
        extract_candidate "$ARCHIVE" "$candidate_stage"
    fi
    # Retain the completed literal stage for this test's independent byte oracle.
    candidate_stage=
else
    perform_install
fi
'''


class ArchiveStageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="installer-454-")
        self.addCleanup(self.temp.cleanup)
        self.home = pathlib.Path(self.temp.name)
        self.generations = self.home / ".local/share/codex-info/generations"
        self.generations.mkdir(parents=True, mode=0o700)
        self.archive = self.home / f"codex-info-2.0.0-{TARGET}.tar.gz"
        self.external = self.home / "literal.manifest.json"
        self.checksum = self.home / "literal.sha256"
        self.sentinel = self.generations / "escaped"
        self.sentinel.write_bytes(b"literal external sentinel\n")
        self.sentinel.chmod(0o640)
        (self.home / "shim.py").write_text(SHIM)
        (self.home / "service-flags").write_bytes(b"active enabled\n")
        self.old = self.home / "literal-old-payload"
        self.old.write_bytes(b"literal old payload\n")
        self.old.chmod(0o755)
        self.entrypoint = self.home / "literal-entrypoint"
        self.entrypoint.symlink_to("literal-old-payload")

    @staticmethod
    def write_archive(path, members):
        with tarfile.open(path, "w:gz") as archive:
            for name, (data, mode) in sorted(members.items()):
                member = tarfile.TarInfo(name)
                member.size = len(data)
                member.mode = mode
                member.mtime = 0
                archive.addfile(member, io.BytesIO(data))

    def seed(self, legacy=False):
        payload = dict(PAYLOAD)
        if legacy:
            payload["codex-info.service"] = (b"[Service]\n# trusted legacy combined unit\n", 0o644)
            del payload["codex_info_recorder"]
            del payload["codex_info_rest"]
        document = {
            "schema": "codex-info-linux-bundle-v1", "product": "codex_info", "version": "2.0.0",
            "source_sha": "2" * 40, "run_id": "100", "run_attempt": 1,
            "target": TARGET, "compatibility": "glibc", "glibc_minimum": "2.31",
            "files": [{"path": name, "size": len(data), "sha256": hashlib.sha256(data).hexdigest(), "mode": mode}
                      for name, (data, mode) in sorted(payload.items())],
        }
        self.raw_manifest = json.dumps(document, sort_keys=True).encode() + b"\n"
        payload["manifest.json"] = (self.raw_manifest, 0o644)
        sums = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n" for name, (data, _) in sorted(payload.items())).encode()
        payload["SHA256SUMS"] = (sums, 0o644)
        self.expected = payload
        self.write_archive(self.archive, payload)
        self.digest = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        self.external.write_bytes(self.raw_manifest)
        self.checksum.write_text(f"{self.digest}  {self.archive.name}\n")

    def run_source(self, *, step="stage", release=False, swap=None, write_failure=False):
        source = INSTALLER.read_text()
        prefix = source.split("\nusage() {", 1)[0]
        functions = []
        for name in ("validate_bundle", "extract_candidate", "published_release_identity",
                     "perform_install", "cleanup_candidate_stage"):
            functions.extend(re.findall(rf"(?ms)^{name}\(\) \{{\n.*?^\}}\n", source))
        script = prefix + "\n" + "\n".join(functions) + MODEL
        env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(self.home), "STEP": step,
               "FIXTURE_ARCHIVE": str(self.archive), "FAIL_STAGE_WRITE": str(int(write_failure)),
               "CODEX_INFO_RELEASE_DIGEST_VERIFIED": "1" if release else "",
               "CODEX_INFO_RELEASE_ARCHIVE_DIGEST": "sha256:" + self.digest if release else ""}
        if swap:
            env["SWAP_ARCHIVE"] = str(swap)
        # The literal fixture overrides only candidate input paths after production
        # defaults; all archive processing and perform_install ordering are real.
        assignment = f"ARCHIVE={str(self.archive)!r}\nMANIFEST={str(self.external)!r}\nCHECKSUM={str(self.checksum)!r}\n"
        assignment += "VALIDATE_TIMEOUT=5\noperation_deadline=0\nQUIET=1\n"
        script = script.replace(MODEL, assignment + MODEL)
        return subprocess.run(  # nosec B603 # fixed Bash/env, owned temporary paths and service stubs.
            ["/bin/bash", "--noprofile", "--norc", "-s"], input=script, env=env,
            capture_output=True, text=True, check=False, timeout=8,
        )

    def assert_predecessor(self):
        self.assertEqual(self.old.read_bytes(), b"literal old payload\n")
        self.assertEqual(stat.S_IMODE(self.old.stat().st_mode), 0o755)
        self.assertTrue(self.entrypoint.is_symlink())
        self.assertEqual(os.readlink(self.entrypoint), "literal-old-payload")
        self.assertEqual((self.home / "service-flags").read_bytes(), b"active enabled\n")
        self.assertEqual(self.sentinel.read_bytes(), b"literal external sentinel\n")
        self.assertEqual(stat.S_IMODE(self.sentinel.stat().st_mode), 0o640)
        self.assertFalse((self.home / "mutations").exists(), "predecessor mutation callback reached")
        self.assertFalse((self.home / "journal").exists())
        self.assertFalse((self.home / "published-run").exists())
        self.assertEqual(list(self.generations.iterdir()), [self.sentinel])

    def assert_stage(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        stage = self.generations / ".candidate.literal"
        actual = {path.relative_to(stage).as_posix() for path in stage.rglob("*") if path.is_file()}
        self.assertEqual(actual, set(self.expected))
        for name, (data, mode) in self.expected.items():
            self.assertEqual((stage / name).read_bytes(), data, name)
            self.assertEqual(stat.S_IMODE((stage / name).stat().st_mode), mode, name)
        self.assertEqual(result.stdout.rstrip("\n").split("\t")[:3], ["2.0.0", "2" * 40, hashlib.sha256(self.raw_manifest).hexdigest()])

    def test_correct_canonical_bundle_has_exact_bytes_and_modes(self):
        self.seed()
        self.assert_stage(self.run_source())

    def test_digest_authorized_legacy_bundle_has_exact_bytes_and_modes(self):
        self.seed(legacy=True)
        self.assert_stage(self.run_source(release=True))

    def test_archive_replacement_cannot_escape_stage(self):
        self.seed()
        swapped = self.home / "replacement.tar.gz"
        self.write_archive(swapped, {"../escaped": (b"escaped changed bytes\n", 0o644)})
        result = self.run_source(step="install", swap=swapped)
        self.assertTrue((self.home / "swap-receipt").exists(), result.stderr)
        self.assertNotEqual(result.returncode, 0)
        self.assert_predecessor()

    def test_archive_replacement_cannot_change_validated_payload(self):
        self.seed()
        swapped = self.home / "replacement.tar.gz"
        changed = dict(self.expected)
        changed["run.sh"] = (b"modified payload accepted\n", 0o755)
        self.write_archive(swapped, changed)
        result = self.run_source(step="install", swap=swapped)
        self.assertTrue((self.home / "swap-receipt").exists(), result.stderr)
        self.assertNotEqual(result.returncode, 0)
        self.assert_predecessor()

    def test_release_replacement_is_rejected_before_predecessor_mutation(self):
        self.seed(legacy=True)
        self.write_archive(self.archive, {"../escaped": (b"changed release bytes\n", 0o644)})
        result = self.run_source(step="install", release=True)
        self.assertNotEqual(result.returncode, 0)
        self.assert_predecessor()

    def test_stage_write_failure_preserves_predecessor(self):
        self.seed()
        result = self.run_source(step="install", write_failure=True)
        self.assertTrue((self.home / "write-receipt").exists(), result.stderr)
        self.assertNotEqual(result.returncode, 0)
        self.assert_predecessor()


if __name__ == "__main__":
    unittest.main()

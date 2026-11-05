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
SHIM = r'''import os,pathlib,sys,tarfile,hashlib
sys.argv = sys.argv[1:]
if sys.argv[0] == "-B": sys.argv.pop(0)
assert sys.argv[0] == "-"
code=sys.stdin.read()
original_close=tarfile.TarFile.close
original_replace=os.replace
home=pathlib.Path(os.environ["HOME"])
original_sha256=hashlib.sha256
def trace_sha256(data=b"", *args, **kwargs):
    if os.environ.get("TRACE_STAGE_HASHES") == "1":
        manifest=(home/"literal.manifest.json").read_bytes()
        kind="manifest-identity" if data == manifest else "content"
        with (home/"stage-hash-trace").open("a") as output:
            output.write(kind+"\n")
    return original_sha256(data, *args, **kwargs)
hashlib.sha256=trace_sha256
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

CURL_BIN=fixture_curl
fixture_curl() {
    local output=
    while (( $# )); do
        if [[ "$1" == --output ]]; then
            output="$2"
            shift 2
        else
            shift
        fi
    done
    cp -- "$ARCHIVE" "$output"
}
sha256sum() {
    printf 'archive\n' >> "$HOME/download-hash-trace"
    /usr/bin/sha256sum "$@"
}
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
if [[ "$STEP" == stage || "$STEP" == automatic ]]; then
    if [[ "$STEP" == automatic ]]; then
        mkdir -m 700 -- "$HOME/transport"
        transferred="$HOME/transport/${ARCHIVE##*/}"
        download_asset "https://github.com/salty919/codex_info_v2/releases/download/windows-v2.0.0/${ARCHIVE##*/}" "$transferred"
        ARCHIVE="$transferred"
        FIXTURE_ARCHIVE="$transferred"
        export FIXTURE_ARCHIVE
    fi
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

    def seed(self, legacy=False, *, version="2.0.0", run_attempt=1):
        self.archive = self.home / f"codex-info-{version}-{TARGET}.tar.gz"
        payload = dict(PAYLOAD)
        if legacy:
            payload["codex-info.service"] = (b"[Service]\n# trusted legacy combined unit\n", 0o644)
            del payload["codex_info_recorder"]
            del payload["codex_info_rest"]
        document = {
            "schema": "codex-info-linux-bundle-v1", "product": "codex_info", "version": version,
            "source_sha": "2" * 40, "run_id": "100", "run_attempt": run_attempt,
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

    def run_source(self, *, step="stage", release=False, swap=None, write_failure=False, trace_hashes=False):
        source = INSTALLER.read_text()
        prefix = source.split("\nusage() {", 1)[0]
        functions = []
        for name in ("download_asset", "version_python", "validate_bundle", "extract_candidate", "published_release_identity",
                     "manifest_is_local_recovery",
                     "perform_install", "cleanup_candidate_stage",
                     "preserve_control_prestate", "cleanup_control_prestate"):
            functions.extend(re.findall(rf"(?ms)^{name}\(\) \{{\n.*?^\}}\n", source))
        script = prefix + "\n" + "\n".join(functions) + MODEL
        env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(self.home), "STEP": step,
               "FIXTURE_ARCHIVE": str(self.archive), "FAIL_STAGE_WRITE": str(int(write_failure)),
               "TRACE_STAGE_HASHES": str(int(trace_hashes)),
               "CODEX_INFO_RELEASE_DIGEST_VERIFIED": "1" if release else "",
               "CODEX_INFO_RELEASE_ARCHIVE_DIGEST": "sha256:" + self.digest if release else ""}
        if swap:
            env["SWAP_ARCHIVE"] = str(swap)
        # The literal fixture overrides only candidate input paths after production
        # defaults; all archive processing and perform_install ordering are real.
        assignment = f"ARCHIVE={str(self.archive)!r}\nMANIFEST={str(self.external)!r}\nCHECKSUM={str(self.checksum)!r}\n"
        assignment += (f"running_installer_source={str(INSTALLER)!r}\n"
                       "VALIDATE_TIMEOUT=5\noperation_deadline=0\nQUIET=1\n")
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

    def assert_stage(self, result, version="2.0.0"):
        self.assertEqual(result.returncode, 0, result.stderr)
        stage = self.generations / ".candidate.literal"
        actual = {path.relative_to(stage).as_posix() for path in stage.rglob("*") if path.is_file()}
        self.assertEqual(actual, set(self.expected))
        for name, (data, mode) in self.expected.items():
            self.assertEqual((stage / name).read_bytes(), data, name)
            self.assertEqual(stat.S_IMODE((stage / name).stat().st_mode), mode, name)
        self.assertEqual(result.stdout.rstrip("\n").split("\t")[:3], [version, "2" * 40, hashlib.sha256(self.raw_manifest).hexdigest()])

    def assert_beta_stage(self, version, *, run_attempt=1, release=False):
        self.seed(version=version, run_attempt=run_attempt)
        stage = self.generations / ".candidate.literal"
        try:
            result = self.run_source(release=release)
            self.assert_stage(result, version)
            manifest = json.loads((stage / "manifest.json").read_bytes())
            self.assertEqual(manifest["version"], version)
            self.assertEqual(manifest["source_sha"], "2" * 40)
            self.assertEqual(manifest["run_id"], "100")
            self.assertEqual(manifest["run_attempt"], run_attempt)
        finally:
            # Only the private flat literal fixture is removed between subcases.
            if stage.exists():
                for member in stage.iterdir():
                    member.unlink()
                stage.rmdir()
        self.assert_predecessor()

    def test_beta_bundle_has_exact_full_identity(self):
        for release in (False, True):
            with self.subTest(release_digest=release):
                self.assert_beta_stage("1.0.110-beta.7.1", release=release)

    def test_beta_version_boundary_and_canonical_components(self):
        self.assert_beta_stage("1.0.110-beta.12345678901234567.1")
        for version in (
            "1.0.110-beta.123456789012345678.1",
            "1.0.110-beta.0.1",
            "1.0.110-beta.07.1",
            "1.0.110-beta.7.0",
            "1.0.110-beta.7.01",
        ):
            with self.subTest(version=version):
                self.seed(version=version)
                result = self.run_source(step="install")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("candidate staging failed: version", result.stderr)
                self.assert_predecessor()

    def test_beta_attempt_is_manifest_run_attempt(self):
        self.assert_beta_stage("1.0.110-beta.7.2", run_attempt=2)
        self.seed(version="1.0.110-beta.7.2", run_attempt=1)
        result = self.run_source(step="install")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("run attempt", result.stderr)
        self.assert_predecessor()

    def assert_beta_install_is_refused(self, *, release=False):
        self.seed(version="1.0.110-beta.7.1")
        result = self.run_source(step="install", release=release)
        trace_path = self.home / "mutations"
        trace = trace_path.read_text() if trace_path.exists() else ""
        self.assertFalse(trace_path.exists(), "beta install reached mutation callbacks: " + trace)
        self.assert_predecessor()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("beta bundle installation is not supported", result.stderr)

    def test_manual_beta_install_refuses_before_predecessor_mutation(self):
        self.assert_beta_install_is_refused()

    def test_digest_beta_install_refuses_before_predecessor_mutation(self):
        self.assert_beta_install_is_refused(release=True)

    def test_verified_release_staging_does_not_repeat_upstream_content_checks(self):
        # Automatic staging authenticates the private archive snapshot once.
        # It must not repeat upstream member-content verification.
        self.seed()
        result = self.run_source(release=True, trace_hashes=True)
        self.assert_stage(result)
        trace = (self.home / "stage-hash-trace").read_text().splitlines()
        self.assertIn("manifest-identity", trace, "hash observer was not exercised")
        self.assertEqual([kind for kind in trace if kind != "manifest-identity"], ["content"],
                         "automatic staging must authenticate one archive snapshot without repeating member content checks")


    def reorder_valid_fixture(self, *, manifest_order=False, tar_order=False):
        payload = dict(self.expected)
        if manifest_order:
            document = json.loads(self.raw_manifest)
            document["files"].reverse()
            self.raw_manifest = (
                json.dumps(document, sort_keys=True).encode() + b"\n"
            )
            payload["manifest.json"] = (self.raw_manifest, 0o644)
            sums = "".join(
                hashlib.sha256(data).hexdigest() + "  " + name + "\n"
                for name, (data, _) in sorted(payload.items())
                if name != "SHA256SUMS"
            ).encode()
            payload["SHA256SUMS"] = (sums, 0o644)
        self.expected = payload
        ordered = sorted(payload.items(), reverse=tar_order)
        with tarfile.open(self.archive, "w:gz") as archive:
            for name, (data, mode) in ordered:
                member = tarfile.TarInfo(name)
                member.size = len(data)
                member.mode = mode
                member.mtime = 0
                archive.addfile(member, io.BytesIO(data))
        self.digest = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        self.external.write_bytes(self.raw_manifest)
        self.checksum.write_text(self.digest + "  " + self.archive.name + "\n")


    def make_generation_snapshot(self):
        self.seed()
        name = (
            "2.0.0-" + "2" * 40 + "-"
            + hashlib.sha256(self.raw_manifest).hexdigest()
        )
        root = self.generations / name
        root.mkdir(mode=0o700)
        for name, (data, mode) in self.expected.items():
            path = root / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(mode)
        return root

    def invoke_generation_snapshot(self, root, *, prune=False, trace=False):
        source = INSTALLER.read_text()

        def function(name):
            found = re.findall(
                r"(?ms)^" + re.escape(name) + r"\(\) \{\n.*?^\}\n", source
            )
            self.assertEqual(len(found), 1, name)
            return found[0]

        prefix = source.split("\nusage() {", 1)[0]
        prefix += r"""
running_installer_source="$FIXTURE_INSTALLER"
python3() { /usr/bin/python3 "$HOME/shim.py" "$@"; }
"""
        prefix += function("version_python")
        if not prune:
            script = (
                prefix + function("verify_generation_files")
                + '\nverify_generation_files "$FIXTURE_GENERATION"\n'
            )
        else:
            definition = function("prune_obsolete_generations")
            self.assertEqual(definition.count("<<'PY'\n"), 1)
            program = definition.split("<<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
            self.assertEqual(program.count("\ndef read_committed_state("), 1)
            # Run the real validator once, without main's independent
            # current/reference/deletion boundaries or any deletion.
            program = program.split("\ndef read_committed_state(", 1)[0]
            program += r"""
root_fd = os.open(generations_path, directory_flags)
try:
    validate_generation(os.path.basename(current_path), root_fd)
finally:
    os.close(root_fd)
"""
            script = prefix + r"""
version_python "$generations_dir" "$FIXTURE_GENERATION" "$transaction" "$unit_dir" "$proc_root" "" "$SCHEMA" "$PRODUCT" "$TARGET" "$COMPATIBILITY" fixture-operation <<'PRUNE_PROBE'
""" + program + "\nPRUNE_PROBE\n"

        before = {
            name: ((root / name).read_bytes(), stat.S_IMODE((root / name).stat().st_mode))
            for name in self.expected
        }
        trace_file = self.home / "stage-hash-trace"
        trace_file.unlink(missing_ok=True)
        result = subprocess.run(
            ["/bin/bash", "--noprofile", "--norc", "-s"],
            input=script,
            env={
                "PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(self.home),
                "FIXTURE_INSTALLER": str(INSTALLER),
                "FIXTURE_GENERATION": str(root),
                "CODEX_INFO_PROC_ROOT": str(self.home / "proc"),
                "TRACE_STAGE_HASHES": "1" if trace else "0",
                "FAIL_STAGE_WRITE": "0",
            },
            capture_output=True, text=True, check=False, timeout=5,
        )
        after = {
            name: ((root / name).read_bytes(), stat.S_IMODE((root / name).stat().st_mode))
            for name in self.expected
        }
        self.assertEqual(after, before)
        self.preserved_transfer_predecessor()
        self.assertEqual(
            (self.home / "service-flags").read_bytes(), b"active enabled\n"
        )
        hashes = trace_file.read_text().splitlines() if trace_file.exists() else []
        return result, hashes

    def assert_snapshot_hashes_once(self, *, prune):
        root = self.make_generation_snapshot()
        result, hashes = self.invoke_generation_snapshot(root, prune=prune, trace=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        # Literal fixture members are independent of verifier iteration.
        self.assertEqual(
            hashes.count("content"), len(PAYLOAD),
            "one verification snapshot hashes each member once",
        )
        self.assertEqual(
            hashes.count("manifest-identity"), 1,
            "one verification snapshot hashes its manifest once",
        )


    def make_local_generation_snapshot(self, *, receipt_source="2" * 40):
        root = self.make_generation_snapshot()
        document = json.loads(self.raw_manifest)
        receipt = json.dumps({
            "kind": "local-recovery",
            "base_source_sha": receipt_source,
            "source_diff_sha256": "a" * 64,
        }, sort_keys=True).encode() + b"\n"
        receipt_digest = hashlib.sha256(receipt).hexdigest()
        self.expected["LOCAL_RECOVERY.json"] = (receipt, 0o644)
        document["run_id"] = "local-" + receipt_digest
        document["files"].append({
            "path": "LOCAL_RECOVERY.json", "size": len(receipt),
            "sha256": receipt_digest, "mode": 0o644,
        })
        document["files"].sort(key=lambda item: item["path"])
        self.raw_manifest = json.dumps(document, sort_keys=True).encode() + b"\n"
        self.expected["manifest.json"] = (self.raw_manifest, 0o644)
        self.expected["SHA256SUMS"] = (
            "".join(
                hashlib.sha256(data).hexdigest() + "  " + name + "\n"
                for name, (data, _) in sorted(self.expected.items())
                if name != "SHA256SUMS"
            ).encode(), 0o644,
        )
        updated = root.parent / (
            "2.0.0-" + "2" * 40 + "-"
            + hashlib.sha256(self.raw_manifest).hexdigest()
        )
        root.rename(updated)
        for name, (data, mode) in self.expected.items():
            path = updated / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(mode)
        self.external.write_bytes(self.raw_manifest)
        return updated

    def assert_local_snapshot_hashes_once(self, *, prune):
        root = self.make_local_generation_snapshot()
        result, hashes = self.invoke_generation_snapshot(root, prune=prune, trace=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(hashes.count("content"), len(self.expected) - 2)
        self.assertEqual(hashes.count("manifest-identity"), 1)

    def test_local_generation_verification_hashes_each_snapshot_once(self):
        self.assert_local_snapshot_hashes_once(prune=False)

    def test_local_prune_validation_hashes_each_snapshot_once(self):
        self.assert_local_snapshot_hashes_once(prune=True)

    def assert_generation_negative_inputs(self, *, prune):
        root = self.make_generation_snapshot()
        sums = root / "SHA256SUMS"
        original_sums = sums.read_bytes()
        for changed_name in ("codex_info_rest", "manifest.json"):
            with self.subTest(catalog_member=changed_name):
                lines = []
                changed = 0
                for line in original_sums.decode().splitlines():
                    fields = line.split()
                    name = fields[1].removeprefix("*")
                    if name == changed_name:
                        fields[0] = "0" * 64
                        changed += 1
                    lines.append(fields[0] + "  " + name + "\n")
                self.assertEqual(changed, 1)
                sums.write_text("".join(lines))
                result, _ = self.invoke_generation_snapshot(root, prune=prune)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        sums.write_bytes(original_sums)
        member = root / "codex_info_rest"
        original = member.read_bytes()
        altered = bytes([original[0] ^ 1]) + original[1:]
        member.write_bytes(altered)
        result, _ = self.invoke_generation_snapshot(root, prune=prune)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(member.read_bytes(), altered)

    def test_generation_keeps_catalog_and_member_rejection(self):
        self.assert_generation_negative_inputs(prune=False)

    def test_prune_keeps_catalog_and_member_rejection(self):
        self.assert_generation_negative_inputs(prune=True)

    def test_local_receipt_still_requires_exact_provenance(self):
        # Member digest, producer ID and manifest identity all bind to these
        # exact bytes; the independently wrong base source must still reject.
        root = self.make_local_generation_snapshot(receipt_source="3" * 40)
        for prune in (False, True):
            with self.subTest(prune=prune):
                result, _ = self.invoke_generation_snapshot(root, prune=prune)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_generation_verification_hashes_each_snapshot_once(self):
        self.assert_snapshot_hashes_once(prune=False)

    def test_prune_validation_hashes_each_snapshot_once(self):
        self.assert_snapshot_hashes_once(prune=True)

    def test_manual_manifest_order_does_not_reject_valid_identity(self):
        self.seed()
        self.reorder_valid_fixture(manifest_order=True)
        result = self.run_source()
        self.preserved_transfer_predecessor()
        self.assertEqual(
            (self.home / "service-flags").read_bytes(), b"active enabled\n"
        )
        self.assert_stage(result)

    def test_manual_tar_order_does_not_reject_valid_identity(self):
        self.seed()
        self.reorder_valid_fixture(tar_order=True)
        result = self.run_source()
        self.preserved_transfer_predecessor()
        self.assertEqual(
            (self.home / "service-flags").read_bytes(), b"active enabled\n"
        )
        self.assert_stage(result)

    def test_manual_staging_keeps_member_digest_verification(self):
        self.seed()
        changed = dict(self.expected)
        original, mode = changed["run.sh"]
        altered = bytes([original[0] ^ 1]) + original[1:]
        self.assertEqual(len(altered), len(original))
        changed["run.sh"] = (altered, mode)
        self.write_archive(self.archive, changed)
        archive_hash = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        self.checksum.write_text(f"{archive_hash}  {self.archive.name}\n")
        result = self.run_source()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("digest", result.stderr)
        self.assert_predecessor()


    def test_transfer_and_staging_authenticate_one_archive_snapshot(self):
        self.seed()
        result = self.run_source(step="automatic", release=True, trace_hashes=True)
        self.assert_stage(result)
        trace = (self.home / "stage-hash-trace").read_text().splitlines()
        self.assertIn("manifest-identity", trace)
        external = self.home / "download-hash-trace"
        external_count = len(external.read_text().splitlines()) if external.exists() else 0
        archive_count = trace.count("content") + external_count
        self.assertEqual(archive_count, 1, "download and staging repeated archive authentication")
        self.preserved_transfer_predecessor()

    def preserved_transfer_predecessor(self):
        self.assertEqual(self.old.read_bytes(), b"literal old payload\n")
        self.assertEqual(self.entrypoint.readlink(), pathlib.Path("literal-old-payload"))
        self.assertEqual(self.sentinel.read_bytes(), b"literal external sentinel\n")
        self.assertFalse((self.home / "mutations").exists())

    def test_transfer_digest_mismatch_preserves_predecessor(self):
        self.seed()
        self.digest = "0" * 64
        result = self.run_source(step="automatic", release=True)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("archive digest differs", result.stderr)
        self.assert_predecessor()

    def test_transfer_keeps_verified_snapshot_after_original_path_replacement(self):
        self.seed()
        swapped = self.home / "replacement.tar.gz"
        self.write_archive(swapped, {"../escaped": (b"unverified replacement\n", 0o644)})
        result = self.run_source(step="automatic", release=True, swap=swapped)
        self.assertTrue((self.home / "swap-receipt").exists(), result.stderr)
        self.assert_stage(result)
        self.preserved_transfer_predecessor()

    def test_transfer_does_not_require_external_manifest_sidecar(self):
        self.seed()
        self.external.unlink()
        result = self.run_source(step="automatic", release=True)
        self.assert_stage(result)
        self.preserved_transfer_predecessor()

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

"""Direct local Linux bundle beta identity contracts for Issue #467.

Literal version/source/run fields and pre-existing sentinel bytes are the oracle.
The three input executables are inert bytes; objdump is a fixed GLIBC fixture.
No installer, binary execution, Cargo build, service or network is invoked.
"""

import hashlib
import json
import os
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BUILDER = ROOT / "scripts/build_linux_bundle.sh"
TARGET = "x86_64-unknown-linux-gnu"
BETA = "1.0.110-beta.7.1"
STABLE = "1.0.109"
SOURCE = "a" * 40
PAYLOADS = {
    "codex_info": b"literal fixture UI bytes\n",
    "codex_info_recorder": b"literal fixture recorder bytes\n",
    "codex_info_rest": b"literal fixture REST bytes\n",
}


class LinuxBetaBundleFixtures(unittest.TestCase):
    def setUp(self):
        (ROOT / "target").mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(
            prefix="issue467-linux-beta-", dir=ROOT / "target"
        )
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.inputs = self.root / "inputs"
        self.inputs.mkdir()
        for name, contents in PAYLOADS.items():
            path = self.inputs / name
            path.write_bytes(contents)
            path.chmod(0o755)
        self.tools = self.root / "tools"
        self.tools.mkdir()
        objdump = self.tools / "objdump"
        objdump.write_text("#!/bin/sh\nprintf 'literal GLIBC_2.31\\n'\n")
        objdump.chmod(0o755)
        cargo = self.tools / "cargo"
        cargo.write_text(
            '#!/bin/sh\nprintf called > "$CARGO_TRAP"\nexit 97\n'
        )
        cargo.chmod(0o755)
        self.cargo_trap = self.root / "cargo-called"
        self.source_sentinel = self.root / "source-sentinel"
        self.source_sentinel.write_bytes(b"literal source sentinel\n")

    def produce(self, version, output, *, attempt="1", legacy=False):
        args = ["bash", str(BUILDER)]
        if legacy:
            args += ["--binary", str(self.inputs / "codex_info")]
        else:
            args += [
                "--ui-binary", str(self.inputs / "codex_info"),
                "--recorder-binary", str(self.inputs / "codex_info_recorder"),
                "--rest-binary", str(self.inputs / "codex_info_rest"),
            ]
        args += [
            "--version", version,
            "--source-sha", SOURCE,
            "--run-id", "92001",
            "--run-attempt", attempt,
            "--output-dir", str(output),
        ]
        env = os.environ.copy()
        env.update({
            "OBJDUMP_BIN": str(self.tools / "objdump"),
            "PATH": f"{self.tools}:{env['PATH']}",
            "CARGO_TRAP": str(self.cargo_trap),
        })
        result = subprocess.run(
            args, cwd=ROOT, env=env, capture_output=True, text=True, timeout=30,
            check=False,
        )
        self.assertFalse(self.cargo_trap.exists(), "fixture invoked Cargo")
        self.assertEqual(
            self.source_sentinel.read_bytes(), b"literal source sentinel\n"
        )
        for name, contents in PAYLOADS.items():
            self.assertEqual((self.inputs / name).read_bytes(), contents)
        return result

    def assert_identity(self, output, version):
        stem = f"codex-info-{version}-{TARGET}"
        names = {f"{stem}.tar.gz", f"{stem}.tar.gz.sha256", f"{stem}.manifest.json"}
        self.assertEqual({p.name for p in output.iterdir()}, names)
        archive = output / f"{stem}.tar.gz"
        manifest_bytes = (output / f"{stem}.manifest.json").read_bytes()
        manifest = json.loads(manifest_bytes)
        expected = {
            "schema": "codex-info-linux-bundle-v1",
            "product": "codex_info", "version": version,
            "source_sha": SOURCE, "run_id": "92001", "run_attempt": 1,
            "target": TARGET, "compatibility": "glibc", "glibc_minimum": "2.31",
        }
        self.assertEqual(set(manifest), set(expected) | {"files"})
        for key, value in expected.items():
            self.assertEqual(manifest[key], value, key)
        entries = {entry["path"]: entry for entry in manifest["files"]}
        with tarfile.open(archive, "r:gz") as stream:
            self.assertEqual(stream.extractfile("manifest.json").read(), manifest_bytes)
            for name, contents in PAYLOADS.items():
                self.assertEqual(stream.extractfile(name).read(), contents)
                self.assertEqual(stream.getmember(name).mode, 0o755)
                self.assertEqual(entries[name], {
                    "path": name, "size": len(contents),
                    "sha256": hashlib.sha256(contents).hexdigest(), "mode": 0o755,
                })
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(
            (output / f"{stem}.tar.gz.sha256").read_text(),
            f"{checksum}  {stem}.tar.gz\n",
        )

    def test_beta_manifest_archive_keep_literal_full_identity(self):
        output = self.root / "beta"
        result = self.produce(BETA, output)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_identity(output, "1.0.110-beta.7.1")

    def test_stable_manifest_archive_compatibility(self):
        for legacy in (False, True):
            with self.subTest(legacy=legacy):
                output = self.root / f"stable-{legacy}"
                result = self.produce(STABLE, output, legacy=legacy)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assert_identity(output, "1.0.109")

    def test_noncanonical_or_overlong_beta_rejects_before_output(self):
        invalid = (
            "1.0.110-beta.0.1", "1.0.110-beta.07.1", "1.0.110-beta.7.01",
            "01.0.110-beta.7.1", "1.0.110-beta.7.1+abc",
            "1.0.110-beta.12345678901234567890.1",
        )
        for index, version in enumerate(invalid):
            with self.subTest(version=version):
                output = self.root / f"invalid-{index}"
                output.mkdir()
                sentinel = output / "preserved"
                sentinel.write_bytes(b"literal output sentinel\n")
                result = self.produce(version, output)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertEqual({p.name for p in output.iterdir()}, {"preserved"})
                self.assertEqual(sentinel.read_bytes(), b"literal output sentinel\n")

    def test_beta_attempt_mismatch_rejects_before_output(self):
        control = self.root / "positive"
        result = self.produce(BETA, control)
        self.assertEqual(result.returncode, 0, result.stderr)
        output = self.root / "mismatch"
        result = self.produce(BETA, output, attempt="2")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")
        self.assertFalse(output.exists())

    def test_existing_beta_outputs_are_never_replaced(self):
        control = self.root / "positive"
        result = self.produce(BETA, control)
        self.assertEqual(result.returncode, 0, result.stderr)
        stem = "codex-info-1.0.110-beta.7.1-x86_64-unknown-linux-gnu"
        archive, checksum, manifest = (
            f"{stem}.tar.gz", f"{stem}.tar.gz.sha256", f"{stem}.manifest.json"
        )
        cases = ((archive, checksum, manifest), (archive,), (checksum,), (manifest,))
        for index, existing in enumerate(cases):
            with self.subTest(existing=existing):
                output = self.root / f"existing-{index}"
                output.mkdir()
                for name in existing:
                    (output / name).write_bytes(f"literal original {name}\n".encode())
                before = {p.name: p.read_bytes() for p in output.iterdir()}
                result = self.produce(BETA, output)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertEqual(
                    {p.name: p.read_bytes() for p in output.iterdir()}, before
                )


if __name__ == "__main__":
    unittest.main()

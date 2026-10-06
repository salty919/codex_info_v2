"""Finite offline actual Linux release selector contracts for Issue #467."""

import copy
import json
import os
import stat
import subprocess  # nosec B404 # Fixed Bash and inert private fixtures only.
import tarfile
import tempfile
import unittest
from pathlib import Path

from test_linux_beta_bundle import LinuxBetaBundleFixtures

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "packaging/install_linux_bundle.sh"
VERSION_MODULE = ROOT / "scripts/product_version.py"
TARGET = "x86_64-unknown-linux-gnu"
DIGEST = "sha256:" + "c" * 64
SHARED_MARKER = "fixture shared comparison called"


def release(version, *, draft=False, prerelease=None):
    if prerelease is None:
        prerelease = "-beta." in version
    archive = f"codex-info-{version}-{TARGET}.tar.gz"
    tag = f"windows-v{version}"
    return {
        "tag_name": tag, "draft": draft, "prerelease": prerelease,
        "published_at": "2026-09-01T00:00:00Z",
        "assets": [{
            "name": archive, "digest": DIGEST, "state": "uploaded", "size": 123,
            "browser_download_url": (
                "https://github.com/salty919/codex_info_v2/releases/download/"
                f"{tag}/{archive}"
            ),
        }],
    }


def update_output(version):
    return (
        f"update\t{version}\n"
        f"codex-info-{version}-{TARGET}.tar.gz\t"
        "https://github.com/salty919/codex_info_v2/releases/download/"
        f"windows-v{version}/codex-info-{version}-{TARGET}.tar.gz\t{DIGEST}\n"
    )


class LinuxReleaseSelectionFixtures(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="issue467-selector-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.home.mkdir()
        self.generation = self.root / "generation"
        self.generation.mkdir()
        self.installer = self.generation / "install.sh"
        self.installer.write_bytes(INSTALLER.read_bytes())
        self.installer.chmod(0o755)
        self.module = self.generation / "product_version.py"
        self.module.write_bytes(VERSION_MODULE.read_bytes())
        self.module.chmod(0o644)
        for name, mode in (("settings.json", 0o600), ("history.sqlite3", 0o640)):
            sentinel = self.home / name
            sentinel.write_bytes(f"inert private {name} sentinel\n".encode())
            sentinel.chmod(mode)
        self.metadata = self.root / "releases.json"
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.trap = self.root / "external-called"
        for command in ("curl", "systemctl", "cargo"):
            path = self.tools / command
            path.write_text('#!/bin/sh\nprintf called > "$SELECTION_EXTERNAL_TRAP"\nexit 97\n')
            path.chmod(0o755)
        self.env = {
            "HOME": str(self.home), "PATH": f"{self.tools}:/usr/bin:/bin", "LC_ALL": "C",
            "CURL_BIN": str(self.tools / "curl"),
            "SYSTEMCTL_BIN": str(self.tools / "systemctl"),
            "SELECTION_EXTERNAL_TRAP": str(self.trap), "PYTHONDONTWRITEBYTECODE": "1",
        }

    def snapshot(self):
        result = {}
        for path in sorted(self.root.rglob("*")):
            relative = path.relative_to(self.root).as_posix()
            if path.is_symlink():
                result[relative] = ("link", os.readlink(path))
            elif path.is_dir():
                result[relative] = ("directory", stat.S_IMODE(path.stat().st_mode))
            else:
                result[relative] = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
        return result

    def invoke(self, metadata, current, *, channel=None, coherent="1", direct=False):
        self.metadata.write_text(json.dumps(metadata))
        before = self.snapshot()
        if direct:
            source = INSTALLER.read_text()
            function = "select_release() {" + source.split("select_release() {", 1)[1].split("\ndownload_asset() {", 1)[0]
            harness = (
                'set -euo pipefail\nTARGET="$1"\nrunning_installer_source="$2"\n'
                + function + '\nselect_release "$3" "$4" "$5"\n'
            )
            arguments = [
                "/bin/bash", "--noprofile", "--norc", "-c", harness,
                "selector-fixture", TARGET, str(self.installer), str(self.metadata), current, coherent,
            ]
        else:
            arguments = [
                "/bin/bash", "--noprofile", "--norc", str(self.installer),
                "--select-release", "--release-metadata", str(self.metadata),
                "--current-version", current, "--local-coherent", coherent,
            ]
            if channel is not None:
                arguments += ["--channel", channel]
        result = subprocess.run(  # nosec B603 # Fixed Bash argv and bounded offline metadata.
            ["/bin/bash", *arguments[1:]],
            env=self.env, cwd=self.root, capture_output=True, text=True,
            timeout=10, check=False, shell=False,
        )
        self.assertEqual(self.snapshot(), before, "selection changed private fixture bytes/modes/entries")
        self.assertFalse(self.trap.exists(), "selection called an external mutation/network tool")
        return result

    def test_stable_selector_calls_shared_comparison_and_preserves_update_states(self):
        self.module.write_text(self.module.read_text() + '''
_fixture_original_compare = compare_versions
def compare_versions(left, right):
    print("fixture shared comparison called", file=sys.stderr)
    return _fixture_original_compare(left, right)
''')
        cases = (
            ("1.0.109", "1", update_output("1.0.110")),
            ("1.0.110", "1", "no-update\t1.0.110\n"),
            ("1.0.110", "0", update_output("1.0.110")),
            ("1.0.111", "0", "no-update\t1.0.110\n"),
        )
        for current, coherent, expected in cases:
            result = self.invoke(release("1.0.110"), current, coherent=coherent, direct=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, expected)
            self.assertIn(SHARED_MARKER, result.stderr, "actual selector did not use shared comparator")

    def test_unset_and_stable_select_only_stable_candidates(self):
        candidates = [release("1.0.110-beta.99.1"), release("1.0.110"), release("1.0.111", draft=True)]
        for channel in (None, "stable"):
            result = self.invoke(candidates, "1.0.109", channel=channel)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, update_output("1.0.110"))
            self.assertEqual(result.stderr, "")
        for latest in (release("1.0.110-beta.99.1"), release("1.0.110", draft=True)):
            result = self.invoke(latest, "1.0.109", direct=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            self.assertIn("release metadata validation failed", result.stderr)

    def test_explicit_beta_uses_numeric_run_attempt_and_monotonic_selection(self):
        candidates = [
            release("1.0.110-beta.9.1"), release("1.0.110-beta.10.2"),
            release("1.0.110-beta.10.1"), release("1.0.110"),
            release("1.0.110-beta.11.1", draft=True),
        ]
        candidates[0]["published_at"] = "2099-01-01T00:00:00Z"
        for current, expected in (
            ("1.0.109", update_output("1.0.110-beta.10.2")),
            ("1.0.110-beta.9.1", update_output("1.0.110-beta.10.2")),
            ("1.0.110-beta.10.2", "no-update\t1.0.110-beta.10.2\n"),
            ("1.0.110-beta.11.1", "no-update\t1.0.110-beta.10.2\n"),
        ):
            result = self.invoke(candidates, current, channel="beta")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, expected)
            self.assertEqual(result.stderr, "")

    def test_no_beta_candidate_has_no_fallback_or_implicit_downgrade(self):
        for candidates in ([], [release("1.0.110")], [release("1.0.110-beta.20.1", draft=True)]):
            result = self.invoke(candidates, "1.0.110-beta.10.2", channel="beta")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "no-candidate\tbeta\n")
            self.assertEqual(result.stderr, "")
        result = self.invoke(release("1.0.109"), "1.0.110-beta.10.2", channel="stable")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "no-update\t1.0.109\n")

    def test_malformed_partial_and_ambiguous_metadata_reject_before_output(self):
        valid = release("1.0.110-beta.10.2")
        invalid = [
            release("1.0.110-beta.0.1"), release("01.0.110-beta.10.2"),
            release("12345678901234567890.0.0-beta.1.1"),
            release("1.0.110-beta.10.2", prerelease=False),
        ]
        for key, value in (("draft", "false"), ("assets", []), ("assets", "not an array")):
            candidate = copy.deepcopy(valid)
            candidate[key] = value
            invalid.append(candidate)
        for key, value in (
            ("digest", "sha256:bad"), ("state", "new"),
            ("browser_download_url", "https://example.invalid/archive.tar.gz"),
            ("browser_download_url", valid["assets"][0]["browser_download_url"].replace("windows-v1.0.110", "windows-v1.0.111")),
        ):
            candidate = copy.deepcopy(valid)
            candidate["assets"][0][key] = value
            invalid.append(candidate)
        candidate = copy.deepcopy(valid)
        candidate["assets"].append(copy.deepcopy(candidate["assets"][0]))
        invalid.append(candidate)
        invalid.append([valid, copy.deepcopy(valid)])
        for metadata in invalid:
            result = self.invoke(metadata, "1.0.110-beta.9.1", channel="beta")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            self.assertIn("release metadata validation failed", result.stderr)
        for current in ("1.0.110-beta.0.1", "12345678901234567890.0.0-beta.1.1"):
            result = self.invoke(valid, current, channel="beta")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            self.assertIn("release metadata validation failed", result.stderr)

    def test_bundle_binds_unchanged_module_and_readonly_cli_has_no_side_effects(self):
        helper = LinuxBetaBundleFixtures(methodName="runTest")
        helper.setUp()
        self.addCleanup(helper.doCleanups)
        original = VERSION_MODULE.read_bytes()
        import hashlib
        for version in ("1.0.109", "1.0.110-beta.7.1"):
            output = helper.root / version
            produced = helper.produce(version, output)
            self.assertEqual(produced.returncode, 0, produced.stderr)
            stem = f"codex-info-{version}-{TARGET}"
            manifest = json.loads((output / f"{stem}.manifest.json").read_text())
            with tarfile.open(output / f"{stem}.tar.gz", "r:gz") as archive:
                self.assertIn("product_version.py", archive.getnames(), "bundle did not ship shared comparator authority")
                member = archive.getmember("product_version.py")
                self.assertTrue(member.isfile())
                self.assertEqual(member.mode, 0o644)
                self.assertEqual(archive.extractfile(member).read(), original)
                self.installer.write_bytes(archive.extractfile("install.sh").read())
            entries = {entry["path"]: entry for entry in manifest["files"]}
            self.assertEqual(entries["product_version.py"], {
                "path": "product_version.py", "size": len(original),
                "sha256": hashlib.sha256(original).hexdigest(), "mode": 0o644,
            })
            result = self.invoke(release("1.0.110-beta.10.2"), "1.0.109", channel="beta")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, update_output("1.0.110-beta.10.2"))
            self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()

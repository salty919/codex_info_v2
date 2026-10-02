#!/usr/bin/env python3
"""Finite caller tests for the read-only planned release snapshot."""
from __future__ import annotations

import json
import os
import shutil
import subprocess  # nosec B404 # required tool API; individual execution calls remain reviewed.
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GIT = shutil.which("git")
if GIT is None or not Path(GIT).is_absolute():
    raise RuntimeError("fixture requires an absolute Git executable")


class PlannedReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = Path(self.tmp.name) / "repo"
        self.repo.mkdir()
        for path in ("Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props"):
            target = self.repo / path
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / path, target)
        self.git("init", "-q")
        self.git("config", "user.name", "fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("add", ".")
        self.git("commit", "-qm", "base")
        self.base = self.git("rev-parse", "HEAD").strip()
        (self.repo / "src").mkdir()
        (self.repo / "src/main.rs").write_text("fn main() {}\n")
        self.git("add", ".")
        self.git("commit", "-qm", "product")
        self.source = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        # Only this test's local Git repository and literal fixture commands reach this call.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        return subprocess.check_output(  # nosec B603 # absolute Git and fixed offline fixture argv.
            [GIT, "-C", str(self.repo), *args], text=True, shell=False,
        )

    def call(self, *args, ok=True):
        # Fixed current interpreter/helper and temp Git data; negative exits are asserted below.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        result = subprocess.run(  # nosec B603 # fixed Python/helper, offline fixture argv, no shell.
            [sys.executable, str(ROOT / ".github/scripts/release_preflight.py"), *args],
            cwd=self.repo, text=True, capture_output=True, shell=False, check=False,
            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
        )
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def plan(self):
        return json.loads(self.call("plan", "--source", self.source, "--main", self.base,
                                    "--workflow", self.base).stdout)

    def test_exact_prospective_tree_and_no_ref_mutation(self):
        plan = self.plan()
        self.assertTrue(plan["main_included"])
        original_ref = self.git("rev-parse", "HEAD")
        self.call("apply", "--source", self.source, "--workflow", self.base,
                  "--plan", json.dumps(plan))
        self.assertEqual(self.git("rev-parse", "HEAD"), original_ref)
        self.assertEqual(set(self.git("diff", "--name-only").splitlines()),
                         {"Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props"})
        self.git("add", ".")
        self.assertEqual(self.git("write-tree").strip(), plan["expected_tree"])

    def test_already_prepared_version_is_not_bumped_twice(self):
        plan = self.plan()
        self.call("apply", "--source", self.source, "--workflow", self.base,
                  "--plan", json.dumps(plan))
        self.git("add", ".")
        self.git("commit", "-qm", "prepared")
        self.source = self.git("rev-parse", "HEAD").strip()
        second = self.plan()
        self.assertEqual(second["expected_version"], plan["expected_version"])
        self.assertEqual(second["expected_tree"], plan["expected_tree"])

    def test_plan_mismatch_and_dirty_code_fail_before_execution(self):
        plan = self.plan()
        for key in ("source_sha", "workflow_sha", "expected_tree", "expected_version"):
            bad = dict(plan)
            bad[key] = "0" * 40 if key.endswith("sha") or key.endswith("tree") else "99.99.99"
            self.call("apply", "--source", self.source, "--workflow", self.base,
                      "--plan", json.dumps(bad), ok=False)
            self.assertEqual(self.git("diff", "--name-only"), "")
        (self.repo / "src/main.rs").write_text("changed\n")
        self.call("apply", "--source", self.source, "--workflow", self.base,
                  "--plan", json.dumps(plan), ok=False)
        self.assertEqual(self.git("diff", "--name-only").splitlines(), ["src/main.rs"])

    def test_main_advancement_changes_plan_and_nonancestor_is_not_reusable(self):
        initial = self.plan()
        self.git("checkout", "-q", "--detach", self.base)
        (self.repo / "policy.txt").write_text("advanced\n")
        self.git("add", ".")
        self.git("commit", "-qm", "new main")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.git("checkout", "-q", "--detach", self.source)
        later = self.plan()
        self.assertFalse(later["main_included"])
        self.assertNotEqual(initial["main_base_sha"], later["main_base_sha"])

    def test_workflow_calls_early_checks_without_publication_authority(self):
        import yaml
        def workflow(name):
            return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())
        feat = workflow("feat-integration.yml")
        call = feat["jobs"]["selective-quality"]["with"]
        self.assertIs(call["release_preflight"], True)
        self.assertIs(call["release_candidate"], False)
        self.assertEqual(feat["jobs"]["classify"]["permissions"], {"contents": "read"})
        for file, job, names in (
            ("rust.yml", "native-quality", ["Build native release", "Run public CLI lifecycle acceptance", "Run recorder daemon live acceptance"]),
            ("windows-client.yml", "windows-quality", ["Install locked Inno Setup compiler", "Build standard Windows setup wizard", "Upgrade latest published Windows release to the exact candidate", "Run installed Windows UI Automation E2E"]),
            ("linux-ui-quality.yml", "linux-ui-quality", ["Run startup UI image and failure-state acceptance"]),
        ):
            steps = workflow(file)["jobs"][job]["steps"]
            for name in names:
                step = next(s for s in steps if s.get("name") == name)
                self.assertEqual(step["if"], "inputs.release_preflight || inputs.release_candidate")
        for file, job in (("windows-client.yml", "windows-quality"), ("linux-distribution.yml", "linux-distribution")):
            steps = workflow(file)["jobs"][job]["steps"]
            upload = next(s for s in steps if s.get("name") == "Upload release candidate")
            self.assertEqual(upload["if"], "inputs.release_candidate")


if __name__ == "__main__":
    unittest.main()

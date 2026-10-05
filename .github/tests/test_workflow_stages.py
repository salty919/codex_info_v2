#!/usr/bin/env python3
"""Issue #520: execute the feat caller and verify stage-specific job reachability."""

from __future__ import annotations

import ast
import json
import os
import re
import shutil
import subprocess  # nosec B404 # required tool API; individual execution calls remain reviewed.
import sys
import tempfile
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
GIT = shutil.which("git")
BASH = shutil.which("bash")
if any(path is None or not Path(path).is_absolute() for path in (GIT, BASH)):
    raise RuntimeError("offline workflow fixtures require absolute Git and Bash executables")


def workflow(name):
    return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())


def expression(source, inputs):
    """Interpret the finite Actions boolean grammar, independent of the gate helper."""
    if not isinstance(source, str):
        return source
    source = source.removeprefix("${{").removesuffix("}}").strip()
    source = source.replace("&&", " and ").replace("||", " or ")
    source = re.sub(r"!(?!=)", " not ", source).strip()
    # The old optional native proof is absent in this fresh-run fixture.
    source = re.sub(r"steps\.native-proof\.[a-z_.]+", "''", source)
    names = {"inputs": inputs, "true": True, "false": False}

    def value(node):
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, ast.Name):
            return names[node.id]
        if isinstance(node, ast.Attribute):
            return value(node.value)[node.attr]
        if isinstance(node, ast.BoolOp):
            values = [bool(value(item)) for item in node.values]
            return all(values) if isinstance(node.op, ast.And) else any(values)
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            return not value(node.operand)
        if isinstance(node, ast.Compare) and len(node.ops) == 1:
            left, right = value(node.left), value(node.comparators[0])
            if isinstance(node.ops[0], ast.Eq):
                return left == right
            if isinstance(node.ops[0], ast.NotEq):
                return left != right
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
            functions = {
                "fromJSON": json.loads,
                "toJSON": lambda item: json.dumps(item, separators=(",", ":")),
                "contains": lambda collection, item: item in collection,
                "always": lambda: True,
            }
            return functions[node.func.id](*(value(arg) for arg in node.args))
        raise AssertionError("unsupported workflow expression: " + source)

    return value(ast.parse(source, mode="eval").body)


class FeatCallerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="ci-stage-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        # Include the old planner so RED reaches the incorrect selection rather
        # than failing because the fixture omitted an existing dependency.
        for name in ("Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props",
                     "scripts/product_version.py", "scripts/ci_change_scope.py",
                     ".github/scripts/release_preflight.py"):
            source = ROOT / name
            if source.exists():
                target = self.repo / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
        self.git("init", "-q")
        self.git("config", "user.name", "Stage fixture")
        self.git("config", "user.email", "stage@example.invalid")
        self.write("docs/original.md", "fixed rename fixture\n")
        self.main = self.commit("main")
        self.write("windows-client/src/pending.cs", "pending other PR\n")
        self.base = self.commit("pending feat change")
        self.git("remote", "add", "origin", self.repo.as_uri())
        self.bin = self.root / "bin"
        self.bin.mkdir()
        gh = self.bin / "gh"
        gh.write_text("#!" + sys.executable + "\n" +
                      "import os, sys\n"
                      "endpoint = sys.argv[2]\n"
                      "if endpoint.endswith('/heads/main'): print(os.environ['FIXTURE_MAIN'])\n"
                      "elif endpoint.endswith('/heads/feat/next'): print(os.environ['FIXTURE_FEAT'])\n"
                      "else: raise SystemExit('unexpected GitHub request: ' + endpoint)\n")
        gh.chmod(0o755)

    def git(self, *args):
        # Only this test's local Git repository and literal fixture commands reach this call.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        return subprocess.check_output(  # nosec B603 # absolute Git and fixed offline fixture argv.
            [GIT, "-C", str(self.repo), *args], text=True, shell=False,
            stderr=subprocess.DEVNULL,
        ).strip()

    def write(self, name, content):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def commit(self, message):
        self.git("add", ".")
        self.git("commit", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def classify(self, head, **overrides):
        output = self.root / "output"
        output.write_text("")
        env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
               "PYTHONDONTWRITEBYTECODE": "1", "RUNNER_TEMP": str(self.root),
               "GITHUB_OUTPUT": str(output), "HEAD_REPOSITORY": "fixture/repo",
               "REPOSITORY": "fixture/repo", "BASE_SHA": self.base, "HEAD_SHA": head,
               "TRUSTED_SHA": self.main, "FIXTURE_MAIN": self.main, "FIXTURE_FEAT": self.base,
               **overrides}
        steps = workflow("feat-integration.yml")["jobs"]["classify"]["steps"]
        script = next(step["run"] for step in steps if step.get("id") == "classify")
        # Execute this checkout's workflow step with local Git and the finite GitHub stub above.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        result = subprocess.run(  # nosec B603 # trusted checked-in script, offline fixtures, no shell=True.
            [BASH, "-euo", "pipefail", "-c", script], cwd=self.repo,
            env=env, text=True, capture_output=True, shell=False, check=False,
        )
        outputs = dict(line.split("=", 1) for line in output.read_text().splitlines())
        return result, outputs

    def test_docs_pr_does_not_recheck_pending_feat_product_changes(self):
        self.write("docs/new.md", "this PR only\n")
        result, outputs = self.classify(self.commit("docs PR"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(outputs["selection_json"]), {
            "owners": ["DOCS"], "codeql_languages": [],
            "binary_impact": False, "distribution_required": False,
        })

    def test_native_pr_does_not_add_windows_or_distribution(self):
        self.write("src/feature.rs", "pub fn feature() {}\n")
        result, outputs = self.classify(self.commit("native PR"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(outputs["selection_json"]), {
            "owners": ["LINUX_BACKEND"], "codeql_languages": ["rust"],
            "binary_impact": True, "distribution_required": False,
        })

    def test_rename_keeps_both_responsibilities_without_pending_owners(self):
        (self.repo / "src").mkdir()
        self.git("mv", "docs/original.md", "src/renamed.rs")
        result, outputs = self.classify(self.commit("rename PR"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(outputs["selection_json"])["owners"],
                         ["DOCS", "LINUX_BACKEND"])

    def test_stale_base_and_source_without_current_base_remain_rejected(self):
        self.write("docs/new.md", "this PR\n")
        head = self.commit("docs PR")
        for source, overrides in ((head, {"FIXTURE_FEAT": head}), (self.main, {})):
            with self.subTest(source=source, overrides=overrides):
                result, outputs = self.classify(source, **overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("selection_json", outputs)


class WorkflowStageTests(unittest.TestCase):
    def inputs(self, release, owners, languages=(), binary=False):
        return {"release_candidate": release, "release_preflight": not release,
                "preflight_plan": "", "source_sha": "a" * 40,
                "selection_json": json.dumps({"owners": owners,
                    "codeql_languages": languages, "binary_impact": binary,
                    "distribution_required": release and binary})}

    def test_selected_jobs_are_stage_specific(self):
        jobs = workflow("selective-quality.yml")["jobs"]
        cases = (
            (False, ["DOCS"], [], False, {"docs-quality"}),
            (False, ["LINUX_BACKEND"], ["rust"], True, {"linux-backend-quality"}),
            (False, ["WINDOWS"], ["csharp"], True, {"windows-quality"}),
            (False, ["GOVERNANCE"], ["actions", "python"], False, {"governance-quality"}),
            (True, ["DOCS"], [], False, {"docs-quality", "selected-quality"}),
            (True, ["GOVERNANCE"], ["actions", "python"], False,
             {"governance-quality", "codeql-quality", "selected-quality"}),
            (True, ["LINUX_BACKEND", "WINDOWS"], ["rust"], True,
             {"linux-backend-quality", "windows-quality", "codeql-quality",
              "linux-distribution", "selected-quality"}),
        )
        for release, owners, languages, binary, expected in cases:
            with self.subTest(release=release, owners=owners):
                inputs = self.inputs(release, owners, languages, binary)
                actual = {name for name, job in jobs.items()
                          if expression(job.get("if", True), inputs)}
                self.assertEqual(actual, expected)

    def test_release_behavior_and_candidates_execute_only_at_main(self):
        selective = workflow("selective-quality.yml")["jobs"]
        cases = (
            ("linux-backend-quality", "rust.yml", "native-quality",
             {"Run native unit tests with coverage", "Reject native compiler and Clippy warnings",
              "Verify recorder and REST compile-time boundary"},
             {"Build native release", "Run public CLI lifecycle acceptance", "Run recorder daemon live acceptance"}),
            ("windows-quality", "windows-client.yml", "windows-quality",
             {"Run Windows unit tests"}, {"Build standard Windows setup wizard",
              "Upgrade latest published Windows release to the exact candidate",
              "Run installed Windows UI Automation E2E", "Upload release candidate"}),
            ("linux-ui-quality", "linux-ui-quality.yml", "linux-ui-quality",
             {"Build native release for UI evaluation", "Run graph UI image acceptance"},
             {"Run startup UI image and failure-state acceptance"}),
        )
        for release in (False, True):
            for caller, file, job_id, ordinary, final in cases:
                with self.subTest(release=release, file=file):
                    inputs = self.inputs(release, ["LINUX_BACKEND", "LINUX_UI", "WINDOWS"], binary=True)
                    bindings = selective[caller]["with"]
                    leaf = {"release_preflight": False, "preflight_plan": ""}
                    for key in ("release_candidate", "release_preflight", "preflight_plan"):
                        if key in bindings:
                            leaf[key] = expression(bindings[key], inputs)
                    steps = workflow(file)["jobs"][job_id]["steps"]
                    reached = {step.get("name") for step in steps
                               if step.get("name") in ordinary | final
                               and expression(step.get("if", True), leaf)}
                    self.assertTrue(ordinary <= reached, ordinary - reached)
                    self.assertEqual(final & reached, final if release else set())

    def test_main_final_head_is_evaluated_without_a_reuse_dependency(self):
        jobs = workflow("version-prepare.yml")["jobs"]
        self.assertEqual(set(jobs), {"version-prepared", "selective-quality", "acceptance"})
        quality = jobs["selective-quality"]
        self.assertEqual(quality["needs"], ["version-prepared"])
        self.assertEqual(quality["with"]["source_sha"], "${{ needs.version-prepared.outputs.quality_sha }}")
        self.assertIs(quality["with"]["release_candidate"], True)
        self.assertEqual(jobs["acceptance"]["needs"], ["version-prepared", "selective-quality"])

    def test_feat_does_not_project_a_release_or_produce_reuse_evidence(self):
        feat = workflow("feat-integration.yml")
        call = feat["jobs"]["selective-quality"]["with"]
        self.assertEqual(call["base_sha"], "${{ github.event.pull_request.base.sha }}")
        self.assertIs(call["release_candidate"], False)
        for file in ("feat-integration.yml", "selective-quality.yml", "version-prepare.yml",
                     "rust.yml", "windows-client.yml", "linux-ui-quality.yml", "linux-distribution.yml"):
            text = (ROOT / ".github/workflows" / file).read_text()
            for obsolete in ("release_preflight", "preflight_plan", "native_reuse_artifact",
                             "native_quality_proof.py"):
                with self.subTest(file=file, obsolete=obsolete):
                    self.assertNotIn(obsolete, text)


if __name__ == "__main__":
    unittest.main()

# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
"""Offline direct contracts for SEC-DEPENDENCY-AUDIT-01 (Issue #452)."""

import ast
import hashlib
import importlib.util
import json
import pathlib
import re
import tempfile
import unittest
from unittest import mock

import yaml
from defusedxml import ElementTree

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / ".github/scripts/dependency_metadata.py"
SCRATCH = ROOT / "target/dependency-security"


class DependencyMetadataTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        SCRATCH.mkdir(parents=True, exist_ok=True)

    def helper(self):
        self.assertTrue(SCRIPT.is_file(), "dependency metadata helper is unimplemented")
        spec = importlib.util.spec_from_file_location("dependency_metadata", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def workflow(self, name):
        path = ROOT / ".github/workflows" / name
        self.assertTrue(path.is_file(), "dependency workflow is unimplemented")
        return yaml.safe_load(path.read_text())

    def test_inventory_covers_locked_direct_transitive_targets_and_python_suppliers(self):
        module = self.helper()
        inventory = module.inventory(ROOT)
        self.assertEqual(len(inventory["nuget_lockfiles"]), 4)
        for path in (ROOT / "windows-client").rglob("packages.lock.json"):
            for target, entries in json.loads(path.read_text())["dependencies"].items():
                for name, entry in entries.items():
                    if entry["type"] in ("Direct", "Transitive"):
                        package = next(p for p in inventory["packages"] if
                                       p["ecosystem"] == "NuGet" and
                                       p["name"].lower() == name.lower() and
                                       p["version"] == entry["resolved"])
                        self.assertIn({"path": str(path.relative_to(ROOT)),
                                       "target": target, "type": entry["type"]},
                                      package["occurrences"])
        python = {p["name"]: p for p in inventory["packages"] if p["ecosystem"] == "PyPI"}
        self.assertEqual(set(python), {"defusedxml", "PyYAML"})
        self.assertEqual(python["defusedxml"]["version"], "0.7.1")
        self.assertEqual(len(python["defusedxml"]["inline_suppliers"]), 3)
        self.assertEqual(python["PyYAML"]["version"], yaml.__version__)
        self.assertEqual(python["PyYAML"]["supply_status"], "review_required")
        self.assertEqual(inventory["external_python_imports"], ["defusedxml", "yaml"])
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            scratch = pathlib.Path(directory)
            (scratch / ".github/workflows").mkdir(parents=True)
            (scratch / ".github").joinpath("requirements-native-proof.txt").write_text(
                "defusedxml==0.7.1 --hash=sha256:" + "a" * 64 + "\n")
            (scratch / ".github/workflows/x.yml").write_text(
                "jobs:\n  x:\n    steps:\n      - run: defusedxml==0.7.1 --hash=sha256:" + "b" * 64)
            with self.assertRaises(ValueError):
                module.inventory(scratch)

    def test_osv_query_order_and_failure_are_not_success(self):
        module = self.helper()
        queries = [{"package": {"ecosystem": "PyPI", "name": name}, "version": "1.0"}
                   for name in ("one", "two")]
        fetch = mock.Mock(return_value={"results": [{}, {"vulns": [{"id": "GHSA-test"}]}]})
        result = module.audit_osv(queries, fetch)
        self.assertEqual(result[0]["advisories"], [])
        self.assertEqual(result[1]["advisories"], ["GHSA-test"])
        self.assertEqual(fetch.call_args.args[1], {"queries": queries})
        for response in (
            {},
            {"results": [{}]},
            {"results": [
                {},
                {"next_page_token": "more"},  # nosec B105 # Public OSV cursor fixture.
            ]},
        ):
            with self.subTest(response=response), self.assertRaises(ValueError):
                module.audit_osv(queries, mock.Mock(return_value=response))
        with self.assertRaises(OSError):
            module.audit_osv(queries, mock.Mock(side_effect=OSError("offline")))
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            output = pathlib.Path(directory) / "result.json"
            with mock.patch.object(module, "inventory", return_value={}), mock.patch.object(
                module, "collect", side_effect=OSError("offline")
            ):
                self.assertEqual(module.main(["--root", str(ROOT), "--output", str(output)]), 1)
            self.assertEqual(json.loads(output.read_text())["collection_status"], "failed")
            with mock.patch.object(module, "inventory", return_value={}), mock.patch.object(
                module, "collect", return_value={"collection_status": "complete", "vulnerabilities": ["GHSA-test"]}
            ):
                self.assertEqual(module.main(["--root", str(ROOT), "--output", str(output)]), 2)

    def test_license_metadata_missing_is_review_required_not_approved(self):
        module = self.helper()
        empty = module.license_metadata({}, "PyPI", "https://pypi.org/pypi/p/1/json")
        self.assertIsNone(empty["expression"])
        self.assertEqual(empty["approval_status"], "review_required")
        known = module.license_metadata({"licenseExpression": "MIT", "licenseUrl": "https://licenses.nuget.org/MIT"},
                                        "NuGet", "https://api.nuget.org/v3/catalog0/x.json")
        self.assertEqual(known["expression"], "MIT")
        self.assertEqual(known["approval_status"], "review_required")
        self.assertEqual(known["url"], "https://licenses.nuget.org/MIT")
        self.assertEqual(known["source"], "https://api.nuget.org/v3/catalog0/x.json")
        invalid = [
            ("NuGet", {"licenseExpression": []}),
            ("NuGet", {"licenseExpression": "MIT", "licenseUrl": []}),
            ("PyPI", {"classifiers": "License :: MIT"}),
            ("PyPI", {"project_urls": []}),
            ("PyPI", {"project_urls": {"License": []}}),
            ("PyPI", {"license": False}),
            ("GitHub Actions", {"license": "MIT"}),
            ("GitHub Actions", {"license": {"spdx_id": []}}),
        ]
        for ecosystem, metadata in invalid:
            with self.subTest(ecosystem=ecosystem, metadata=metadata), self.assertRaises(TypeError):
                module.license_metadata(metadata, ecosystem, "https://pypi.org/pypi/p/1/json")
        nullable = [
            ("NuGet", {"licenseExpression": None, "licenseUrl": None}),
            ("PyPI", {"license_expression": None, "license": None,
                      "classifiers": None, "project_urls": None}),
            ("GitHub Actions", {"license": None, "html_url": None}),
        ]
        for ecosystem, metadata in nullable:
            result = module.license_metadata(metadata, ecosystem, "https://pypi.org/pypi/p/1/json")
            self.assertIsNone(result["expression"])
            self.assertEqual(result["approval_status"], "review_required")
        with tempfile.TemporaryDirectory(dir=SCRATCH) as directory:
            output = pathlib.Path(directory) / "schema-error.json"
            def invalid_license(*args):
                return module.license_metadata({"licenseExpression": []}, "NuGet", "https://api.nuget.org")
            with mock.patch.object(module, "inventory", return_value={}), mock.patch.object(
                module, "collect", side_effect=invalid_license
            ):
                self.assertEqual(module.main(["--root", str(ROOT), "--output", str(output)]), 1)
            self.assertEqual(json.loads(output.read_text())["collection_status"], "failed")

    def test_actions_exact_sha_and_version_coverage(self):
        module = self.helper()
        sha = "a" * 40
        def fetch(url, payload=None):
            if "/commits/" in url:
                return {"sha": sha}
            if "/tags?" in url:
                return [{"name": "v5", "commit": {"sha": sha}},
                        {"name": "v5.0.1", "commit": {"sha": sha}},
                        {"name": "v5.0.2", "commit": {"sha": "b" * 40}}]
            self.assertIn("/license?ref=" + sha, url)
            return {"license": {"spdx_id": "MIT"}, "html_url": "https://github.com/a/b/blob/" + sha + "/LICENSE"}
        action = module.action_metadata({"name": "a/b", "ref": "v5"}, fetch)
        self.assertEqual(action["resolved_sha"], sha)
        self.assertEqual(action["versions"], ["5.0.1"])
        self.assertFalse(action["immutable"])
        self.assertEqual(action["version_coverage"], "exact_tag_at_resolved_sha")
        def no_version(url, payload=None):
            if "/tags?" in url:
                return [{"name": "v5", "commit": {"sha": sha}}]
            return fetch(url, payload)
        self.assertEqual(module.action_metadata({"name": "a/b", "ref": sha}, no_version)["version_coverage"],
                         "review_required")
        def wrong_sha(url, payload=None):
            return {"sha": "b" * 40}
        with self.assertRaises(ValueError):
            module.action_metadata({"name": "a/b", "ref": sha}, wrong_sha)

    def test_coverage_workflow_uses_verified_v5_action_commits(self):
        job = self.workflow("codacy-coverage.yml")["jobs"]["upload-complete-coverage"]
        refs = [step["uses"] for step in job["steps"] if "uses" in step]
        checkout = "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09"
        download = "actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0"
        self.assertEqual(len(refs), 4)
        for index, expected in enumerate((checkout, download, download, checkout)):
            with self.subTest(step=index):
                self.assertEqual(refs[index], expected)

    def test_metadata_workflow_is_read_only_and_propagates_failure(self):
        data = self.workflow("dependency-metadata.yml")
        events = data.get("on", data.get(True))
        self.assertEqual(events, {"pull_request": {"branches": ["feat/next", "main"]}})
        self.assertEqual(data["permissions"], {"contents": "read"})
        self.assertEqual(len(data["jobs"]), 1)
        job = next(iter(data["jobs"].values()))
        self.assertNotIn("permissions", job)
        for step in job["steps"]:
            self.assertNotIn("continue-on-error", step)
            if "uses" in step:
                self.assertRegex(step["uses"], r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")
        checkout = job["steps"][0]
        self.assertFalse(checkout["with"]["persist-credentials"])
        self.assertEqual(checkout["with"]["ref"], "$" + "{{ github.event.pull_request.head.sha }}")
        audit = next(s for s in job["steps"] if s.get("name") == "Collect and audit dependency metadata")
        self.assertIn("set -euo pipefail", audit["run"])
        self.assertIn("python3 .github/scripts/dependency_metadata.py", audit["run"])
        self.assertNotIn("||", audit["run"])
        self.assertNotIn("pip install", audit["run"])

    def test_windows_restore_declares_audit_and_checks_native_exit(self):
        step = next(s for s in self.workflow("windows-client.yml")["jobs"]["windows-quality"]["steps"]
                    if s.get("name") == "Run Windows unit tests")
        source = step["run"]
        restore = source[:source.index("dotnet format")]
        for value in ("--locked-mode", "--configfile $nugetConfigPath",
                      "NuGetAudit=true", "NuGetAuditMode=all", "NuGetAuditLevel=low"):
            self.assertIn(value, restore)
        self.assertIn("-warnaserror:NU1900,NU1901,NU1902,NU1903,NU1904,NU1905", restore)
        self.assertIn("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }", restore)
        self.assertIn("$nugetConfigPath = Join-Path $env:RUNNER_TEMP", source)
        self.assertIn("[System.IO.File]::WriteAllText(", source)
        self.assertIn("[System.Text.UTF8Encoding]::new($false)", source)
        match = re.search(r"\$nugetConfigXml = @'\n(.*?)\n'@", source, re.DOTALL)
        self.assertIsNotNone(match, "NuGet config must be generated by the Windows owner")
        generated = (match.group(1).replace("\r\n", "\n") + "\n").encode("utf-8")
        self.assertEqual(hashlib.sha256(generated).hexdigest(), "4c7829a5f730968885cf3178a72d748723af7f30659814d7e983a550695a3628")
        self.assertFalse((ROOT / "windows-client/NuGet.Config").exists())
        document = ElementTree.fromstring(generated)
        self.assertEqual(document.tag, "configuration")
        self.assertEqual([node.tag for node in document], ["packageSources", "auditSources"])
        for section in document:
            self.assertEqual([node.tag for node in section], ["clear", "add"])
            self.assertEqual(section[0].attrib, {})
            self.assertEqual(section[1].attrib,
                             {"key": "nuget.org", "value": "https://api.nuget.org/v3/index.json"})

    def test_governance_runs_dependency_contract_tests_once(self):
        tree = ast.parse((ROOT / "scripts/workflow_quality_gate.py").read_text())
        function = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and
                        n.name == "_preflight_caller_tests")
        names = [n.value for n in ast.walk(function) if isinstance(n, ast.Constant) and
                 isinstance(n.value, str) and n.value.startswith("test_") and n.value.endswith(".py")]
        for name in ("test_dependency_security_workflows.py", "test_dependency_metadata.py"):
            self.assertEqual(names.count(name), 1, "missing/duplicated governance caller: " + name)


if __name__ == "__main__":
    unittest.main()

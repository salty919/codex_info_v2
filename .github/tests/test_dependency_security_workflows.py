# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
"""Finite contracts for the read-only dependency workflows (Issue #443)."""

import json
import pathlib
import subprocess  # nosec B404 # Required for the reviewed offline workflow fixture.
import tempfile
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github/workflows"
RUST_PATHS = {
    "Cargo.toml", "Cargo.lock", "**/Cargo.toml", ".cargo/**",
    "rust-toolchain*", "deny.toml", ".github/workflows/rust-dependencies.yml",
}
REVIEW_PATHS = {
    "Cargo.toml", "Cargo.lock", "**/Cargo.toml", "**/*.csproj",
    "**/packages.lock.json", "**/Directory.Packages.props",
    "windows-client/Directory.Build.props", "**/NuGet.Config", "**/nuget.config",
    ".github/workflows/*.yml",
}


class DependencySecurityWorkflowTests(unittest.TestCase):
    def workflow(self, name, paths):
        path = WORKFLOWS / name
        self.assertTrue(path.is_file(), f"unimplemented dependency workflow: {name}")
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
        events = data.get("on", data.get(True))
        self.assertEqual(set(events), {"pull_request"})
        self.assertEqual(set(events["pull_request"]["branches"]), {"feat/next", "main"})
        self.assertEqual(set(events["pull_request"]["paths"]), paths)
        self.assertEqual(data["permissions"], {"contents": "read"})
        self.assertEqual(len(data["jobs"]), 1)
        job = next(iter(data["jobs"].values()))
        self.assertEqual(job["runs-on"], "ubuntu-latest")
        self.assertNotIn("permissions", job)
        self.assertNotIn("continue-on-error", job)
        self.assertNotIn("secrets", job)
        for step in job["steps"]:
            self.assertNotIn("continue-on-error", step)
            if "uses" in step:
                self.assertRegex(step["uses"], r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$")
        return job

    def test_rust_workflow_contract(self):
        job = self.workflow("rust-dependencies.yml", RUST_PATHS)
        checkout = next(s for s in job["steps"] if s.get("uses", "").startswith("actions/checkout@"))
        self.assertEqual(checkout["with"]["ref"], "$" + "{{ github.event.pull_request.head.sha }}")
        self.assertFalse(checkout["with"]["persist-credentials"])
        install = next(s for s in job["steps"] if s.get("uses", "").startswith("taiki-e/install-action@"))
        self.assertRegex(install["with"]["tool"], r"^cargo-deny@\d+\.\d+\.\d+$")
        self.assertEqual(install["with"]["fallback"], "none")
        self.assertEqual(len([s for s in job["steps"] if "run" in s]), 1)

    def test_rust_check_propagates_cargo_status(self):
        job = self.workflow("rust-dependencies.yml", RUST_PATHS)
        script = next(s["run"] for s in job["steps"] if "run" in s)
        scratch = ROOT / "target/dependency-security"
        scratch.mkdir(parents=True, exist_ok=True)
        # Cargo-deny's documented advisory/license/source bits, plus tool failure.
        for status in (0, 1, 4, 8, 42):
            with self.subTest(status=status), tempfile.TemporaryDirectory(dir=scratch) as directory:
                root = pathlib.Path(directory)
                executable = root / "cargo"
                executable.write_text(
                    "#!/usr/bin/python3\n"
                    "import json, os, sys\n"
                    "if sys.argv[1:] == ['deny', '--version']:\n"
                    "    print('cargo-deny 0.20.2')\n"
                    "    sys.exit(0)\n"
                    "with open(os.environ['FAKE_CARGO_LOG'], 'w') as stream:\n"
                    "    json.dump(sys.argv[1:], stream)\n"
                    "sys.exit(int(os.environ['FAKE_CARGO_STATUS']))\n",
                    encoding="utf-8",
                )
                executable.chmod(0o700)
                log = root / "args.json"
                result = subprocess.run(  # nosec B603 # Reviewed Bash; fixture env.
                    ["/bin/bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-s"],
                    input=script,
                    env={
                        "PATH": str(root) + ":/usr/bin:/bin",
                        "LC_ALL": "C",
                        "CARGO_HOME": str(root / "cargo-home"),
                        "FAKE_CARGO_LOG": str(log),
                        "FAKE_CARGO_STATUS": str(status),
                    },
                    capture_output=True,
                    text=True,
                    check=False,
                    timeout=5,
                )
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertEqual(
                    json.loads(log.read_text(encoding="utf-8")),
                    ["deny", "--locked", "check", "advisories", "licenses", "sources"],
                )

    def test_dependency_review_contract(self):
        job = self.workflow("dependency-review.yml", REVIEW_PATHS)
        self.assertEqual(len(job["steps"]), 1)
        review = job["steps"][0]
        self.assertRegex(review["uses"], r"^actions/dependency-review-action@[0-9a-f]{40}$")
        self.assertEqual(
            review["with"],
            {
                "fail-on-severity": "low",
                "fail-on-scopes": "runtime,development,unknown",
                "license-check": "false",
                "vulnerability-check": "true",
                "comment-summary-in-pr": "never",
                "retry-on-snapshot-warnings": "false",
                "show-openssf-scorecard": "false",
                "warn-only": "false",
            },
        )


if __name__ == "__main__":
    unittest.main()

"""Exercise Python diagnostics through the real pre-PR caller."""

import os
import shutil
import subprocess  # noqa: B404  # nosec B404 - fixed shell, literal commands only
import tempfile
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent


class PrePrPythonQualityTests(unittest.TestCase):
    def setUp(self):
        artifacts = ROOT / "artifacts"
        artifacts.mkdir(exist_ok=True)
        directory = tempfile.TemporaryDirectory(dir=artifacts)
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.tool_path = (
            str(ROOT / "artifacts/python-quality-venv/bin")
            + os.pathsep
            + os.environ["PATH"]
        )
        (self.root / "scripts").mkdir()
        (self.root / ".github").mkdir()
        shutil.copy2(
            ROOT / ".github/requirements-python-quality.txt",
            self.root / ".github/requirements-python-quality.txt",
        )
        for name in (
            "pre_pr_gate.sh",
            "quality_plan.py",
            "ci_change_scope.py",
            "python_quality_gate.sh",
            "python_quality.toml",
        ):
            source = ROOT / "scripts" / name
            if source.exists():
                shutil.copy2(source, self.root / "scripts" / name)
        result = self.shell(
            "git init -q\n"
            "git add scripts .github\n"
            "git -c user.name=fixture -c user.email=fixture@example.invalid "
            "-c core.hooksPath=/dev/null commit -qm baseline\n"
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def shell(self, script):
        # The interpreter argv is fixed; stdin is test-owned literal shell code.
        return subprocess.run(  # noqa: B603  # nosec B603
            ["/bin/bash", "--noprofile", "--norc", "-s"],
            input="set -eu\n" + script,
            cwd=self.root,
            env={
                "PATH": self.tool_path,
                "HOME": str(self.root),
                "GIT_CONFIG_NOSYSTEM": "1",
                "PYTHONDONTWRITEBYTECODE": "1",
                "RUNNER_TEMP": str(self.root / "runner"),
            },
            text=True,
            capture_output=True,
            check=False,
            timeout=60,
        )

    def gate(self, source, name="probe.py"):
        (self.root / "scripts" / name).write_text(source)
        return self.shell(
            "bash scripts/pre_pr_gate.sh --requested-check governance-contract\n"
        )

    def test_unused_suppression_is_rejected_before_pr(self):
        result = self.gate("import os  # noqa: S404\n\nprint(os.name)\n")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("RUF100", result.stdout + result.stderr)

    def test_nonexecutable_shebang_is_rejected_before_pr(self):
        result = self.gate("#!/usr/bin/env python3\nprint('fixture')\n")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("EXE001", result.stdout + result.stderr)

    def test_git_mode_cannot_be_hidden_by_local_executable_bit(self):
        probe = self.root / "scripts/probe.py"
        probe.write_text("#!/usr/bin/env python3\nprint('fixture')\n")
        staged = self.shell("git add scripts/probe.py\n")
        self.assertEqual(staged.returncode, 0, staged.stderr)
        probe.chmod(0o755)
        result = self.shell(
            "bash scripts/pre_pr_gate.sh --requested-check governance-contract\n"
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("EXE001", result.stdout + result.stderr)

    def test_import_and_subprocess_contracts_are_checked(self):
        result = self.gate(
            "import sys\nimport subprocess\n\n"
            "subprocess.run(['/bin/echo', sys.version])\n"
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)
        for code in ("I001", "PLW1510"):
            self.assertIn(code, result.stdout + result.stderr)

    def test_valid_python_is_accepted(self):
        result = self.gate("import os\n\nprint(os.name)\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_trusted_caller_without_ruff_can_run_the_gate(self):
        self.tool_path = "/usr/bin:/bin"
        result = self.gate("import os\n\nprint(os.name)\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(
            (self.root / "artifacts/python-quality-venv/bin/ruff").is_file()
        )

    def test_non_python_change_is_accepted(self):
        result = self.gate("exit 0\n", "probe.sh")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_main_workflow_passes_changed_python_to_same_gate(self):
        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/selective-quality.yml").read_text()
        )
        step = next(
            (
                item
                for item in workflow["jobs"]["governance-quality"]["steps"]
                if item.get("name") == "Check changed Python for main"
            ),
            None,
        )
        self.assertIsNotNone(step, "main must invoke the shared Python gate")
        self.assertEqual(step["if"], "inputs.release_candidate")
        self.assertEqual(step["env"]["BASE_SHA"], "${{ inputs.base_sha }}")
        self.assertEqual(step["env"]["SOURCE_SHA"], "${{ inputs.source_sha }}")
        (self.root / "scripts/probe.py").write_text(
            "import os  # noqa: S404\n\nprint(os.name)\n"
        )
        result = self.shell(
            "export BASE_SHA=$(git rev-parse HEAD)\n"
            "git add scripts/probe.py\n"
            "git -c user.name=fixture -c user.email=fixture@example.invalid "
            "-c core.hooksPath=/dev/null commit -qm change\n"
            "export SOURCE_SHA=$(git rev-parse HEAD)\n" + step["run"]
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("RUF100", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

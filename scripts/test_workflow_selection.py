#!/usr/bin/env python3
"""Direct contract for the finite jobs selected by selective-quality.yml."""

from __future__ import annotations

from pathlib import Path
import re
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/selective-quality.yml"
JOB = re.compile(r"^  ([a-z][a-z0-9-]*):\s*$")


def job_blocks() -> dict[str, str]:
    blocks: dict[str, list[str]] = {}
    current: str | None = None
    in_jobs = False
    for line in WORKFLOW.read_text(encoding="utf-8").splitlines():
        if line == "jobs:":
            in_jobs = True
            continue
        if not in_jobs:
            continue
        match = JOB.fullmatch(line)
        if match:
            current = match.group(1)
            blocks[current] = []
        elif current is not None:
            blocks[current].append(line)
    return {name: "\n".join(lines) for name, lines in blocks.items()}


class WorkflowSelectionTests(unittest.TestCase):
    def test_jobs_are_finite_and_each_has_its_direct_selector(self) -> None:
        expected = {
            "docs-quality": ("'DOCS'", "scripts/requirements_ledger_gate.sh"),
            "governance-quality": ("'GOVERNANCE'", "scripts/pre_pr_gate.sh"),
            "linux-backend-quality": ("'LINUX_BACKEND'", "./.github/workflows/rust.yml"),
            "linux-ui-quality": ("'LINUX_UI'", "./.github/workflows/linux-ui-quality.yml"),
            "windows-quality": ("'WINDOWS'", "./.github/workflows/windows-client.yml"),
            "codeql-quality": ("codeql_languages", "./.github/workflows/codeql.yml"),
            "linux-release-quality": ("distribution_required", "./.github/workflows/linux-release-quality.yml"),
        }
        blocks = job_blocks()
        self.assertEqual(set(blocks), set(expected))
        for job, markers in expected.items():
            with self.subTest(job=job):
                self.assertIn("    if:", blocks[job])
                for marker in markers:
                    self.assertIn(marker, blocks[job])

    def test_release_combines_linux_checks_instead_of_running_them_twice(self) -> None:
        blocks = job_blocks()
        for job in ("linux-backend-quality", "linux-ui-quality"):
            self.assertIn("inputs.release_candidate", blocks[job])
            self.assertIn("binary_impact", blocks[job])


if __name__ == "__main__":
    unittest.main()

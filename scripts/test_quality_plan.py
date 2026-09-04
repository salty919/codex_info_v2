#!/usr/bin/env python3
"""Direct fixtures for the bounded quality-plan selector."""

from __future__ import annotations

import contextlib
import io
import json
import unittest

from ci_change_scope import ScopeError
from quality_plan import QualityPlanError, main, plan_for_paths


class QualityPlanFixtures(unittest.TestCase):
    def test_authority_validation_precedes_every_high_cost_owner(self) -> None:
        for path in (
            "docs/spec.md",
            ".github/workflows/check.yml",
            "src/server.rs",
            "ui/app.slint",
            "windows-client/src/CodexInfo.WindowsClient/App.axaml.cs",
        ):
            with self.subTest(path=path):
                self.assertEqual(plan_for_paths((path,)).checks[0], "requirements-authority")

    def test_docs_only_has_no_product_check(self) -> None:
        plan = plan_for_paths(("docs/PRODUCT_REQUIREMENTS.md",))
        self.assertEqual(plan.affected_owners, ("DOCS",))
        self.assertEqual(plan.checks, ("requirements-authority",))

    def test_governance_and_docs_deduplicate_shared_check(self) -> None:
        plan = plan_for_paths(("docs/spec.md", ".github/workflows/check.yml"))
        self.assertEqual(plan.affected_owners, ("DOCS", "GOVERNANCE"))
        self.assertEqual(
            plan.checks, ("requirements-authority", "governance-contract")
        )

    def test_shared_linux_owners_deduplicate_rust_checks(self) -> None:
        plan = plan_for_paths(("Cargo.toml", "ui/app.ts"))
        self.assertEqual(plan.affected_owners, ("LINUX_BACKEND", "LINUX_UI"))
        self.assertEqual(
            plan.checks, ("requirements-authority", "rust-format", "rust-test")
        )

    def test_windows_owner(self) -> None:
        plan = plan_for_paths(
            ("windows-client/src/CodexInfo.WindowsClient/App.axaml.cs",)
        )
        self.assertEqual(plan.affected_owners, ("WINDOWS",))
        self.assertEqual(
            plan.checks, ("requirements-authority", "windows-contract")
        )

    def test_unknown_path_fails_like_classifier(self) -> None:
        with self.assertRaises(QualityPlanError) as raised:
            plan_for_paths(("not-classified.txt",))
        self.assertIn("no CI owner", str(raised.exception))

    def test_empty_path_collection_fails(self) -> None:
        with self.assertRaises(QualityPlanError):
            plan_for_paths(())

    def test_empty_path_fails(self) -> None:
        with self.assertRaises(QualityPlanError):
            plan_for_paths(("",))

    def test_duplicate_requested_check_fails(self) -> None:
        with self.assertRaises(QualityPlanError):
            plan_for_paths(
                ("ui/app.ts",),
                requested_checks=("rust-test", "rust-test"),
            )

    def test_known_but_unplanned_requested_check_fails(self) -> None:
        with self.assertRaises(QualityPlanError):
            plan_for_paths(
                ("docs/spec.md",), requested_checks=("rust-test",)
            )

    def test_unknown_requested_check_fails(self) -> None:
        with self.assertRaises(QualityPlanError):
            plan_for_paths(
                ("docs/spec.md",), requested_checks=("invented-check",)
            )

    def test_exact_requested_set_is_allowed(self) -> None:
        plan = plan_for_paths(
            ("ui/app.ts",),
            requested_checks=(
                "requirements-authority",
                "rust-test",
                "rust-format",
            ),
        )
        self.assertEqual(
            plan.checks, ("requirements-authority", "rust-format", "rust-test")
        )

    def test_cli_emits_stable_json_without_running_commands(self) -> None:
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = main(("docs/spec.md",))
        self.assertEqual(result, 0)
        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(
            json.loads(stdout.getvalue()),
            {
                "affected_owners": ["DOCS"],
                "checks": ["requirements-authority"],
            },
        )

    def test_cli_requested_failure_is_pre_execution_failure(self) -> None:
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = main(
                (
                    "docs/spec.md",
                    "--requested-check",
                    "rust-test",
                )
            )
        self.assertEqual(result, 1)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("quality-plan: FAIL", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()

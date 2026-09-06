#!/usr/bin/env python3
"""Direct tests for owner-based local quality planning."""

from __future__ import annotations

import unittest

from quality_plan import QualityPlanError, plan_for_paths


class QualityPlanTests(unittest.TestCase):
    def test_each_owner_maps_to_its_normal_checks(self) -> None:
        cases = {
            "docs/PRODUCT_REQUIREMENTS.md": (("DOCS",), ("requirements-authority",)),
            ".github/workflows/feat-integration.yml": (
                ("GOVERNANCE",), ("governance-contract",)
            ),
            "src/lib.rs": (
                ("LINUX_BACKEND",),
                ("rust-format", "rust-test"),
            ),
            "ui/app.slint": (
                ("LINUX_UI",),
                ("linux-ui-contract",),
            ),
            "windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml.cs": (
                ("WINDOWS",), ("windows-contract",)
            ),
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                plan = plan_for_paths((path,))
                self.assertEqual((plan.affected_owners, plan.checks), expected)

    def test_shared_checks_are_deduplicated_in_stable_order(self) -> None:
        plan = plan_for_paths(
            ("docs/PRODUCT_REQUIREMENTS.md", "src/main.rs", "windows-client/src/X.cs")
        )
        self.assertEqual(
            plan.checks,
            (
                "requirements-authority",
                "rust-format",
                "rust-test",
                "linux-ui-contract",
                "windows-contract",
            ),
        )

    def test_requested_subset_is_allowed_without_changing_plan(self) -> None:
        plan = plan_for_paths(("src/lib.rs",), requested_checks=("rust-test",))
        self.assertEqual(plan.checks, ("rust-format", "rust-test"))

    def test_duplicate_unknown_and_unrelated_requests_fail(self) -> None:
        cases = (
            ("src/lib.rs", ("rust-test", "rust-test")),
            ("src/lib.rs", ("not-a-check",)),
            ("docs/PRODUCT_REQUIREMENTS.md", ("rust-test",)),
        )
        for path, requested in cases:
            with self.subTest(requested=requested), self.assertRaises(QualityPlanError):
                plan_for_paths((path,), requested_checks=requested)

    def test_empty_and_unknown_paths_fail(self) -> None:
        for paths in ((), ("unknown/file.txt",)):
            with self.subTest(paths=paths), self.assertRaises(QualityPlanError):
                plan_for_paths(paths)


if __name__ == "__main__":
    unittest.main()

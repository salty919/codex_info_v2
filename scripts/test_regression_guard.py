#!/usr/bin/env python3
"""Guard the Rust quality gate against silently skipping workspace crates."""

from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class RegressionGuardContractTests(unittest.TestCase):
    def test_workspace_gate_includes_all_crate_targets(self) -> None:
        script = (ROOT / "scripts" / "regression_guard.sh").read_text(
            encoding="utf-8"
        )
        workspace_command = (
            "cargo test --locked --workspace --all-targets -- --nocapture"
        )
        self.assertEqual(script.count(workspace_command), 1)
        self.assertNotIn("cargo test --locked --all-targets -- --nocapture", script)


if __name__ == "__main__":
    unittest.main()

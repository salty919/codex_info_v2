#!/usr/bin/env python3
"""Guard the Rust quality gate against silently skipping workspace crates."""

from __future__ import annotations

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class RegressionGuardContractTests(unittest.TestCase):
    def test_workspace_gate_matches_native_ci_coverage_command(self) -> None:
        script = (ROOT / "scripts" / "regression_guard.sh").read_text(
            encoding="utf-8"
        )
        workspace_command = (
            "cargo llvm-cov --workspace --locked --all-targets --cobertura"
        )
        self.assertIn(
            "cargo llvm-cov --version 2>/dev/null | "
            "grep -Eq '^cargo-llvm-cov 0\\.9\\.0([[:space:]]|$)'",
            script,
        )
        self.assertIn("cargo-llvm-cov 0.9.0 is required for --test", script)
        self.assertEqual(script.count(workspace_command), 1)
        self.assertIn(
            "--output-path artifacts/codacy-coverage-rust/rust.cobertura.xml",
            script,
        )
        self.assertIn('rm -f -- "$report"', script)
        self.assertNotIn(
            "cargo test --locked --workspace --all-targets -- --nocapture",
            script,
        )


if __name__ == "__main__":
    unittest.main()

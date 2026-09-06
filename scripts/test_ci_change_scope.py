#!/usr/bin/env python3
"""Direct tests for stable path-to-owner CI classification."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from ci_change_scope import (
    ScopeError,
    paths_from_name_status,
    selection_for_paths,
    selection_from_name_status,
)


ROOT = Path(__file__).resolve().parents[1]


class OwnerSelectionTests(unittest.TestCase):
    def test_stable_roots_select_only_their_owners(self) -> None:
        cases = {
            "docs/PRODUCT_REQUIREMENTS.md": (("DOCS",), False, ()),
            ".github/workflows/feat-integration.yml": (
                ("GOVERNANCE",), False, ("actions",)
            ),
            "scripts/ci_change_scope.py": (("GOVERNANCE",), False, ("python",)),
            "scripts/test_future_contract.py": (("GOVERNANCE",), False, ()),
            "AGENTS.md": (("GOVERNANCE",), False, ()),
            "scripts/fake_codex_app_server.py": (("LINUX_BACKEND",), False, ()),
            "scripts/linux_future_probe.sh": (("LINUX_BACKEND",), True, ()),
            "scripts/x11_future_visual_gate.sh": (("LINUX_UI",), False, ()),
            "scripts/windows_future_smoke.ps1": (("WINDOWS",), False, ()),
            "src/usage_store.rs": (("LINUX_BACKEND",), True, ("rust",)),
            "ui/app.slint": (("LINUX_UI",), True, ()),
            "windows-client/src/CodexInfo.WindowsClient.Core/DetailsContracts.cs": (
                ("WINDOWS",), True, ("csharp",)
            ),
            "windows-client/tools/Install-InnoSetup.ps1": (("WINDOWS",), True, ()),
            "windows-client/tools/Test-WindowsInstallerLifecycle.ps1": (("WINDOWS",), False, ()),
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                value = selection_for_paths((path,))
                self.assertEqual(
                    (value.owners, value.binary_impact, value.codeql_languages),
                    expected,
                )
                self.assertEqual(
                    value.powershell_paths,
                    (path,) if path.endswith(".ps1") else (),
                )
                self.assertFalse(value.distribution_required)

    def test_shared_native_entry_selects_backend_and_ui_once(self) -> None:
        value = selection_for_paths(("src/main.rs", "src/main.rs"))
        self.assertEqual(value.owners, ("LINUX_BACKEND", "LINUX_UI"))
        self.assertEqual(value.codeql_languages, ("rust",))

    def test_release_candidate_adds_windows_and_distribution_for_binary(self) -> None:
        value = selection_for_paths(("src/usage_store.rs",), release_candidate=True)
        self.assertEqual(value.owners, ("LINUX_BACKEND", "WINDOWS"))
        self.assertTrue(value.binary_impact)
        self.assertTrue(value.distribution_required)

    def test_release_candidate_does_not_expand_documentation(self) -> None:
        value = selection_for_paths(
            ("docs/PRODUCT_REQUIREMENTS.md",), release_candidate=True
        )
        self.assertEqual(value.owners, ("DOCS",))
        self.assertFalse(value.binary_impact)
        self.assertFalse(value.distribution_required)

    def test_test_sources_do_not_trigger_binary_or_codeql(self) -> None:
        value = selection_for_paths(
            (
                "tests/db_protection_runtime.rs",
                "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/ContractsTests.cs",
            )
        )
        self.assertEqual(value.owners, ("LINUX_BACKEND", "WINDOWS"))
        self.assertFalse(value.binary_impact)
        self.assertEqual(value.codeql_languages, ())

    def test_empty_unknown_and_malformed_paths_fail(self) -> None:
        for paths in (
            (),
            ("new-root/file.txt",),
            ("../src/main.rs",),
            ("/tmp/x",),
            ("scripts/bad\nname.ps1",),
        ):
            with self.subTest(paths=paths), self.assertRaises(ScopeError):
                selection_for_paths(paths)


class NameStatusTests(unittest.TestCase):
    def test_rename_keeps_both_endpoints_and_copy_keeps_only_changed_target(self) -> None:
        raw = (
            b"R100\0docs/old.md\0src/new.rs\0"
            b"C090\0ui/source.slint\0windows-client/src/New.cs\0"
        )
        self.assertEqual(
            paths_from_name_status(raw),
            ("docs/old.md", "src/new.rs", "windows-client/src/New.cs"),
        )
        value = selection_from_name_status(raw)
        self.assertEqual(
            value.owners, ("DOCS", "LINUX_BACKEND", "WINDOWS")
        )

    def test_add_modify_delete_are_parsed(self) -> None:
        self.assertEqual(
            paths_from_name_status(b"A\0docs/a.md\0M\0src/lib.rs\0D\0ui/a.slint\0"),
            ("docs/a.md", "src/lib.rs", "ui/a.slint"),
        )

    def test_truncated_non_utf8_and_unsupported_status_fail(self) -> None:
        for raw in (b"", b"M\0src/lib.rs", b"M\0\xff\0", b"U\0src/lib.rs\0"):
            with self.subTest(raw=raw), self.assertRaises(ScopeError):
                paths_from_name_status(raw)


class CliTests(unittest.TestCase):
    def test_cli_emits_owner_contract_without_feature_profile(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            changes = Path(directory) / "changes.z"
            changes.write_bytes(b"M\0src/lib.rs\0")
            result = subprocess.run(
                ("python3", str(ROOT / "scripts/ci_change_scope.py"), "--name-status", str(changes)),
                cwd=ROOT, text=True, capture_output=True, check=True,
            )
        payload = json.loads(result.stdout)
        self.assertEqual(payload["owners"], ["LINUX_BACKEND"])
        self.assertEqual(payload["powershell_paths"], [])
        self.assertNotIn("quality_profile", payload)

    def test_removed_profile_argument_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            changes = Path(directory) / "changes.z"
            changes.write_bytes(b"M\0src/lib.rs\0")
            result = subprocess.run(
                (
                    "python3", str(ROOT / "scripts/ci_change_scope.py"),
                    "--name-status", str(changes), "--profile-document", str(changes),
                ),
                cwd=ROOT, text=True, capture_output=True,
            )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()

"""Direct tests for stable path-to-owner CI classification."""

from __future__ import annotations

import json

# Only the current interpreter and fixed repository CLI run with owned inputs.
import subprocess  # nosec B404
import sys
import tempfile
import unittest
from pathlib import Path

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
            "scripts/regression_guard.sh": (
                ("GOVERNANCE", "LINUX_BACKEND"), False, ()
            ),
            "scripts/check_recorder_rest_boundary.sh": (
                ("LINUX_BACKEND",), False, ()
            ),
            "src/usage_store.rs": (("LINUX_BACKEND",), True, ("rust",)),
            "crates/codex-info-rest/src/lib.rs": (
                ("LINUX_BACKEND",), True, ("rust",)
            ),
            "ui/app.slint": (("LINUX_BACKEND", "LINUX_UI"), True, ()),
            "assets/codex-info.svg": (("LINUX_UI",), True, ()),
            "windows-client/src/CodexInfo.WindowsClient.Core/DetailsContracts.cs": (
                ("WINDOWS",), True, ("csharp",)
            ),
            "windows-client/tools/Reproduce-WindowsInstalledE2E.ps1": (
                ("WINDOWS",), False, ()
            ),
            "windows-client/tools/Ensure-DotNetSdk.ps1": (
                ("WINDOWS",), False, ()
            ),
            "windows-client/tools/Ensure-InnoSetupCompiler.ps1": (
                ("WINDOWS",), False, ()
            ),
            "windows-client/tools/Install-WindowsCandidateForE2E.ps1": (
                ("WINDOWS",), False, ()
            ),
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                value = selection_for_paths((path,))
                self.assertEqual(
                    (value.owners, value.binary_impact, value.codeql_languages),
                    expected,
                )
                self.assertFalse(value.distribution_required)

    def test_bundle_fixture_uses_existing_governance_prefix(self) -> None:
        for release_candidate in (False, True):
            with self.subTest(release_candidate=release_candidate):
                value = selection_for_paths(
                    ("scripts/test_linux_bundle.sh",),
                    release_candidate=release_candidate,
                )
                self.assertEqual(
                    (value.owners, value.binary_impact,
                     value.distribution_required, value.codeql_languages),
                    (("GOVERNANCE",), False, False, ()),
                )

    def test_crate_integration_tests_keep_backend_without_binary_impact(self) -> None:
        for release_candidate in (False, True):
            with self.subTest(release_candidate=release_candidate):
                value = selection_for_paths(
                    ("crates/codex-info-recorder/tests/app_server_sqlite_isolation.rs",),
                    release_candidate=release_candidate,
                )
                self.assertEqual(
                    (value.owners, value.binary_impact,
                     value.distribution_required, value.codeql_languages),
                    (("LINUX_BACKEND",), False, False, ()),
                )
        nested_source = selection_for_paths(
            ("crates/codex-info-recorder/src/tests/runtime.rs",)
        )
        self.assertEqual(
            (nested_source.owners, nested_source.binary_impact,
             nested_source.codeql_languages),
            (("LINUX_BACKEND",), True, ("rust",)),
        )

    def test_classifier_test_source_does_not_add_codeql(self) -> None:
        for release_candidate in (False, True):
            with self.subTest(release_candidate=release_candidate):
                value = selection_for_paths(
                    ("scripts/test_ci_change_scope.py",),
                    release_candidate=release_candidate,
                )
                self.assertEqual(
                    (value.owners, value.binary_impact,
                     value.distribution_required, value.codeql_languages),
                    (("GOVERNANCE",), False, False, ()),
                )
        mixed = selection_for_paths(
            ("scripts/ci_change_scope.py", "scripts/test_ci_change_scope.py")
        )
        self.assertEqual(mixed.owners, ("GOVERNANCE",))
        self.assertEqual(mixed.codeql_languages, ("python",))
        nested_implementation = selection_for_paths(
            ("scripts/test_tools/helper.py",)
        )
        self.assertEqual(nested_implementation.owners, ("GOVERNANCE",))
        self.assertEqual(nested_implementation.codeql_languages, ("python",))

    def test_shared_native_entry_selects_backend_and_ui_once(self) -> None:
        value = selection_for_paths(("src/main.rs", "src/main.rs"))
        self.assertEqual(value.owners, ("LINUX_BACKEND", "LINUX_UI"))
        self.assertEqual(value.codeql_languages, ("rust",))

    def test_generated_codacy_configs_are_governance_only(self) -> None:
        value = selection_from_name_status(
            b"A\0.codacy/generated/Ruff/ruff.toml\0"
            b"D\0.codacy/generated/Semgrep/opengrep-rules.yaml\0"
        )
        self.assertEqual(value.owners, ("GOVERNANCE",))
        self.assertFalse(value.binary_impact)
        self.assertFalse(value.distribution_required)
        self.assertEqual(value.codeql_languages, ())

    def test_release_candidate_selects_paired_windows_and_linux_candidates(self) -> None:
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
                "tests/release_linux_rollout.rs",
                "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/ContractsTests.cs",
            )
        )
        self.assertEqual(value.owners, ("LINUX_BACKEND", "WINDOWS"))
        self.assertFalse(value.binary_impact)
        self.assertEqual(value.codeql_languages, ())

    def test_empty_unknown_and_malformed_paths_fail(self) -> None:
        # The absolute path is rejection data only; no filesystem access occurs.
        for paths in (
            (),
            ("new-root/file.txt",),
            ("../src/main.rs",),
            ("/tmp/x",),  # noqa: S108  # nosec B108
        ):
            with self.subTest(paths=paths), self.assertRaises(ScopeError):
                selection_for_paths(paths)


class NameStatusTests(unittest.TestCase):
    def test_rename_and_copy_keep_both_endpoints(self) -> None:
        raw = (
            b"R100\0docs/old.md\0src/new.rs\0"
            b"C090\0ui/source.slint\0windows-client/src/New.cs\0"
        )
        self.assertEqual(
            paths_from_name_status(raw),
            ("docs/old.md", "src/new.rs", "ui/source.slint", "windows-client/src/New.cs"),
        )
        value = selection_from_name_status(raw)
        self.assertEqual(
            value.owners, ("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS")
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
            # Current interpreter, fixed repository CLI and owned input; no shell.
            result = subprocess.run(  # noqa: S603  # nosec B603
                (sys.executable, str(ROOT / "scripts/ci_change_scope.py"), "--name-status", str(changes)),
                cwd=ROOT, text=True, capture_output=True, check=True,
            )
        payload = json.loads(result.stdout)
        self.assertEqual(payload["owners"], ["LINUX_BACKEND"])
        self.assertNotIn("quality_profile", payload)

    def test_removed_profile_argument_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            changes = Path(directory) / "changes.z"
            changes.write_bytes(b"M\0src/lib.rs\0")
            # Current interpreter, fixed repository CLI and owned input; no shell.
            result = subprocess.run(  # noqa: S603  # nosec B603
                (
                    sys.executable, str(ROOT / "scripts/ci_change_scope.py"),
                    "--name-status", str(changes), "--profile-document", str(changes),
                ),
                cwd=ROOT, text=True, capture_output=True, check=False,
            )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()

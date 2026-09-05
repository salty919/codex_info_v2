#!/usr/bin/env python3
"""Finite tests for complete-diff owner selection."""

from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

if __package__:
    from .ci_change_scope import (
        ScopeError,
        owners_for_path,
        paths_from_name_status,
        quality_profile_from_document,
        selection_for_paths,
        selection_from_name_status,
    )
else:
    from ci_change_scope import (
        ScopeError,
        owners_for_path,
        paths_from_name_status,
        quality_profile_from_document,
        selection_for_paths,
        selection_from_name_status,
    )


class OwnerTableTests(unittest.TestCase):
    def test_single_and_shared_owners(self) -> None:
        cases = {
            "docs/PRODUCT_REQUIREMENTS.md": {"DOCS"},
            ".github/workflows/feat-integration.yml": {"GOVERNANCE"},
            "scripts/product_version.py": {"GOVERNANCE"},
            "scripts/build_linux_bundle.sh": {"LINUX_BACKEND"},
            "scripts/test_linux_bundle.sh": {"LINUX_BACKEND"},
            "src/server.rs": {"LINUX_BACKEND"},
            "ui/app.slint": {"LINUX_UI"},
            "windows-client/src/App.cs": {"WINDOWS"},
            "Cargo.lock": {"LINUX_BACKEND", "LINUX_UI"},
            "protocol/status.schema.json": {"LINUX_BACKEND", "WINDOWS"},
            "LICENSES/dependency.txt": {"LINUX_BACKEND", "LINUX_UI", "WINDOWS"},
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(owners_for_path(path), frozenset(expected))

    def test_every_tracked_path_has_an_owner(self) -> None:
        raw = subprocess.run(
            ["git", "-c", "core.quotepath=false", "ls-files", "-z"],
            check=True,
            capture_output=True,
        ).stdout
        paths = [path for path in raw.decode("utf-8").split("\0") if path]
        self.assertGreater(len(paths), 0)
        for path in paths:
            with self.subTest(path=path):
                self.assertTrue(owners_for_path(path))

    def test_unknown_or_non_normalized_path_is_rejected(self) -> None:
        for path in ("future/unknown.bin", "../Cargo.toml", "/Cargo.toml", "a//b"):
            with self.subTest(path=path), self.assertRaises(ScopeError):
                owners_for_path(path)


class DiffParserTests(unittest.TestCase):
    def test_add_modify_delete_and_type_change(self) -> None:
        raw = b"A\0README.md\0M\0src/server.rs\0D\0windows-client/Old.cs\0T\0ui/app.slint\0"
        self.assertEqual(
            paths_from_name_status(raw),
            ("README.md", "src/server.rs", "windows-client/Old.cs", "ui/app.slint"),
        )

    def test_rename_and_copy_include_old_and_new_paths(self) -> None:
        raw = (
            b"R100\0src/old.rs\0docs/new.md\0"
            b"C75\0ui/old.slint\0windows-client/src/CodexInfo.WindowsClient/New.cs\0"
        )
        result = selection_from_name_status(raw, release_candidate=True)
        self.assertEqual(
            result.owners,
            ("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"),
        )

    def test_empty_truncated_unknown_and_malformed_records_fail(self) -> None:
        bad = (
            b"",
            b"M\0README.md",
            b"R100\0src/old.rs\0",
            b"U\0README.md\0",
            b"M\0../README.md\0",
            b"M\0future/file.bin\0",
        )
        for raw in bad:
            with self.subTest(raw=raw), self.assertRaises(ScopeError):
                selection_from_name_status(raw)


class SelectionTests(unittest.TestCase):
    def test_docs_has_no_binary_or_codeql(self) -> None:
        result = selection_for_paths(["README.md"])
        self.assertEqual(result.owners, ("DOCS",))
        self.assertEqual(result.codeql_languages, ())
        self.assertFalse(result.binary_impact)
        self.assertFalse(result.distribution_required)
        self.assertEqual(result.quality_profile, "authority-only")

        release_result = selection_for_paths(
            ["README.md"], release_candidate=True
        )
        self.assertEqual(release_result.owners, ("DOCS",))
        self.assertEqual(release_result.codeql_languages, ())
        self.assertFalse(release_result.binary_impact)
        self.assertFalse(release_result.distribution_required)
        self.assertEqual(release_result.quality_profile, "release")

    def test_agents_authority_change_needs_no_profile(self) -> None:
        result = selection_for_paths(["AGENTS.md"])
        self.assertEqual(result.owners, ("DOCS",))
        self.assertEqual(result.codeql_languages, ())
        self.assertFalse(result.binary_impact)
        self.assertFalse(result.distribution_required)
        self.assertEqual(result.quality_profile, "authority-only")

    def test_executable_governance_still_requires_workflow_profile(self) -> None:
        for path in (
            ".github/workflows/selective-quality.yml",
            "scripts/ci_change_scope.py",
            "scripts/regression_guard.sh",
        ):
            with self.subTest(path=path), self.assertRaisesRegex(
                ScopeError, "requires one finite Quality-Profile"
            ):
                selection_for_paths([path])

    def test_feat_product_change_without_finite_profile_stops(self) -> None:
        with self.assertRaisesRegex(ScopeError, "requires one finite Quality-Profile"):
            selection_for_paths(["src/server.rs"])

    def test_history_graph_profile_is_finite_and_skips_distribution(self) -> None:
        result = selection_for_paths(
            [
                "docs/REST_API_V1.md",
                "src/main.rs",
                "src/usage_store.rs",
                "tests/fixtures/graph_delayed_quota.json",
                "windows-client/src/CodexInfo.WindowsClient/Controls/GraphPlotControl.cs",
                "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphPlotProjection.cs",
                "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphScene.cs",
                "windows-client/src/CodexInfo.WindowsClient/ViewModels/DetailsWindowViewModels.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/DetailsWindowViewModelTests.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/WindowDragGeometryTests.cs",
            ],
            quality_profile="history-graph",
        )
        self.assertEqual(
            result.owners, ("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS")
        )
        self.assertEqual(result.codeql_languages, ("csharp", "rust"))
        self.assertTrue(result.binary_impact)
        self.assertFalse(result.distribution_required)
        self.assertEqual(result.quality_profile, "history-graph")

    def test_history_graph_profile_rejects_paths_outside_issue138_manifest(self) -> None:
        for path in (
            "docs/DATA_PROTECTION_POLICY.md",
            "docs/PRODUCT_REQUIREMENTS.md",
            "docs/REQUIREMENTS_LEDGER.md",
            "docs/WINDOWS_UX_SPEC.md",
            "tests/fixtures/graph_cumulative_correction.json",
            "tests/fixtures/graph_weekly_reset_rollover.json",
            "src/server.rs",
        ):
            with self.subTest(path=path), self.assertRaises(ScopeError):
                selection_for_paths(
                    ["src/main.rs", path],
                    quality_profile="history-graph",
                )

    def test_model_history_profile_maps_issue160_complete_diff_to_all_owners(self) -> None:
        result = selection_for_paths(
            [
                "docs/DATA_PROTECTION_POLICY.md",
                "docs/PRODUCT_REQUIREMENTS.md",
                "docs/REQUIREMENTS_LEDGER.md",
                "docs/REST_API_V1.md",
                "docs/WINDOWS_CLIENT.md",
                "docs/WINDOWS_CLIENT_REQUIREMENTS.md",
                "docs/WINDOWS_UX_SPEC.md",
                "src/daemon.rs",
                "src/main.rs",
                "src/server.rs",
                "src/usage_store.rs",
                "ui/app.slint",
                "ui/components.slint",
                "ui/theme.slint",
                "windows-client/src/CodexInfo.WindowsClient.Core/DetailsContracts.cs",
                "windows-client/src/CodexInfo.WindowsClient.Core/LoopbackStatusClient.cs",
                "windows-client/src/CodexInfo.WindowsClient/Controls/GraphPlotControl.cs",
                "windows-client/src/CodexInfo.WindowsClient/GraphWindow.axaml",
                "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphPlotProjection.cs",
                "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphScene.cs",
                "windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml",
                "windows-client/src/CodexInfo.WindowsClient/ViewModels/DetailsWindowViewModels.cs",
                "windows-client/src/CodexInfo.WindowsClient/ViewModels/MainWindowViewModel.cs",
                "windows-client/src/CodexInfo.WindowsClient/ViewModels/ModelUsageViewModel.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/LoopbackBoundaryCoverageTests.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/LoopbackStatusClientTests.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/ContractsTests.cs",
                "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs",
            ],
            quality_profile="model-history",
        )
        self.assertEqual(
            result.owners, ("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS")
        )
        self.assertEqual(result.codeql_languages, ("csharp", "rust"))
        self.assertTrue(result.binary_impact)
        self.assertFalse(result.distribution_required)
        self.assertEqual(result.quality_profile, "model-history")

    def test_model_history_profile_selects_only_changed_product_owners(self) -> None:
        rust = selection_for_paths(
            ["src/usage_store.rs"], quality_profile="model-history"
        )
        self.assertEqual(rust.owners, ("LINUX_BACKEND",))
        self.assertEqual(rust.codeql_languages, ("rust",))
        self.assertTrue(rust.binary_impact)

        windows_test = selection_for_paths(
            [
                "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/ContractsTests.cs"
            ],
            quality_profile="model-history",
        )
        self.assertEqual(windows_test.owners, ("WINDOWS",))
        self.assertEqual(windows_test.codeql_languages, ())
        self.assertFalse(windows_test.binary_impact)
        self.assertFalse(windows_test.distribution_required)

    def test_model_history_profile_has_finite_path_boundaries(self) -> None:
        for paths in (
            ["src/main.rs", "src/account_scope.rs"],
            ["src/server.rs", "tests/fixtures/graph_delayed_quota.json"],
            ["src/server.rs", ".github/workflows/rust.yml"],
        ):
            with self.subTest(paths=paths), self.assertRaises(ScopeError):
                selection_for_paths(paths, quality_profile="model-history")

    def test_release_candidate_linux_selection_adds_windows_without_unchanged_csharp(self) -> None:
        result = selection_for_paths(
            ["src/server.rs"], release_candidate=True
        )
        self.assertEqual(result.owners, ("LINUX_BACKEND", "WINDOWS"))
        self.assertEqual(result.codeql_languages, ("rust",))
        self.assertTrue(result.binary_impact)
        self.assertTrue(result.distribution_required)
        self.assertEqual(result.quality_profile, "release")

    def test_codeql_languages_come_from_changed_source_not_owner_label(self) -> None:
        workflow = selection_for_paths(
            [".github/workflows/feat-integration.yml"],
            quality_profile="workflow-selection",
        )
        python = selection_for_paths(
            ["scripts/ci_change_scope.py"],
            quality_profile="workflow-selection",
        )
        shell = selection_for_paths(
            ["scripts/pre_pr_gate.sh"],
            quality_profile="workflow-selection",
        )
        self.assertEqual(workflow.codeql_languages, ("actions",))
        self.assertEqual(python.codeql_languages, ("python",))
        self.assertEqual(shell.codeql_languages, ())

    def test_profile_on_non_product_diff_is_rejected_as_over_quality(self) -> None:
        with self.assertRaisesRegex(ScopeError, "unnecessary"):
            selection_for_paths(
                ["README.md"], quality_profile="history-graph"
            )

    def test_release_candidate_mixed_selection_is_deterministic(self) -> None:
        paths = ["ui/app.slint", "README.md", "src/server.rs"]
        result = selection_for_paths(paths, release_candidate=True)
        reversed_result = selection_for_paths(
            list(reversed(paths)), release_candidate=True
        )
        self.assertEqual(
            result.owners,
            ("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"),
        )
        self.assertEqual(result.codeql_languages, ("rust",))
        self.assertEqual(result, reversed_result)
        self.assertTrue(result.distribution_required)
        self.assertEqual(result.quality_profile, "release")

    def test_test_only_paths_select_direct_quality_without_binary_or_codeql(self) -> None:
        cases = {
            "tests/db_protection_runtime.rs": ("LINUX_BACKEND",),
            "scripts/x11_graph_visual_gate.sh": ("LINUX_UI",),
            "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/ContractsTests.cs": (
                "WINDOWS",
            ),
        }
        for path, owners in cases.items():
            with self.subTest(path=path):
                result = selection_for_paths([path], release_candidate=True)
                self.assertEqual(result.owners, owners)
                self.assertEqual(result.codeql_languages, ())
                self.assertFalse(result.binary_impact)

    def test_shared_graph_fixture_selects_both_implementations_without_distribution(self) -> None:
        result = selection_for_paths(
            ["tests/fixtures/graph_delayed_quota.json"],
            quality_profile="history-graph",
        )
        self.assertEqual(
            result.owners, ("LINUX_BACKEND", "LINUX_UI", "WINDOWS")
        )
        self.assertEqual(result.codeql_languages, ())
        self.assertFalse(result.binary_impact)
        self.assertFalse(result.distribution_required)
        self.assertEqual(result.quality_profile, "history-graph")

    def test_profile_document_is_exact_single_and_known(self) -> None:
        self.assertEqual(
            quality_profile_from_document(
                "Summary\n\nQuality-Profile: history-graph\n"
            ),
            "history-graph",
        )
        self.assertIsNone(quality_profile_from_document("Summary only\n"))
        self.assertEqual(
            quality_profile_from_document(
                "Quality-Profile: workflow-selection\n"
            ),
            "workflow-selection",
        )
        self.assertEqual(
            quality_profile_from_document("Quality-Profile: model-history\n"),
            "model-history",
        )
        for body in (
            "Quality-Profile: history-graph\nQuality-Profile: history-graph\n",
            "Quality-Profile: full\n",
            "Quality-Profile: history graph\n",
        ):
            with self.subTest(body=body), self.assertRaises(ScopeError):
                quality_profile_from_document(body)

    def test_workflow_profile_owns_only_the_finite_governance_change(self) -> None:
        result = selection_for_paths(
            [
                ".github/workflows/selective-quality.yml",
                "scripts/workflow_quality_gate.py",
                "docs/REQUIREMENTS_LEDGER.md",
            ],
            quality_profile="workflow-selection",
        )
        self.assertEqual(result.owners, ("DOCS", "GOVERNANCE"))
        self.assertEqual(result.quality_profile, "workflow-selection")
        self.assertFalse(result.binary_impact)
        self.assertFalse(result.distribution_required)


class CliTests(unittest.TestCase):
    def test_feat_product_paths_without_profile_fail_before_owner_jobs(self) -> None:
        classifier = Path(__file__).with_name("ci_change_scope.py")
        with tempfile.TemporaryDirectory() as raw_root:
            name_status = Path(raw_root) / "changed-paths.z"
            name_status.write_bytes(
                b"A\0scripts/test_linux_update_convergence.sh\0"
                b"M\0scripts/test_run_launcher_version_sync.sh\0"
            )
            result = subprocess.run(
                [sys.executable, str(classifier), "--name-status", str(name_status)],
                check=False,
                capture_output=True,
                text=True,
            )
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("requires one finite Quality-Profile", result.stderr)

    def test_release_candidate_flag_selects_windows_for_linux_path(self) -> None:
        classifier = Path(__file__).with_name("ci_change_scope.py")
        with tempfile.TemporaryDirectory() as raw_root:
            name_status = Path(raw_root) / "changed-paths.z"
            name_status.write_bytes(b"M\0src/server.rs\0")
            result = subprocess.run(
                [
                    sys.executable,
                    str(classifier),
                    "--name-status",
                    str(name_status),
                    "--release-candidate",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {
                "binary_impact": True,
                "codeql_languages": ["rust"],
                "distribution_required": True,
                "owners": ["LINUX_BACKEND", "WINDOWS"],
                "quality_profile": "release",
            },
        )

    def test_profile_document_drives_the_graph_profile(self) -> None:
        classifier = Path(__file__).with_name("ci_change_scope.py")
        with tempfile.TemporaryDirectory() as raw_root:
            name_status = Path(raw_root) / "changed-paths.z"
            profile = Path(raw_root) / "pr-body.txt"
            name_status.write_bytes(b"M\0src/main.rs\0")
            profile.write_text("Quality-Profile: history-graph\n", encoding="utf-8")
            result = subprocess.run(
                [
                    sys.executable,
                    str(classifier),
                    "--name-status",
                    str(name_status),
                    "--profile-document",
                    str(profile),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["quality_profile"], "history-graph")

    def test_profile_document_accepts_the_issue160_complete_diff(self) -> None:
        classifier = Path(__file__).with_name("ci_change_scope.py")
        paths = (
            "docs/DATA_PROTECTION_POLICY.md",
            "docs/PRODUCT_REQUIREMENTS.md",
            "docs/REQUIREMENTS_LEDGER.md",
            "docs/REST_API_V1.md",
            "docs/WINDOWS_CLIENT.md",
            "docs/WINDOWS_CLIENT_REQUIREMENTS.md",
            "docs/WINDOWS_UX_SPEC.md",
            "src/daemon.rs",
            "src/main.rs",
            "src/server.rs",
            "src/usage_store.rs",
            "ui/app.slint",
            "ui/components.slint",
            "ui/theme.slint",
            "windows-client/src/CodexInfo.WindowsClient.Core/DetailsContracts.cs",
            "windows-client/src/CodexInfo.WindowsClient.Core/LoopbackStatusClient.cs",
            "windows-client/src/CodexInfo.WindowsClient/Controls/GraphPlotControl.cs",
            "windows-client/src/CodexInfo.WindowsClient/GraphWindow.axaml",
            "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphPlotProjection.cs",
            "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphScene.cs",
            "windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml",
            "windows-client/src/CodexInfo.WindowsClient/ViewModels/DetailsWindowViewModels.cs",
            "windows-client/src/CodexInfo.WindowsClient/ViewModels/MainWindowViewModel.cs",
            "windows-client/src/CodexInfo.WindowsClient/ViewModels/ModelUsageViewModel.cs",
            "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/LoopbackBoundaryCoverageTests.cs",
            "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/LoopbackStatusClientTests.cs",
            "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/ContractsTests.cs",
            "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs",
        )
        with tempfile.TemporaryDirectory() as raw_root:
            name_status = Path(raw_root) / "changed-paths.z"
            profile = Path(raw_root) / "pr-body.txt"
            name_status.write_bytes(
                b"".join(b"M\0" + path.encode("utf-8") + b"\0" for path in paths)
            )
            profile.write_text("Quality-Profile: model-history\n", encoding="utf-8")
            result = subprocess.run(
                [
                    sys.executable,
                    str(classifier),
                    "--name-status",
                    str(name_status),
                    "--profile-document",
                    str(profile),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {
                "binary_impact": True,
                "codeql_languages": ["csharp", "rust"],
                "distribution_required": False,
                "owners": ["DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"],
                "quality_profile": "model-history",
            },
        )


if __name__ == "__main__":
    unittest.main()

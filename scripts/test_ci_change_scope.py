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
        selection_for_paths,
        selection_from_name_status,
    )
else:
    from ci_change_scope import (
        ScopeError,
        owners_for_path,
        paths_from_name_status,
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
        raw = b"R100\0src/old.rs\0docs/new.md\0C75\0ui/old.slint\0windows-client/New.cs\0"
        result = selection_from_name_status(raw)
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

        release_result = selection_for_paths(
            ["README.md"], release_candidate=True
        )
        self.assertEqual(release_result.owners, ("DOCS",))
        self.assertEqual(release_result.codeql_languages, ())
        self.assertFalse(release_result.binary_impact)

    def test_default_linux_selection_does_not_select_windows(self) -> None:
        result = selection_for_paths(["src/server.rs"])
        self.assertEqual(result.owners, ("LINUX_BACKEND",))
        self.assertEqual(result.codeql_languages, ("rust",))

    def test_release_candidate_linux_selection_includes_windows_and_csharp(self) -> None:
        result = selection_for_paths(
            ["src/server.rs"], release_candidate=True
        )
        self.assertEqual(result.owners, ("LINUX_BACKEND", "WINDOWS"))
        self.assertEqual(result.codeql_languages, ("csharp", "rust"))
        self.assertTrue(result.binary_impact)

    def test_governance_maps_to_actions_and_python(self) -> None:
        result = selection_for_paths([".github/workflows/codeql.yml"])
        self.assertEqual(result.owners, ("GOVERNANCE",))
        self.assertEqual(result.codeql_languages, ("actions", "python"))

    def test_mixed_selection_is_deduplicated(self) -> None:
        result = selection_for_paths(
            ["README.md", "src/server.rs", "ui/app.slint", "windows-client/src/App.cs"]
        )
        self.assertEqual(
            result.owners,
            ("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"),
        )
        self.assertEqual(result.codeql_languages, ("csharp", "rust"))
        self.assertTrue(result.binary_impact)

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
        self.assertEqual(result.codeql_languages, ("csharp", "rust"))
        self.assertEqual(result, reversed_result)


class CliTests(unittest.TestCase):
    def test_linux_product_scripts_do_not_select_governance_codeql(self) -> None:
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
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(result.stdout),
            {
                "binary_impact": True,
                "codeql_languages": ["rust"],
                "owners": ["LINUX_BACKEND"],
            },
        )

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
                "codeql_languages": ["csharp", "rust"],
                "owners": ["LINUX_BACKEND", "WINDOWS"],
            },
        )


if __name__ == "__main__":
    unittest.main()

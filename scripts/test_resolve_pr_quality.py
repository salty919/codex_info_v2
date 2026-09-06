#!/usr/bin/env python3
"""Focused integration cases for PR owner and version resolution."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
import resolve_pr_quality  # noqa: E402


class ResolverFixture:
    def __init__(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="codex-info-pr-resolver-")
        self.root = Path(self.temporary.name)
        subprocess.run(("git", "init", "-q"), cwd=self.root, check=True)
        subprocess.run(("git", "config", "user.name", "fixture"), cwd=self.root, check=True)
        subprocess.run(("git", "config", "user.email", "fixture@example.invalid"), cwd=self.root, check=True)
        (self.root / "windows-client").mkdir()
        self.write_versions("1.0.9")
        (self.root / "src").mkdir()
        (self.root / "src" / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        (self.root / "docs").mkdir()
        (self.root / "docs" / "note.md").write_text("base\n", encoding="utf-8")
        self.base = self.commit("base")

    def close(self) -> None:
        self.temporary.cleanup()

    def write_versions(self, version: str) -> None:
        (self.root / "Cargo.toml").write_text(
            f'[package]\nname = "codex_info"\nversion = "{version}"\n', encoding="utf-8"
        )
        (self.root / "Cargo.lock").write_text(
            f'version = 4\n\n[[package]]\nname = "codex_info"\nversion = "{version}"\n',
            encoding="utf-8",
        )
        (self.root / "windows-client" / "Directory.Build.props").write_text(
            f"<Project><PropertyGroup><Version>{version}</Version></PropertyGroup></Project>\n",
            encoding="utf-8",
        )

    def commit(self, message: str) -> str:
        subprocess.run(("git", "add", "."), cwd=self.root, check=True)
        subprocess.run(("git", "commit", "-qm", message), cwd=self.root, check=True)
        return subprocess.check_output(("git", "rev-parse", "HEAD"), cwd=self.root, text=True).strip()

    def resolve(self, head: str, *, release: bool):
        old = Path.cwd()
        try:
            import os

            os.chdir(self.root)
            return resolve_pr_quality.resolve(
                base_sha=self.base,
                head_sha=head,
                repository="owner/repository",
                head_repository="owner/repository",
                release_candidate=release,
            )
        finally:
            os.chdir(old)


class ResolverTests(unittest.TestCase):
    def use_fixture(self) -> ResolverFixture:
        fixture = ResolverFixture()
        self.addCleanup(fixture.close)
        return fixture

    def test_feat_classifies_without_requiring_a_version_change(self) -> None:
        fixture = self.use_fixture()
        (fixture.root / "src" / "main.rs").write_text("fn main() { println!(\"x\"); }\n", encoding="utf-8")
        head = fixture.commit("product")
        selection = fixture.resolve(head, release=False)
        self.assertTrue(selection.binary_impact)
        self.assertFalse(selection.distribution_required)

    def test_main_binary_accepts_an_explicit_forward_version(self) -> None:
        for version in ("1.0.10", "1.1.0", "2.0.0"):
            with self.subTest(version=version):
                fixture = self.use_fixture()
                fixture.write_versions(version)
                (fixture.root / "src" / "main.rs").write_text(
                    "fn main() { println!(\"x\"); }\n", encoding="utf-8"
                )
                selection = fixture.resolve(fixture.commit("release"), release=True)
                self.assertTrue(selection.binary_impact)
                self.assertTrue(selection.distribution_required)

    def test_main_binary_rejects_an_unbumped_version(self) -> None:
        fixture = self.use_fixture()
        (fixture.root / "src" / "main.rs").write_text("fn main() { println!(\"x\"); }\n", encoding="utf-8")
        with self.assertRaises(resolve_pr_quality.ResolutionError):
            fixture.resolve(fixture.commit("missing bump"), release=True)

    def test_main_binary_rejects_a_version_decrease(self) -> None:
        fixture = self.use_fixture()
        fixture.write_versions("0.9.99")
        (fixture.root / "src" / "main.rs").write_text(
            "fn main() { println!(\"x\"); }\n", encoding="utf-8"
        )
        with self.assertRaises(resolve_pr_quality.ResolutionError):
            fixture.resolve(fixture.commit("version decrease"), release=True)

    def test_main_document_change_keeps_version_and_skips_distribution(self) -> None:
        fixture = self.use_fixture()
        (fixture.root / "docs" / "note.md").write_text("changed\n", encoding="utf-8")
        selection = fixture.resolve(fixture.commit("documentation"), release=True)
        self.assertFalse(selection.binary_impact)
        self.assertFalse(selection.distribution_required)

if __name__ == "__main__":
    unittest.main()

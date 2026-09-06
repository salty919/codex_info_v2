#!/usr/bin/env python3
"""Finite, dependency-free fixtures for product_version.py."""

from __future__ import annotations

import importlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
SCRIPT = SCRIPT_DIR / "product_version.py"
sys.path.insert(0, str(SCRIPT_DIR))
product_version = importlib.import_module("product_version")


CARGO_TOML = """# fixture Cargo manifest
[package]
name = "codex_info"
version = "{version}"
edition = "2021"

[dependencies]
serde = "1"
"""

CARGO_LOCK = """# fixture lockfile
version = 4

[[package]]
name = "codex_info"
version = "{version}"
dependencies = [
 "serde",
]

[[package]]
name = "serde"
version = "1.0.0"
"""

WINDOWS_PROPS = """<Project>
  <PropertyGroup>
    <Version>{version}</Version>
    <Deterministic>true</Deterministic>
  </PropertyGroup>
</Project>
"""


class VersionFixture:
    def __init__(self, version: str = "1.0.8") -> None:
        self.directory = tempfile.TemporaryDirectory(prefix="codex-info-version-")
        root = Path(self.directory.name)
        self.paths = product_version.VersionPaths(
            cargo_toml=root / "Cargo.toml",
            cargo_lock=root / "Cargo.lock",
            windows_props=root / "windows-client" / "Directory.Build.props",
        )
        self.paths.windows_props.parent.mkdir()
        self.paths.cargo_toml.write_bytes(CARGO_TOML.replace("{version}", version).encode())
        self.paths.cargo_lock.write_bytes(CARGO_LOCK.replace("{version}", version).encode())
        self.paths.windows_props.write_bytes(
            WINDOWS_PROPS.replace("{version}", version).encode()
        )

    def close(self) -> None:
        self.directory.cleanup()

    def snapshot(self) -> dict[Path, bytes]:
        return {path: path.read_bytes() for path in self.paths.ordered() if path.exists()}


def run_cli(
    fixture: VersionFixture,
    command: str,
) -> subprocess.CompletedProcess[str]:
    arguments = [sys.executable, str(SCRIPT), command]
    arguments.extend(
        [
            "--cargo-toml",
            str(fixture.paths.cargo_toml),
            "--cargo-lock",
            str(fixture.paths.cargo_lock),
            "--windows-props",
            str(fixture.paths.windows_props),
        ]
    )
    return subprocess.run(arguments, text=True, capture_output=True, check=False)


class ProductVersionFixtures(unittest.TestCase):
    def use_fixture(self, version: str = "1.0.8") -> VersionFixture:
        fixture = VersionFixture(version)
        self.addCleanup(fixture.close)
        return fixture

    def assert_check_rejected_without_writes(self, fixture: VersionFixture) -> None:
        before = fixture.snapshot()
        result = run_cli(fixture, "check")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(before, fixture.snapshot())

    def test_check_requires_three_equal_stable_values(self) -> None:
        fixture = self.use_fixture("1.0.8")
        result = run_cli(fixture, "check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual("version=1.0.8\nsynchronized=true\n", result.stdout)

    def test_forward_version_accepts_patch_minor_and_major_only(self) -> None:
        for candidate, expected in (
            ("1.0.10", True),
            ("1.1.0", True),
            ("2.0.0", True),
            ("1.0.9", False),
            ("0.9.99", False),
        ):
            with self.subTest(candidate=candidate):
                self.assertEqual(
                    product_version.is_forward_version("1.0.9", candidate), expected
                )

    def test_cross_file_mismatch_is_fail_closed(self) -> None:
        fixture = self.use_fixture("1.0.8")
        fixture.paths.windows_props.write_bytes(
            WINDOWS_PROPS.replace("{version}", "1.0.9").encode()
        )
        self.assert_check_rejected_without_writes(fixture)

    def test_missing_target_is_fail_closed(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.cargo_lock.unlink()
        self.assert_check_rejected_without_writes(fixture)

    def test_leading_zero_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.cargo_toml.write_bytes(CARGO_TOML.replace("{version}", "1.0.08").encode())
        self.assert_check_rejected_without_writes(fixture)

    def test_duplicate_cargo_version_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.cargo_toml.write_bytes(
            CARGO_TOML.replace("{version}", "1.0.8").replace(
                'version = "1.0.8"\n', 'version = "1.0.8"\nversion = "1.0.8"\n'
            ).encode()
        )
        self.assert_check_rejected_without_writes(fixture)

    def test_duplicate_lock_root_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        duplicate = b'\n[[package]]\nname = "codex_info"\nversion = "1.0.8"\n'
        fixture.paths.cargo_lock.write_bytes(fixture.paths.cargo_lock.read_bytes() + duplicate)
        self.assert_check_rejected_without_writes(fixture)

    def test_duplicate_props_version_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.windows_props.write_bytes(
            WINDOWS_PROPS.replace(
                "{version}", "1.0.8"
            ).replace(
                "    <Version>1.0.8</Version>\n",
                "    <Version>1.0.8</Version>\n    <Version>1.0.8</Version>\n",
            ).encode()
        )
        self.assert_check_rejected_without_writes(fixture)

    def test_malformed_props_xml_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.windows_props.write_bytes(b"<Project><PropertyGroup><Version>1.0.8")
        self.assert_check_rejected_without_writes(fixture)


if __name__ == "__main__":
    unittest.main()

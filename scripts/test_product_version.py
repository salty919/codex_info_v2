#!/usr/bin/env python3
"""Finite, dependency-free fixtures for product_version.py."""

from __future__ import annotations

import importlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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
WINDOWS_PROPS_MAX_BYTES = 64 * 1024


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
    expected: str | None = None,
) -> subprocess.CompletedProcess[str]:
    arguments = [sys.executable, str(SCRIPT), command]
    if command == "bump" and expected is not None:
        arguments.extend(["--expected", expected])
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

    def assert_rejected_without_writes(
        self,
        fixture: VersionFixture,
        expected: str = "1.0.8",
    ) -> None:
        before = fixture.snapshot()
        result = run_cli(fixture, "bump", expected)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(before, fixture.snapshot())

    def test_check_requires_three_equal_stable_values(self) -> None:
        fixture = self.use_fixture("1.0.8")
        result = run_cli(fixture, "check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual("version=1.0.8\nsynchronized=true\n", result.stdout)

    def test_bump_increments_patch_without_semver_carry(self) -> None:
        fixture = self.use_fixture("1.0.9")
        before = fixture.snapshot()
        result = run_cli(fixture, "bump", "1.0.9")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            "previous_version=1.0.9\n"
            "version=1.0.10\n"
            "changed=true\n"
            "major_minor_unchanged=true\n"
            "synchronized=true\n",
            result.stdout,
        )
        self.assertEqual(product_version.check_versions(fixture.paths), "1.0.10")
        for path, original in before.items():
            self.assertEqual(
                path.read_bytes(), original.replace(b"1.0.9", b"1.0.10", 1)
            )

    def test_bump_99_to_100_preserves_major_and_minor(self) -> None:
        fixture = self.use_fixture("2.7.99")
        result = run_cli(fixture, "bump", "2.7.99")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("version=2.7.100\n", result.stdout)
        self.assertEqual(product_version.check_versions(fixture.paths), "2.7.100")

    def test_next_reports_1_0_10_without_writing(self) -> None:
        fixture = self.use_fixture("1.0.9")
        before = fixture.snapshot()
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "next", "--version", "1.0.9"],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "version=1.0.10\n")
        self.assertEqual(before, fixture.snapshot())

    def test_expected_version_is_mandatory(self) -> None:
        fixture = self.use_fixture()
        before = fixture.snapshot()
        result = run_cli(fixture, "bump")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(before, fixture.snapshot())

    def test_expected_version_mismatch_is_fail_closed(self) -> None:
        fixture = self.use_fixture("1.0.9")
        self.assert_rejected_without_writes(fixture, "1.0.8")

    def test_cross_file_mismatch_is_fail_closed(self) -> None:
        fixture = self.use_fixture("1.0.8")
        fixture.paths.windows_props.write_bytes(
            WINDOWS_PROPS.replace("{version}", "1.0.9").encode()
        )
        self.assert_rejected_without_writes(fixture)

    def test_missing_target_is_fail_closed(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.cargo_lock.unlink()
        self.assert_rejected_without_writes(fixture)

    def test_leading_zero_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.cargo_toml.write_bytes(CARGO_TOML.replace("{version}", "1.0.08").encode())
        self.assert_rejected_without_writes(fixture)

    def test_duplicate_cargo_version_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.cargo_toml.write_bytes(
            CARGO_TOML.replace("{version}", "1.0.8").replace(
                'version = "1.0.8"\n', 'version = "1.0.8"\nversion = "1.0.8"\n'
            ).encode()
        )
        self.assert_rejected_without_writes(fixture)

    def test_duplicate_lock_root_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        duplicate = b'\n[[package]]\nname = "codex_info"\nversion = "1.0.8"\n'
        fixture.paths.cargo_lock.write_bytes(fixture.paths.cargo_lock.read_bytes() + duplicate)
        self.assert_rejected_without_writes(fixture)

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
        self.assert_rejected_without_writes(fixture)

    def test_malformed_props_xml_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.windows_props.write_bytes(b"<Project><PropertyGroup><Version>1.0.8")
        self.assert_rejected_without_writes(fixture)

    def test_props_internal_entity_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.windows_props.write_text(
            "<!DOCTYPE Project [<!ENTITY version '1.0.8'>]>"
            "<Project><PropertyGroup><Version>&version;</Version>"
            "</PropertyGroup></Project>",
            encoding="utf-8",
        )
        self.assert_rejected_without_writes(fixture)

    def test_props_external_entity_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        fixture.paths.windows_props.write_text(
            "<!DOCTYPE Project [<!ENTITY external SYSTEM 'file:///etc/passwd'>]>"
            "<Project><PropertyGroup><Version>1.0.8</Version>"
            "<Value>&external;</Value></PropertyGroup></Project>",
            encoding="utf-8",
        )
        self.assert_rejected_without_writes(fixture)

    def test_oversized_props_xml_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        payload = WINDOWS_PROPS.replace("{version}", "1.0.8")
        fixture.paths.windows_props.write_text(
            payload + " " * WINDOWS_PROPS_MAX_BYTES,
            encoding="utf-8",
        )
        self.assert_rejected_without_writes(fixture)

    def test_deep_props_xml_is_rejected_without_writes(self) -> None:
        fixture = self.use_fixture()
        nested = "<Node>" * 16 + "</Node>" * 16
        payload = WINDOWS_PROPS.replace("{version}", "1.0.8").replace(
            "</Project>", f"{nested}</Project>"
        )
        fixture.paths.windows_props.write_text(payload, encoding="utf-8")
        self.assert_rejected_without_writes(fixture)

    def test_atomic_commit_failure_rolls_back_every_target(self) -> None:
        fixture = self.use_fixture("1.0.8")
        before = fixture.snapshot()
        real_replace = product_version.os.replace
        calls = 0

        def fail_second_replace(source: str | bytes, destination: str | bytes) -> None:
            nonlocal calls
            calls += 1
            if calls == 2:
                raise OSError("fixture replacement failure")
            real_replace(source, destination)

        with mock.patch.object(
            product_version.os, "replace", side_effect=fail_second_replace
        ), self.assertRaises(OSError):
            product_version.bump_versions(fixture.paths, "1.0.8")
        self.assertGreaterEqual(calls, 3)
        self.assertEqual(before, fixture.snapshot())


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Finite, dependency-free fixtures for product_version.py."""

from __future__ import annotations

import importlib
import stat
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

    def test_bump_without_fchmod_preserves_modes_and_atomic_replacement(self) -> None:
        fixture = self.use_fixture("1.0.8")
        before = fixture.snapshot()
        for path, mode in zip(fixture.paths.ordered(), (0o640, 0o600, 0o644)):
            path.chmod(mode)
        modes = {path: stat.S_IMODE(path.stat().st_mode) for path in before}
        real_replace = product_version.os.replace
        real_chmod = product_version.os.chmod
        replaced = []

        with mock.patch.dict(product_version.os.__dict__):
            product_version.os.__dict__.pop("fchmod", None)
            with mock.patch.object(
                product_version.os, "chmod", wraps=real_chmod
            ) as chmod:
                def replace_staged(source: Path, destination: Path) -> None:
                    self.assertEqual(source.parent, destination.parent)
                    chmod.assert_any_call(source, modes[destination])
                    self.assertEqual(stat.S_IMODE(source.stat().st_mode), modes[destination])
                    self.assertEqual(destination.read_bytes(), before[destination])
                    self.assertEqual(
                        source.read_bytes(),
                        before[destination].replace(b"1.0.8", b"1.0.9", 1),
                    )
                    real_replace(source, destination)
                    replaced.append(destination)

                with mock.patch.object(
                    product_version.os, "replace", side_effect=replace_staged
                ):
                    result = product_version.bump_versions(fixture.paths, "1.0.8")

        self.assertEqual(result.current, "1.0.9")
        self.assertEqual(replaced, list(fixture.paths.ordered()))
        for path, original in before.items():
            self.assertEqual(path.read_bytes(), original.replace(b"1.0.8", b"1.0.9", 1))
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), modes[path])
        self.assertEqual(list(Path(fixture.directory.name).rglob(".*")), [])

    def test_chmod_failure_without_fchmod_leaves_targets_unchanged(self) -> None:
        fixture = self.use_fixture("1.0.8")
        before = fixture.snapshot()
        with mock.patch.dict(product_version.os.__dict__):
            product_version.os.__dict__.pop("fchmod", None)
            with mock.patch.object(
                product_version.os, "chmod", side_effect=OSError("fixture chmod failure")
            ), mock.patch.object(product_version.os, "replace") as replace:
                with self.assertRaisesRegex(OSError, "fixture chmod failure"):
                    product_version.bump_versions(fixture.paths, "1.0.8")
                replace.assert_not_called()
        self.assertEqual(before, fixture.snapshot())
        self.assertEqual(list(Path(fixture.directory.name).rglob(".*")), [])

    def test_fchmod_failure_is_not_retried_with_chmod(self) -> None:
        fixture = self.use_fixture("1.0.8")
        before = fixture.snapshot()
        with mock.patch.object(
            product_version.os,
            "fchmod",
            side_effect=OSError("fixture fchmod failure"),
            create=True,
        ), mock.patch.object(product_version.os, "chmod") as chmod, mock.patch.object(
            product_version.os, "replace"
        ) as replace:
            with self.assertRaisesRegex(OSError, "fixture fchmod failure"):
                product_version.bump_versions(fixture.paths, "1.0.8")
            chmod.assert_not_called()
            replace.assert_not_called()
        self.assertEqual(before, fixture.snapshot())
        self.assertEqual(list(Path(fixture.directory.name).rglob(".*")), [])

    def test_atomic_commit_failure_rolls_back_every_target(self) -> None:
        for without_fchmod in (False, True):
            with self.subTest(without_fchmod=without_fchmod):
                fixture = self.use_fixture("1.0.8")
                before = fixture.snapshot()
                modes = {path: stat.S_IMODE(path.stat().st_mode) for path in before}
                real_replace = product_version.os.replace
                calls = 0

                def fail_second_replace(
                    source: Path, destination: Path, _replace=real_replace
                ) -> None:
                    nonlocal calls
                    calls += 1
                    if calls == 2:
                        raise OSError("fixture replacement failure")
                    _replace(source, destination)

                with mock.patch.dict(product_version.os.__dict__):
                    if without_fchmod:
                        product_version.os.__dict__.pop("fchmod", None)
                    with mock.patch.object(
                        product_version.os, "replace", side_effect=fail_second_replace
                    ), self.assertRaisesRegex(OSError, "fixture replacement failure"):
                        product_version.bump_versions(fixture.paths, "1.0.8")
                self.assertGreaterEqual(calls, 3)
                self.assertEqual(before, fixture.snapshot())
                for path in before:
                    self.assertEqual(stat.S_IMODE(path.stat().st_mode), modes[path])
                self.assertEqual(list(Path(fixture.directory.name).rglob(".*")), [])


class BetaIdentityFixtures(unittest.TestCase):
    def run_beta(
        self,
        stable_version: str = "1.0.109",
        run_number: str = "7",
        run_attempt: str = "1",
        fixture: VersionFixture | None = None,
    ) -> subprocess.CompletedProcess[str]:
        arguments = [sys.executable, str(SCRIPT)]
        if fixture is not None:
            arguments.extend(
                [
                    "--cargo-toml", str(fixture.paths.cargo_toml),
                    "--cargo-lock", str(fixture.paths.cargo_lock),
                    "--windows-props", str(fixture.paths.windows_props),
                ]
            )
        arguments.extend(
            [
                "beta", "--stable-version", stable_version,
                "--run-number", run_number, "--run-attempt", run_attempt,
            ]
        )
        return subprocess.run(arguments, text=True, capture_output=True, check=False)

    def test_beta_identity_outputs_next_patch_and_release_flags(self) -> None:
        result = self.run_beta()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout,
            "version=1.0.110-beta.7.1\n"
            "tag=windows-v1.0.110-beta.7.1\n"
            "channel=beta\nprerelease=true\nmake_latest=false\n",
        )
        self.assertEqual(result.stderr, "")

    def test_beta_identity_run_and_attempt_are_distinct(self) -> None:
        for number, attempt, expected in (
            ("7", "1", "version=1.0.110-beta.7.1"),
            ("8", "1", "version=1.0.110-beta.8.1"),
            ("7", "2", "version=1.0.110-beta.7.2"),
        ):
            with self.subTest(number=number, attempt=attempt):
                result = self.run_beta(run_number=number, run_attempt=attempt)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.splitlines()[0], expected)

    def test_beta_identity_patch_increment_preserves_major_minor(self) -> None:
        result = self.run_beta(stable_version="2.7.99")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[0], "version=2.7.100-beta.7.1")
        self.assertEqual(result.stdout.splitlines()[1], "tag=windows-v2.7.100-beta.7.1")

    def test_beta_identity_invalid_inputs_fail_without_output_or_writes(self) -> None:
        fixture = VersionFixture()
        self.addCleanup(fixture.close)
        before = fixture.snapshot()
        directory = Path(fixture.directory.name)
        entries = sorted(path.relative_to(directory) for path in directory.rglob("*"))
        for stable, number, attempt, option in (
            ("1.0.109", "0", "1", "--run-number"),
            ("1.0.109", "-1", "1", "--run-number"),
            ("1.0.109", "07", "1", "--run-number"),
            ("1.0.109", "7", "0", "--run-attempt"),
            ("1.0.109", "7", "-1", "--run-attempt"),
            ("1.0.109", "7", "01", "--run-attempt"),
            ("1.0.010", "7", "1", "--stable-version"),
            ("1.0.109-beta.7.1", "7", "1", "--stable-version"),
        ):
            with self.subTest(stable=stable, number=number, attempt=attempt):
                result = self.run_beta(stable, number, attempt, fixture)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, "")
                self.assertIn(option, result.stderr)
                self.assertEqual(before, fixture.snapshot())
                self.assertEqual(
                    entries,
                    sorted(path.relative_to(directory) for path in directory.rglob("*")),
                )

    def test_beta_identity_is_read_only(self) -> None:
        fixture = VersionFixture()
        self.addCleanup(fixture.close)
        before = fixture.snapshot()
        directory = Path(fixture.directory.name)
        entries = sorted(path.relative_to(directory) for path in directory.rglob("*"))
        result = self.run_beta(fixture=fixture)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(before, fixture.snapshot())
        self.assertEqual(
            entries,
            sorted(path.relative_to(directory) for path in directory.rglob("*")),
        )


class BetaStampFixtures(unittest.TestCase):
    def use_fixture(self) -> VersionFixture:
        fixture = VersionFixture("1.0.119")
        self.addCleanup(fixture.close)
        return fixture

    def run_stamp(self, fixture: VersionFixture, **overrides: str) -> subprocess.CompletedProcess[str]:
        values = {
            "snapshot": fixture.directory.name,
            "expected-source-version": "1.0.119",
            "source-sha": "a" * 40,
            "stable-version": "1.0.109",
            "run-number": "7",
            "run-attempt": "1",
        }
        values.update(overrides)
        arguments = [sys.executable, str(SCRIPT), "stamp-beta"]
        for option, value in values.items():
            arguments.extend([f"--{option}", value])
        return subprocess.run(arguments, text=True, capture_output=True, check=False)

    def test_stamp_beta_binds_full_version_and_source_in_snapshot(self) -> None:
        fixture = self.use_fixture()
        before = fixture.snapshot()
        result = self.run_stamp(fixture)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout,
                         "version=1.0.110-beta.7.1\n"
                         "source_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"
                         "synchronized=true\n")
        for path in (fixture.paths.cargo_toml, fixture.paths.cargo_lock):
            self.assertEqual(path.read_bytes(), before[path].replace(b"1.0.119", b"1.0.110-beta.7.1", 1))
        props = fixture.paths.windows_props.read_text()
        for literal in (
            "<Version>1.0.110-beta.7.1</Version>",
            "<AssemblyVersion>1.0.110.0</AssemblyVersion>",
            "<FileVersion>1.0.110.0</FileVersion>",
            "<InformationalVersion>1.0.110-beta.7.1+aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</InformationalVersion>",
            "<IncludeSourceRevisionInInformationalVersion>false</IncludeSourceRevisionInInformationalVersion>",
            "<Deterministic>true</Deterministic>",
        ):
            self.assertIn(literal, props)
        self.assertIn(b'name = "serde"\nversion = "1.0.0"', fixture.paths.cargo_lock.read_bytes())
        # Stable release commands remain strict after the isolated beta stamp.
        for command in ("check", "bump"):
            after = fixture.snapshot()
            rejected = run_cli(fixture, command, "1.0.119")
            self.assertEqual(rejected.returncode, 1, rejected.stderr)
            self.assertEqual(after, fixture.snapshot())

    def test_stamp_invalid_identity_expected_and_numeric_inputs_do_not_write(self) -> None:
        fixture = self.use_fixture()
        before = fixture.snapshot()
        entries = sorted(Path(fixture.directory.name).rglob("*"))
        for overrides in (
            {"expected-source-version": "1.0.118"},
            {"source-sha": "not-a-source-sha"},
            {"stable-version": "1.0.65534"},
            {"run-number": "9" * 32},
        ):
            with self.subTest(overrides=overrides):
                result = self.run_stamp(fixture, **overrides)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, "")
                self.assertEqual(before, fixture.snapshot())
                self.assertEqual(entries, sorted(Path(fixture.directory.name).rglob("*")))

    def test_stamp_rejects_inconsistent_source_and_existing_metadata(self) -> None:
        for change in ("version", "metadata"):
            with self.subTest(change=change):
                fixture = self.use_fixture()
                props = fixture.paths.windows_props.read_text()
                props = (props.replace("1.0.119", "1.0.118") if change == "version" else
                         props.replace("<Deterministic>", "<AssemblyVersion>1.0.119.0</AssemblyVersion>\n    <Deterministic>"))
                fixture.paths.windows_props.write_text(props)
                before = fixture.snapshot()
                result = self.run_stamp(fixture)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(before, fixture.snapshot())

    def test_stamp_checkout_and_symlink_are_not_snapshots(self) -> None:
        fixture = self.use_fixture()
        root = Path(fixture.directory.name)
        (root / ".git").write_text("gitdir: /not-a-build-snapshot\n")
        before = fixture.snapshot()
        result = self.run_stamp(fixture)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(before, fixture.snapshot())
        (root / ".git").unlink()
        link = root / "snapshot-link"
        link.symlink_to(root, target_is_directory=True)
        result = self.run_stamp(fixture, snapshot=str(link))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(before, fixture.snapshot())

    def test_stamp_replacement_failure_restores_all_source_bytes(self) -> None:
        fixture = self.use_fixture()
        before = fixture.snapshot()
        real_replace = product_version.os.replace
        calls = 0

        def fail_third_replace(source: Path, destination: Path) -> None:
            nonlocal calls
            calls += 1
            if calls == 3:
                raise OSError("fixture beta replacement failure")
            real_replace(source, destination)

        with (
            mock.patch.object(product_version.os, "replace", side_effect=fail_third_replace),
            self.assertRaisesRegex(OSError, "fixture beta replacement failure"),
        ):
            product_version.stamp_beta(Path(fixture.directory.name), "1.0.119", "a" * 40, "1.0.109", "7", "1")
        self.assertEqual(before, fixture.snapshot())
        self.assertEqual(list(Path(fixture.directory.name).rglob(".*")), [])


if __name__ == "__main__":
    unittest.main()

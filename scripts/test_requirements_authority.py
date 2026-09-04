#!/usr/bin/env python3
"""Fixture-focused tests for requirements_authority.py."""

from __future__ import annotations

import contextlib
import io
from pathlib import Path
import sys
import tempfile
import unittest


SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import requirements_authority as authority  # noqa: E402


class RequirementAuthorityFixtures(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self._write_fixture()

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def _write(self, relative: str, text: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def _write_fixture(
        self,
        *,
        owners: dict[str, tuple[str, list[str]]] | None = None,
        ledger: list[tuple[str, str, str, str, str]] | None = None,
        product_prefix: str = "",
    ) -> None:
        owners = owners or {
            "PRODUCT": ("docs/PRODUCT_REQUIREMENTS.md", ["PROD-1"]),
            "WIRE": ("docs/WIRE.md", ["WIRE-1"]),
        }
        registry_rows = "\n".join(
            f"| `{owner}` | `{path}` | contract boundary |"
            for owner, (path, _master_ids) in owners.items()
        )
        product_master_ids = " ".join(
            owners["PRODUCT"][1]
        ) if "PRODUCT" in owners else ""
        product = (
            "<!-- codex-info-requirement-owner: PRODUCT -->\n"
            f"<!-- codex-info-master-ids: {product_master_ids} -->\n"
            f"{product_prefix}"
            "\n"
            "# Product requirements\n\n"
            "| owner ID | 唯一のowner | 所有する契約境界 |\n"
            "| --- | --- | --- |\n"
            f"{registry_rows}\n"
        )
        for owner, (path, master_ids) in owners.items():
            if path == "docs/PRODUCT_REQUIREMENTS.md":
                self._write(path, product)
            else:
                ids = " ".join(master_ids)
                self._write(
                    path,
                    f"<!-- codex-info-requirement-owner: {owner} -->\n"
                    f"<!-- codex-info-master-ids: {ids} -->\n\n"
                    f"# {owner}\n",
                )
        if ledger is None:
            ledger = [
                ("PROD-1", "PRODUCT", "product code", "unit test", "implemented"),
                ("WIRE-1", "WIRE", "wire code", "fixture", "verified"),
            ]
        ledger_rows = "\n".join(
            f"| {requirement_id} | {owner} | {scope} | {oracle} | {status} |"
            for requirement_id, owner, scope, oracle, status in ledger
        )
        self._write(
            "docs/REQUIREMENTS_LEDGER.md",
            "# Requirements ledger\n\n"
            "| ID | owner | 実装範囲 | 直接オラクル | 状態 |\n"
            "| --- | --- | --- | --- | --- |\n"
            f"{ledger_rows}\n",
        )

    def _assert_fail(self, *, final: bool = False) -> str:
        with self.assertRaises(authority.AuthorityValidationError) as caught:
            authority.validate(self.root, final=final)
        return str(caught.exception)

    def test_valid_fixture_and_normal_mode(self) -> None:
        report = authority.validate(self.root)
        self.assertEqual((report.owners, report.requirements), (2, 2))

    def test_license_comments_may_precede_owner_marker(self) -> None:
        product = (self.root / "docs/PRODUCT_REQUIREMENTS.md").read_text(
            encoding="utf-8"
        )
        self._write(
            "docs/PRODUCT_REQUIREMENTS.md",
            "<!-- Copyright example -->\n<!-- SPDX-License-Identifier: GPL-3.0-only -->\n"
            + product,
        )
        report = authority.validate(self.root)
        self.assertEqual((report.owners, report.requirements), (2, 2))

    def test_final_mode_requires_verified(self) -> None:
        message = self._assert_fail(final=True)
        self.assertIn("final validation", message)

    def test_final_mode_passes_all_verified(self) -> None:
        self._write_fixture(
            ledger=[
                ("PROD-1", "PRODUCT", "product code", "unit test", "verified"),
                ("WIRE-1", "WIRE", "wire code", "fixture", "verified"),
            ]
        )
        report = authority.validate(self.root, final=True)
        self.assertTrue(report.final)

    def test_duplicate_registry_owner(self) -> None:
        self._write_fixture(
            owners={
                "PRODUCT": ("docs/PRODUCT_REQUIREMENTS.md", ["PROD-1"]),
                "PRODUCT": ("docs/OTHER.md", ["OTHER-1"]),
            }
        )
        # Python mappings cannot retain duplicate keys, so append a duplicate
        # registry row directly and provide its referenced document.
        self._write("docs/OTHER.md", "# other\n")
        product = (self.root / "docs/PRODUCT_REQUIREMENTS.md").read_text(encoding="utf-8")
        product += "| `PRODUCT` | `docs/OTHER.md` | another boundary |\n"
        self._write("docs/PRODUCT_REQUIREMENTS.md", product)
        self.assertIn("duplicate registry owner", self._assert_fail())

    def test_duplicate_registry_path(self) -> None:
        self._write_fixture(
            owners={
                "PRODUCT": ("docs/PRODUCT_REQUIREMENTS.md", ["PROD-1"]),
                "WIRE": ("docs/PRODUCT_REQUIREMENTS.md", ["WIRE-1"]),
            }
        )
        self.assertIn("duplicate registry owner path", self._assert_fail())

    def test_missing_owner_marker(self) -> None:
        self._write("docs/WIRE.md", "<!-- codex-info-master-ids: WIRE-1 -->\n")
        self.assertIn("owner marker is missing", self._assert_fail())

    def test_duplicate_owner_marker(self) -> None:
        self._write(
            "docs/WIRE.md",
            "<!-- codex-info-requirement-owner: WIRE -->\n"
            "<!-- codex-info-requirement-owner: WIRE -->\n"
            "<!-- codex-info-master-ids: WIRE-1 -->\n",
        )
        self.assertIn("owner marker is duplicated", self._assert_fail())

    def test_wrong_owner_marker(self) -> None:
        self._write(
            "docs/WIRE.md",
            "<!-- codex-info-requirement-owner: PRODUCT -->\n"
            "<!-- codex-info-master-ids: WIRE-1 -->\n",
        )
        self.assertIn("owner marker mismatch", self._assert_fail())

    def test_unregistered_marker_owner(self) -> None:
        self._write(
            "docs/UNREGISTERED.md",
            "<!-- codex-info-requirement-owner: UNKNOWN -->\n"
            "<!-- codex-info-master-ids: UNKNOWN-1 -->\n",
        )
        self.assertIn("unregistered marker owner", self._assert_fail())

    def test_registered_owner_marker_outside_registered_path(self) -> None:
        self._write(
            "docs/DERIVED.md",
            "<!-- codex-info-requirement-owner: WIRE -->\n",
        )
        self.assertIn("outside its registered path", self._assert_fail())

    def test_master_marker_outside_registered_path(self) -> None:
        self._write(
            "docs/DERIVED.md",
            "<!-- codex-info-master-ids: WIRE-1 -->\n",
        )
        self.assertIn("master-ID marker is outside", self._assert_fail())

    def test_missing_or_duplicate_master_marker(self) -> None:
        self._write("docs/WIRE.md", "<!-- codex-info-requirement-owner: WIRE -->\n")
        self.assertIn("master-ID marker is missing", self._assert_fail())
        self._write(
            "docs/WIRE.md",
            "<!-- codex-info-requirement-owner: WIRE -->\n"
            "<!-- codex-info-master-ids: WIRE-1 -->\n"
            "<!-- codex-info-master-ids: WIRE-2 -->\n",
        )
        self.assertIn("master-ID marker is duplicated", self._assert_fail())

    def test_duplicate_master_id_across_owners(self) -> None:
        self._write(
            "docs/WIRE.md",
            "<!-- codex-info-requirement-owner: WIRE -->\n"
            "<!-- codex-info-master-ids: PROD-1 -->\n",
        )
        self.assertIn("multiple owners", self._assert_fail())

    def test_duplicate_ledger_id(self) -> None:
        self._write_fixture(
            ledger=[
                ("PROD-1", "PRODUCT", "scope", "oracle", "implemented"),
                ("PROD-1", "PRODUCT", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("duplicate ledger ID", self._assert_fail())

    def test_ledger_owner_mismatch(self) -> None:
        self._write_fixture(
            ledger=[
                ("PROD-1", "WIRE", "scope", "oracle", "verified"),
                ("WIRE-1", "WIRE", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("owner mismatch", self._assert_fail())

    def test_ledger_orphan(self) -> None:
        self._write_fixture(
            ledger=[
                ("ORPHAN-1", "PRODUCT", "scope", "oracle", "verified"),
                ("PROD-1", "PRODUCT", "scope", "oracle", "verified"),
                ("WIRE-1", "WIRE", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("ledger orphan", self._assert_fail())

    def test_master_id_without_ledger(self) -> None:
        self._write_fixture(
            ledger=[
                ("PROD-1", "PRODUCT", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("master ID without ledger", self._assert_fail())

    def test_empty_scope_oracle_status(self) -> None:
        for index, row in enumerate(
            [
                ("PROD-1", "PRODUCT", "", "oracle", "verified"),
                ("PROD-1", "PRODUCT", "scope", "", "verified"),
                ("PROD-1", "PRODUCT", "scope", "oracle", ""),
            ]
        ):
            with self.subTest(index=index):
                self._write_fixture(
                    ledger=[row, ("WIRE-1", "WIRE", "scope", "oracle", "verified")]
                )
                self.assertIn("empty ledger", self._assert_fail())

    def test_malformed_id_and_owner(self) -> None:
        self._write_fixture(
            ledger=[
                ("bad-id", "PRODUCT", "scope", "oracle", "verified"),
                ("WIRE-1", "WIRE", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("malformed ledger ID", self._assert_fail())

        self._write_fixture(
            ledger=[
                ("PROD-1", "bad owner", "scope", "oracle", "verified"),
                ("WIRE-1", "WIRE", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("malformed ledger owner", self._assert_fail())

    def test_invalid_status(self) -> None:
        self._write_fixture(
            ledger=[
                ("PROD-1", "PRODUCT", "scope", "oracle", "pending"),
                ("WIRE-1", "WIRE", "scope", "oracle", "verified"),
            ]
        )
        self.assertIn("invalid ledger status", self._assert_fail())

    def test_cli_reports_pass_and_fail(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = authority.main(["--root", str(self.root)])
        self.assertEqual(code, 0)
        self.assertIn("PASS", output.getvalue())

        self._write("docs/WIRE.md", "not an owner document\n")
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            code = authority.main(["--root", str(self.root)])
        self.assertNotEqual(code, 0)
        self.assertIn("FAIL", errors.getvalue())


if __name__ == "__main__":
    unittest.main()

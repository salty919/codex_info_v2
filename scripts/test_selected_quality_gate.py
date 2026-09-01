#!/usr/bin/env python3
"""Finite selected/skipped result matrix for the quality dispatcher."""

from __future__ import annotations

import itertools
import json
import unittest

from selected_quality_gate import OWNER_JOBS, QualitySelectionError, main, validate


OWNERS = tuple(OWNER_JOBS)
JOBS = tuple(OWNER_JOBS.values()) + ("codeql-quality", "linux-distribution")


def selection(owners: tuple[str, ...]) -> str:
    languages = ["selected-language"] if owners != ("DOCS",) else []
    binary_impact = bool(
        {"LINUX_BACKEND", "LINUX_UI", "WINDOWS"}.intersection(owners)
    )
    return json.dumps(
        {
            "owners": list(owners),
            "codeql_languages": languages,
            "binary_impact": binary_impact,
        }
    )


def successful_results(owners: tuple[str, ...]) -> dict[str, str]:
    selected_jobs = {OWNER_JOBS[owner] for owner in owners}
    codeql = owners != ("DOCS",)
    distribution = bool(
        {"LINUX_BACKEND", "LINUX_UI", "WINDOWS"}.intersection(owners)
    )
    return {
        job: (
            "success"
            if job in selected_jobs
            or (job == "codeql-quality" and codeql)
            or (job == "linux-distribution" and distribution)
            else "skipped"
        )
        for job in JOBS
    }


class SelectedQualityTests(unittest.TestCase):
    def test_all_31_nonempty_owner_combinations(self) -> None:
        cases = 0
        for count in range(1, len(OWNERS) + 1):
            for owners in itertools.combinations(OWNERS, count):
                validate(selection(owners), json.dumps(successful_results(owners)))
                cases += 1
        self.assertEqual(cases, 31)

    def test_selected_failure_cancel_skip_missing_and_nonselected_run_fail(self) -> None:
        owners = ("WINDOWS",)
        baseline = successful_results(owners)
        mutations: list[dict[str, str]] = []
        for result in ("failure", "cancelled", "skipped", ""):
            candidate = dict(baseline)
            candidate["windows-quality"] = result
            mutations.append(candidate)
        candidate = dict(baseline)
        candidate["docs-quality"] = "success"
        mutations.append(candidate)
        candidate = dict(baseline)
        candidate.pop("codeql-quality")
        mutations.append(candidate)
        candidate = dict(baseline)
        candidate["linux-distribution"] = "failure"
        mutations.append(candidate)
        for candidate in mutations:
            with self.subTest(candidate=candidate), self.assertRaises(QualitySelectionError):
                validate(selection(owners), json.dumps(candidate))

    def test_empty_unknown_or_malformed_selection_fails(self) -> None:
        cases = (
            {"owners": [], "codeql_languages": [], "binary_impact": False},
            {"owners": ["UNKNOWN"], "codeql_languages": [], "binary_impact": False},
            {"owners": ["DOCS"]},
            {"owners": ["WINDOWS"], "codeql_languages": ["csharp"], "binary_impact": False},
        )
        for value in cases:
            with self.subTest(value=value), self.assertRaises(QualitySelectionError):
                validate(json.dumps(value), json.dumps(successful_results(("DOCS",))))

    def test_cli_defaults_to_non_candidate_for_legacy_callers(self) -> None:
        self.assertEqual(
            main(
                [
                    "--selection",
                    selection(("DOCS",)),
                    "--results",
                    json.dumps(successful_results(("DOCS",))),
                ]
            ),
            0,
        )

    def test_release_candidate_requires_windows_for_binary_impact(self) -> None:
        for linux_only in (("LINUX_BACKEND",), ("LINUX_UI",)):
            with self.subTest(owners=linux_only), self.assertRaisesRegex(
                QualitySelectionError,
                "release candidate binary impact must select WINDOWS",
            ):
                validate(
                    selection(linux_only),
                    json.dumps(successful_results(linux_only)),
                    release_candidate=True,
                )

            validate(
                selection(linux_only),
                json.dumps(successful_results(linux_only)),
                release_candidate=False,
            )
        validate(
            selection(("LINUX_BACKEND", "WINDOWS")),
            json.dumps(successful_results(("LINUX_BACKEND", "WINDOWS"))),
            release_candidate=True,
        )
        validate(
            selection(("DOCS",)),
            json.dumps(successful_results(("DOCS",))),
            release_candidate=True,
        )


if __name__ == "__main__":
    unittest.main()

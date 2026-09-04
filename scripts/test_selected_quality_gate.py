#!/usr/bin/env python3
"""Finite selected/skipped result matrix for the quality dispatcher."""

from __future__ import annotations

import json
import unittest

from selected_quality_gate import OWNER_JOBS, QualitySelectionError, validate


JOBS = tuple(OWNER_JOBS.values()) + ("codeql-quality", "linux-distribution")


def selection(
    owners: tuple[str, ...],
    *,
    binary_impact: bool,
    languages: tuple[str, ...] = (),
) -> str:
    return json.dumps(
        {
            "owners": list(owners),
            "codeql_languages": languages,
            "binary_impact": binary_impact,
        }
    )


def successful_results(
    owners: tuple[str, ...],
    *,
    binary_impact: bool,
    languages: tuple[str, ...] = (),
) -> dict[str, str]:
    selected_jobs = {OWNER_JOBS[owner] for owner in owners}
    return {
        job: (
            "success"
            if job in selected_jobs
            or (job == "codeql-quality" and languages)
            or (job == "linux-distribution" and binary_impact)
            else "skipped"
        )
        for job in JOBS
    }


class SelectedQualityTests(unittest.TestCase):
    def test_finite_causal_selection_examples(self) -> None:
        cases = (
            (("DOCS",), False, ()),
            (("GOVERNANCE",), False, ("python",)),
            (("LINUX_BACKEND",), False, ()),
            (("LINUX_BACKEND",), True, ("rust",)),
            (("WINDOWS",), False, ()),
            (("WINDOWS",), True, ("csharp",)),
            (("DOCS", "LINUX_BACKEND", "WINDOWS"), True, ("csharp", "rust")),
        )
        for owners, binary_impact, languages in cases:
            with self.subTest(
                owners=owners, binary_impact=binary_impact, languages=languages
            ):
                validate(
                    selection(
                        owners,
                        binary_impact=binary_impact,
                        languages=languages,
                    ),
                    json.dumps(
                        successful_results(
                            owners,
                            binary_impact=binary_impact,
                            languages=languages,
                        )
                    ),
                )

    def test_selected_failure_cancel_skip_missing_and_nonselected_run_fail(self) -> None:
        owners = ("WINDOWS",)
        baseline = successful_results(
            owners, binary_impact=True, languages=("csharp",)
        )
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
                validate(
                    selection(
                        owners, binary_impact=True, languages=("csharp",)
                    ),
                    json.dumps(candidate),
                )

    def test_empty_unknown_or_malformed_selection_fails(self) -> None:
        cases = (
            {"owners": [], "codeql_languages": [], "binary_impact": False},
            {"owners": ["UNKNOWN"], "codeql_languages": [], "binary_impact": False},
            {"owners": ["DOCS"]},
            {
                "owners": ["WINDOWS"],
                "codeql_languages": ["unknown"],
                "binary_impact": False,
            },
            {"owners": ["DOCS"], "codeql_languages": [], "binary_impact": True},
            {
                "owners": ["DOCS", "DOCS"],
                "codeql_languages": [],
                "binary_impact": False,
            },
            {
                "owners": ["WINDOWS"],
                "codeql_languages": ["csharp", "csharp"],
                "binary_impact": False,
            },
            {
                "owners": ["DOCS"],
                "codeql_languages": ["rust"],
                "binary_impact": False,
            },
        )
        for value in cases:
            with self.subTest(value=value), self.assertRaises(QualitySelectionError):
                validate(
                    json.dumps(value),
                    json.dumps(successful_results(("DOCS",), binary_impact=False)),
                )

    def test_release_candidate_requires_windows_for_binary_impact(self) -> None:
        for linux_only in (("LINUX_BACKEND",), ("LINUX_UI",)):
            with self.subTest(owners=linux_only), self.assertRaisesRegex(
                QualitySelectionError,
                "release candidate binary impact must select WINDOWS",
            ):
                validate(
                    selection(linux_only, binary_impact=True, languages=("rust",)),
                    json.dumps(
                        successful_results(
                            linux_only, binary_impact=True, languages=("rust",)
                        )
                    ),
                    release_candidate=True,
                )

            validate(
                selection(linux_only, binary_impact=False),
                json.dumps(successful_results(linux_only, binary_impact=False)),
                release_candidate=False,
            )
        validate(
            selection(
                ("LINUX_BACKEND", "WINDOWS"),
                binary_impact=True,
                languages=("rust",),
            ),
            json.dumps(
                successful_results(
                    ("LINUX_BACKEND", "WINDOWS"),
                    binary_impact=True,
                    languages=("rust",),
                )
            ),
            release_candidate=True,
        )
        validate(
            selection(("DOCS",), binary_impact=False),
            json.dumps(successful_results(("DOCS",), binary_impact=False)),
            release_candidate=True,
        )


if __name__ == "__main__":
    unittest.main()

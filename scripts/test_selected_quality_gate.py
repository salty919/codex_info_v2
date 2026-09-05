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
    distribution_required: bool = False,
    quality_profile: str = "history-graph",
) -> str:
    return json.dumps(
        {
            "owners": list(owners),
            "codeql_languages": languages,
            "binary_impact": binary_impact,
            "distribution_required": distribution_required,
            "quality_profile": quality_profile,
        }
    )


def successful_results(
    owners: tuple[str, ...],
    *,
    binary_impact: bool,
    languages: tuple[str, ...] = (),
    distribution_required: bool = False,
) -> dict[str, str]:
    selected_jobs = {OWNER_JOBS[owner] for owner in owners}
    return {
        job: (
            "success"
            if job in selected_jobs
            or (job == "codeql-quality" and languages)
            or (job == "linux-distribution" and distribution_required)
            else "skipped"
        )
        for job in JOBS
    }


class SelectedQualityTests(unittest.TestCase):
    def test_finite_causal_selection_examples(self) -> None:
        cases = (
            (("DOCS",), False, (), "authority-only"),
            (("GOVERNANCE",), False, ("python",), "workflow-selection"),
            (("LINUX_BACKEND",), False, (), "history-graph"),
            (("LINUX_BACKEND",), True, ("rust",), "history-graph"),
            (("WINDOWS",), False, (), "history-graph"),
            (("WINDOWS",), True, ("csharp",), "history-graph"),
            (("DOCS", "LINUX_BACKEND", "WINDOWS"), True, ("csharp", "rust"), "history-graph"),
            (("LINUX_BACKEND",), True, ("rust",), "model-history"),
            (("WINDOWS",), False, (), "model-history"),
            (("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"), True, ("csharp", "rust"), "model-history"),
        )
        for owners, binary_impact, languages, quality_profile in cases:
            with self.subTest(
                owners=owners, binary_impact=binary_impact, languages=languages
            ):
                validate(
                    selection(
                        owners,
                        binary_impact=binary_impact,
                        languages=languages,
                        quality_profile=quality_profile,
                    ),
                    json.dumps(
                        successful_results(
                            owners,
                            binary_impact=binary_impact,
                            languages=languages,
                        )
                    ),
                )

    def test_model_history_rejects_unrelated_jobs_and_distribution(self) -> None:
        cases = (
            (("DOCS",), (), False),
            (("DOCS", "GOVERNANCE", "WINDOWS"), (), False),
            (("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"), ("actions", "csharp", "rust"), False),
            (("DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"), ("csharp", "rust"), True),
        )
        for owners, languages, distribution_required in cases:
            with self.subTest(
                owners=owners,
                languages=languages,
                distribution_required=distribution_required,
            ), self.assertRaises(QualitySelectionError):
                validate(
                    selection(
                        owners,
                        binary_impact=True,
                        languages=languages,
                        distribution_required=distribution_required,
                        quality_profile="model-history",
                    ),
                    json.dumps(
                        successful_results(
                            owners,
                            binary_impact=True,
                            languages=languages,
                            distribution_required=distribution_required,
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
                "distribution_required": False,
                "quality_profile": "history-graph",
            },
            {
                "owners": ["DOCS"],
                "codeql_languages": [],
                "binary_impact": True,
                "distribution_required": False,
                "quality_profile": "authority-only",
            },
            {
                "owners": ["DOCS", "DOCS"],
                "codeql_languages": [],
                "binary_impact": False,
                "distribution_required": False,
                "quality_profile": "authority-only",
            },
            {
                "owners": ["WINDOWS"],
                "codeql_languages": ["csharp", "csharp"],
                "binary_impact": False,
                "distribution_required": False,
                "quality_profile": "history-graph",
            },
            {
                "owners": ["DOCS"],
                "codeql_languages": ["rust"],
                "binary_impact": False,
                "distribution_required": False,
                "quality_profile": "authority-only",
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
                    selection(
                        linux_only,
                        binary_impact=True,
                        languages=("rust",),
                        distribution_required=True,
                        quality_profile="release",
                    ),
                    json.dumps(
                        successful_results(
                            linux_only,
                            binary_impact=True,
                            languages=("rust",),
                            distribution_required=True,
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
                distribution_required=True,
                quality_profile="release",
            ),
            json.dumps(
                successful_results(
                    ("LINUX_BACKEND", "WINDOWS"),
                    binary_impact=True,
                    languages=("rust",),
                    distribution_required=True,
                )
            ),
            release_candidate=True,
        )
        validate(
            selection(
                ("DOCS",), binary_impact=False, quality_profile="release"
            ),
            json.dumps(successful_results(("DOCS",), binary_impact=False)),
            release_candidate=True,
        )


if __name__ == "__main__":
    unittest.main()

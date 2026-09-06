#!/usr/bin/env python3
"""Direct tests for selected owner job aggregation."""

from __future__ import annotations

import json
import unittest

from selected_quality_gate import ALL_JOBS, OWNER_JOBS, QualitySelectionError, validate


def selection(
    owners: tuple[str, ...], *, binary: bool = False,
    distribution: bool = False, languages: tuple[str, ...] = (),
) -> str:
    return json.dumps(
        {
            "owners": list(owners), "codeql_languages": list(languages),
            "binary_impact": binary, "distribution_required": distribution,
        },
        separators=(",", ":"),
    )


def results(
    owners: tuple[str, ...], *, codeql: bool = False, distribution: bool = False,
) -> str:
    selected_jobs = {OWNER_JOBS[owner] for owner in owners}
    return json.dumps(
        {
            job: (
                "success"
                if job in selected_jobs
                or (job == "codeql-quality" and codeql)
                or (job == "linux-distribution" and distribution)
                else "skipped"
            )
            for job in ALL_JOBS
        },
        separators=(",", ":"),
    )


class SelectedQualityTests(unittest.TestCase):
    def test_feat_accepts_exact_owner_and_codeql_results(self) -> None:
        owners = ("DOCS", "LINUX_BACKEND")
        validate(
            selection(owners, binary=True, languages=("rust",)),
            results(owners, codeql=True), release_candidate=False,
        )

    def test_release_accepts_platform_complete_binary_result(self) -> None:
        owners = ("LINUX_BACKEND", "WINDOWS")
        validate(
            selection(owners, binary=True, distribution=True, languages=("rust",)),
            results(owners, codeql=True, distribution=True), release_candidate=True,
        )

    def test_release_accepts_non_binary_document_result(self) -> None:
        owners = ("DOCS",)
        validate(selection(owners), results(owners), release_candidate=True)

    def test_job_failure_skip_or_extra_execution_is_rejected(self) -> None:
        owners = ("LINUX_BACKEND",)
        baseline = json.loads(results(owners))
        for job, value in (
            ("linux-backend-quality", "failure"),
            ("linux-backend-quality", "skipped"),
            ("windows-quality", "success"),
        ):
            with self.subTest(job=job, value=value):
                changed = dict(baseline)
                changed[job] = value
                with self.assertRaises(QualitySelectionError):
                    validate(selection(owners, binary=True), json.dumps(changed))

    def test_release_binary_requires_windows_and_distribution(self) -> None:
        bad = (
            selection(("LINUX_BACKEND",), binary=True, distribution=True),
            selection(("LINUX_BACKEND", "WINDOWS"), binary=True),
        )
        for payload in bad:
            decoded = json.loads(payload)
            owners = tuple(decoded["owners"])
            with self.subTest(payload=payload), self.assertRaises(QualitySelectionError):
                validate(
                    payload,
                    results(owners, distribution=decoded["distribution_required"]),
                    release_candidate=True,
                )

    def test_feat_cannot_request_distribution(self) -> None:
        owners = ("LINUX_BACKEND",)
        with self.assertRaises(QualitySelectionError):
            validate(
                selection(owners, binary=True, distribution=True),
                results(owners, distribution=True), release_candidate=False,
            )

    def test_language_and_binary_decisions_require_corresponding_owner(self) -> None:
        for payload in (
            selection(("DOCS",), languages=("rust",)),
            selection(("DOCS",), binary=True),
        ):
            with self.subTest(payload=payload), self.assertRaises(QualitySelectionError):
                validate(payload, results(("DOCS",)))

    def test_malformed_selection_and_result_shape_are_rejected(self) -> None:
        valid_results = results(("DOCS",))
        bad_selections = (
            "[]", selection(()), selection(("DOCS", "DOCS")),
            json.dumps({"owners": ["DOCS"]}),
            selection(("DOCS",), languages=("ruby",)),
        )
        for payload in bad_selections:
            with self.subTest(payload=payload), self.assertRaises(QualitySelectionError):
                validate(payload, valid_results)

        incomplete = json.loads(valid_results)
        incomplete.pop("codeql-quality")
        with self.assertRaises(QualitySelectionError):
            validate(selection(("DOCS",)), json.dumps(incomplete))


if __name__ == "__main__":
    unittest.main()

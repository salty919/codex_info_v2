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
    def test_candidate_distribution_owns_ui_quality_and_must_succeed(self) -> None:
        owners = ("LINUX_UI",)
        payload = selection(owners, binary=True, distribution=True, languages=("rust",))
        observed = json.loads(results(owners, codeql=True, distribution=True))
        observed["linux-ui-quality"] = "skipped"
        validate(payload, json.dumps(observed), release_candidate=True)
        for state in ("failure", "cancelled", "skipped", None):
            with self.subTest(distribution=state):
                changed = dict(observed)
                changed["linux-distribution"] = state
                with self.assertRaises(QualitySelectionError):
                    validate(payload, json.dumps(changed), release_candidate=True)

    def test_ui_without_candidate_still_requires_its_own_job(self) -> None:
        for release in (False, True):
            with self.subTest(release=release):
                observed = json.loads(results(("LINUX_UI",)))
                observed["linux-ui-quality"] = "skipped"
                with self.assertRaises(QualitySelectionError):
                    validate(selection(("LINUX_UI",)), json.dumps(observed), release_candidate=release)

    def test_feat_accepts_exact_owner_and_codeql_results(self) -> None:
        owners = ("DOCS", "LINUX_BACKEND")
        validate(
            selection(owners, binary=True, languages=("rust",)),
            results(owners, codeql=True), release_candidate=False,
        )

    def test_release_accepts_linux_only_binary_result(self) -> None:
        owners = ("LINUX_BACKEND",)
        validate(
            selection(owners, binary=True, distribution=True, languages=("rust",)),
            results(owners, codeql=True, distribution=True), release_candidate=True,
        )

    def test_release_accepts_non_binary_document_result(self) -> None:
        owners = ("DOCS",)
        validate(selection(owners), results(owners), release_candidate=True)

    def test_selected_job_must_succeed_and_unselected_job_is_ignored(self) -> None:
        owners = ("LINUX_BACKEND",)
        baseline = json.loads(results(owners))
        for job, value in (
            ("linux-backend-quality", "failure"),
            ("linux-backend-quality", "skipped"),
        ):
            with self.subTest(job=job, value=value):
                changed = dict(baseline)
                changed[job] = value
                with self.assertRaises(QualitySelectionError):
                    validate(selection(owners, binary=True), json.dumps(changed))
        for value in ("success", "failure"):
            with self.subTest(unselected_windows=value):
                changed = dict(baseline)
                changed["windows-quality"] = value
                validate(selection(owners, binary=True), json.dumps(changed))

    def test_release_binary_requires_distribution(self) -> None:
        owners = ("LINUX_BACKEND",)
        payload = selection(owners, binary=True)
        with self.assertRaises(QualitySelectionError):
            validate(payload, results(owners), release_candidate=True)

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

    def test_malformed_selection_is_rejected(self) -> None:
        valid_results = results(("DOCS",))
        bad_selections = (
            "[]", selection(()), selection(("DOCS", "DOCS")),
            json.dumps({"owners": ["DOCS"]}),
            selection(("DOCS",), languages=("ruby",)),
        )
        for payload in bad_selections:
            with self.subTest(payload=payload), self.assertRaises(QualitySelectionError):
                validate(payload, valid_results)


if __name__ == "__main__":
    unittest.main()

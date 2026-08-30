#!/usr/bin/env python3
"""Finite selected/skipped result matrix for the quality dispatcher."""

from __future__ import annotations

import itertools
import json
import unittest

from selected_quality_gate import OWNER_JOBS, QualitySelectionError, validate


OWNERS = tuple(OWNER_JOBS)
JOBS = tuple(OWNER_JOBS.values()) + ("codeql-quality",)


def selection(owners: tuple[str, ...]) -> str:
    languages = ["selected-language"] if owners != ("DOCS",) else []
    return json.dumps({"owners": list(owners), "codeql_languages": languages})


def successful_results(owners: tuple[str, ...]) -> dict[str, str]:
    selected_jobs = {OWNER_JOBS[owner] for owner in owners}
    codeql = owners != ("DOCS",)
    return {
        job: (
            "success"
            if job in selected_jobs or (job == "codeql-quality" and codeql)
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
        for candidate in mutations:
            with self.subTest(candidate=candidate), self.assertRaises(QualitySelectionError):
                validate(selection(owners), json.dumps(candidate))

    def test_empty_unknown_or_malformed_selection_fails(self) -> None:
        cases = (
            {"owners": [], "codeql_languages": []},
            {"owners": ["UNKNOWN"], "codeql_languages": []},
            {"owners": ["DOCS"]},
        )
        for value in cases:
            with self.subTest(value=value), self.assertRaises(QualitySelectionError):
                validate(json.dumps(value), json.dumps(successful_results(("DOCS",))))


if __name__ == "__main__":
    unittest.main()

"""Behavior tests for exact Codacy coverage artifact pairing."""

from __future__ import annotations

import unittest

from codacy_coverage_artifacts import CoverageArtifactError, resolve_pair

RUN = "123"
ATTEMPT = "2"
SHA = "a" * 40


def artifact(kind: str, sha: str = SHA, *, attempt: str = ATTEMPT) -> dict[str, object]:
    return {
        "name": f"codacy-coverage-{kind}-v1-head-{sha}-run-{RUN}-attempt-{attempt}",
        "expired": False,
    }


def response(*artifacts: dict[str, object]) -> dict[str, object]:
    return {"total_count": len(artifacts), "artifacts": list(artifacts)}


class ResolvePairTests(unittest.TestCase):
    def test_no_report_is_not_eligible(self) -> None:
        self.assertFalse(resolve_pair(response(), RUN, ATTEMPT)["ready"])

    def test_one_platform_is_not_eligible(self) -> None:
        result = resolve_pair(response(artifact("rust")), RUN, ATTEMPT)
        self.assertFalse(result["ready"])
        self.assertIn("incomplete", result["reason"])

    def test_exact_pair_returns_both_names_and_source(self) -> None:
        rust = artifact("rust")
        windows = artifact("windows")
        result = resolve_pair(response(windows, rust), RUN, ATTEMPT)
        self.assertEqual(
            result,
            {
                "ready": True,
                "source_sha": SHA,
                "rust_name": rust["name"],
                "windows_name": windows["name"],
            },
        )

    def test_mismatched_source_shas_are_rejected(self) -> None:
        with self.assertRaisesRegex(CoverageArtifactError, "SHAs do not match"):
            resolve_pair(
                response(artifact("rust"), artifact("windows", "b" * 40)),
                RUN,
                ATTEMPT,
            )

    def test_duplicate_platform_is_rejected(self) -> None:
        with self.assertRaisesRegex(CoverageArtifactError, "both platforms"):
            resolve_pair(response(artifact("rust"), artifact("rust")), RUN, ATTEMPT)

    def test_previous_attempt_is_ignored(self) -> None:
        result = resolve_pair(
            response(artifact("rust", attempt="1"), artifact("windows", attempt="1")),
            RUN,
            ATTEMPT,
        )
        self.assertFalse(result["ready"])

    def test_truncated_api_response_is_rejected(self) -> None:
        with self.assertRaisesRegex(CoverageArtifactError, "incomplete"):
            resolve_pair({"total_count": 2, "artifacts": [artifact("rust")]}, RUN, ATTEMPT)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Finite release-authority cases without GitHub or workflow execution."""

from __future__ import annotations

import sys
import unittest
from unittest.mock import patch
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
import release_authority  # noqa: E402


REPOSITORY = "owner/repository"
HEAD = "1" * 40
MERGE = "2" * 40
BASE = "3" * 40


def pull() -> dict[str, object]:
    return {
        "number": 7,
        "state": "closed",
        "merged": True,
        "merge_commit_sha": MERGE,
        "base": {"ref": "main", "sha": BASE, "repo": {"full_name": REPOSITORY}},
        "head": {"ref": "feat/next", "sha": HEAD, "repo": {"full_name": REPOSITORY}},
    }


def quality_run(*, run_id: int = 41, conclusion: str = "success", attempt: int = 1) -> dict[str, object]:
    return {
        "id": run_id,
        "head_sha": HEAD,
        "event": "pull_request_target",
        "status": "completed",
        "conclusion": conclusion,
        "run_attempt": attempt,
        # pull_request_target runs are observed with an empty association list.
        "pull_requests": [],
    }


def candidate(platform: str, *, artifact_id: int) -> dict[str, object]:
    return {
        "id": artifact_id,
        "name": f"release-candidate-{platform}-v1-version-1.0.35",
        "expired": False,
        "digest": "sha256:" + str(artifact_id % 10) * 64,
    }


class ReleaseAuthorityTests(unittest.TestCase):
    def resolve(self, runs, artifacts, *, binary_impact=False):
        return release_authority.resolve_from_objects(
            repository=REPOSITORY,
            number=7,
            pull=pull(),
            runs=runs,
            artifacts=artifacts,
            binary_impact=binary_impact,
        )

    def test_exact_first_attempt_and_two_platforms_publish(self) -> None:
        result = self.resolve(
            [quality_run()],
            [candidate("windows", artifact_id=51), candidate("linux", artifact_id=52)],
            binary_impact=True,
        )
        self.assertTrue(result["publish"])
        self.assertEqual(result["tag"], "windows-v1.0.35")
        self.assertEqual([item["platform"] for item in result["artifacts"]], ["linux", "windows"])

    def test_success_without_candidates_is_a_non_binary_noop(self) -> None:
        self.assertEqual(
            self.resolve([quality_run()], []),
            {"publish": False, "reason": "non-binary-quality"},
        )

    def test_binary_without_candidates_and_non_binary_with_candidates_are_rejected(self) -> None:
        with self.assertRaises(release_authority.AuthorityError):
            self.resolve([quality_run()], [], binary_impact=True)
        with self.assertRaises(release_authority.AuthorityError):
            self.resolve(
                [quality_run()],
                [candidate("windows", artifact_id=51), candidate("linux", artifact_id=52)],
            )

    def test_binary_impact_reuses_the_version_input_blob_identity(self) -> None:
        with patch.object(
            release_authority,
            "_version_input_blob",
            side_effect=("a" * 40, "b" * 40),
        ):
            self.assertTrue(release_authority._binary_impact(REPOSITORY, BASE, HEAD))
        with patch.object(
            release_authority,
            "_version_input_blob",
            side_effect=("a" * 40, "a" * 40),
        ):
            self.assertFalse(release_authority._binary_impact(REPOSITORY, BASE, HEAD))

    def test_github_api_timeout_is_a_finite_authority_failure(self) -> None:
        with patch.object(
            release_authority.subprocess,
            "run",
            side_effect=release_authority.subprocess.TimeoutExpired("gh", 300),
        ):
            with self.assertRaises(release_authority.AuthorityError):
                release_authority._api("repos/owner/repository/pulls/7")

    def test_failure_or_rerun_cannot_publish(self) -> None:
        self.assertFalse(self.resolve([quality_run(conclusion="failure")], [])["publish"])
        self.assertFalse(self.resolve([quality_run(attempt=2)], [])["publish"])

    def test_latest_same_head_run_is_authoritative_without_old_success_fallback(self) -> None:
        self.assertFalse(
            self.resolve(
                [quality_run(run_id=41), quality_run(run_id=42, conclusion="failure")],
                [],
            )["publish"]
        )
        result = self.resolve(
            [quality_run(run_id=40, conclusion="failure"), quality_run(run_id=41)],
            [candidate("windows", artifact_id=51), candidate("linux", artifact_id=52)],
            binary_impact=True,
        )
        self.assertTrue(result["publish"])

    def test_one_platform_candidate_is_rejected(self) -> None:
        with self.assertRaises(release_authority.AuthorityError):
            self.resolve(
                [quality_run()],
                [candidate("windows", artifact_id=51)],
                binary_impact=True,
            )

    def test_non_success_workflow_signal_is_a_noop(self) -> None:
        number, head, eligible = release_authority._signal(
            "workflow_run", {"workflow_run": {"conclusion": "failure"}}
        )
        self.assertIsNone(number)
        self.assertIsNone(head)
        self.assertFalse(eligible)

    def test_structured_workflow_signal_uses_head_without_empty_run_associations(self) -> None:
        number, head, eligible = release_authority._signal(
            "workflow_run",
            {
                "workflow_run": {
                    "conclusion": "success",
                    "event": "pull_request_target",
                    "head_sha": HEAD,
                    "pull_requests": [],
                }
            },
        )
        self.assertIsNone(number)
        self.assertEqual(head, HEAD)
        self.assertTrue(eligible)

    def test_commit_pull_lookup_uses_only_exact_same_repository_main_head(self) -> None:
        unrelated = pull()
        unrelated["number"] = 6
        unrelated["base"] = {
            "ref": "feat/next",
            "sha": BASE,
            "repo": {"full_name": REPOSITORY},
        }
        self.assertEqual(
            release_authority._pull_number_for_head(
                [unrelated, pull()], REPOSITORY, HEAD
            ),
            7,
        )

    def test_merged_pull_signal_and_unmerged_close_have_distinct_results(self) -> None:
        self.assertEqual(
            release_authority._signal(
                "pull_request_target",
                {
                    "action": "closed",
                    "pull_request": {
                        "number": 7,
                        "merged": True,
                        "head": {"sha": HEAD},
                    },
                },
            ),
            (7, HEAD, True),
        )
        self.assertEqual(
            release_authority._signal(
                "pull_request_target",
                {"action": "closed", "pull_request": {"number": 7, "merged": False}},
            ),
            (None, None, False),
        )

    def test_release_signal_head_must_match_the_live_merged_pull(self) -> None:
        associated = pull()
        associated["head"] = {
            "ref": "feat/next",
            "sha": "4" * 40,
            "repo": {"full_name": REPOSITORY},
        }
        event = {
            "action": "closed",
            "pull_request": {
                "number": 7,
                "merged": True,
                "head": {"sha": "4" * 40},
            },
        }
        with patch.object(release_authority, "_api", side_effect=([associated], pull())):
            with self.assertRaises(release_authority.AuthorityError):
                release_authority.resolve_live("pull_request_target", event, REPOSITORY)

    def test_closed_signal_pr_must_match_the_exact_head_association(self) -> None:
        event = {
            "action": "closed",
            "pull_request": {
                "number": 8,
                "merged": True,
                "head": {"sha": HEAD},
            },
        }
        with patch.object(release_authority, "_api", return_value=[pull()]):
            with self.assertRaises(release_authority.AuthorityError):
                release_authority.resolve_live("pull_request_target", event, REPOSITORY)


if __name__ == "__main__":
    unittest.main()

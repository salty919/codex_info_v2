#!/usr/bin/env python3
"""Select one complete Rust/.NET coverage pair from one Actions run attempt."""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any


class CoverageArtifactError(ValueError):
    """The Actions artifact response cannot prove one complete report pair."""


def _positive_decimal(value: str, label: str) -> str:
    if re.fullmatch(r"[1-9][0-9]*", value) is None:
        raise CoverageArtifactError(f"{label} must be a positive decimal")
    return value


def resolve_pair(response: Any, run_id: str, run_attempt: str) -> dict[str, Any]:
    """Return exact artifact names only when both platforms share one source SHA."""

    run_id = _positive_decimal(run_id, "run id")
    run_attempt = _positive_decimal(run_attempt, "run attempt")
    if not isinstance(response, dict):
        raise CoverageArtifactError("artifact response must be an object")
    total = response.get("total_count")
    artifacts = response.get("artifacts")
    if (
        isinstance(total, bool)
        or not isinstance(total, int)
        or total < 0
        or not isinstance(artifacts, list)
        or total != len(artifacts)
    ):
        raise CoverageArtifactError("artifact response is incomplete")

    suffix = f"-run-{run_id}-attempt-{run_attempt}"
    pattern = re.compile(
        rf"^codacy-coverage-(rust|windows)-v1-head-([0-9a-f]{{40}})"
        rf"-run-{re.escape(run_id)}-attempt-{re.escape(run_attempt)}$"
    )
    selected: list[tuple[str, str, str]] = []
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            continue
        name = artifact.get("name")
        if not isinstance(name, str) or not (
            name.startswith("codacy-coverage-") and name.endswith(suffix)
        ):
            continue
        if artifact.get("expired") is not False:
            continue
        match = pattern.fullmatch(name)
        if match is None:
            raise CoverageArtifactError("coverage artifact name is malformed")
        selected.append((match.group(1), match.group(2), name))

    if not selected:
        return {
            "ready": False,
            "reason": "No complete coverage pair was produced by the selected quality jobs.",
        }
    if len(selected) == 1:
        return {
            "ready": False,
            "reason": "Only one platform report exists; incomplete coverage will not be uploaded.",
        }
    if len(selected) != 2:
        raise CoverageArtifactError("coverage artifact pair is not unique")
    by_kind = {kind: (sha, name) for kind, sha, name in selected}
    if len(by_kind) != 2 or set(by_kind) != {"rust", "windows"}:
        raise CoverageArtifactError("coverage artifact pair must contain both platforms")
    source_shas = {sha for sha, _ in by_kind.values()}
    if len(source_shas) != 1:
        raise CoverageArtifactError("coverage artifact source SHAs do not match")
    return {
        "ready": True,
        "source_sha": source_shas.pop(),
        "rust_name": by_kind["rust"][1],
        "windows_name": by_kind["windows"][1],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-attempt", required=True)
    args = parser.parse_args()
    try:
        result = resolve_pair(json.load(sys.stdin), args.run_id, args.run_attempt)
    except (CoverageArtifactError, json.JSONDecodeError) as exc:
        parser.error(str(exc))
    print(json.dumps(result, separators=(",", ":"), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

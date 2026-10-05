#!/usr/bin/env python3
"""Require selected quality jobs to succeed and all other jobs to stay skipped."""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any, Sequence


OWNER_JOBS = {
    "DOCS": "docs-quality",
    "GOVERNANCE": "governance-quality",
    "LINUX_BACKEND": "linux-backend-quality",
    "LINUX_UI": "linux-ui-quality",
    "WINDOWS": "windows-quality",
}
LINUX_DISTRIBUTION_JOB = "linux-distribution"
PRODUCT_OWNERS = frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"})
CODEQL_LANGUAGES = frozenset({"actions", "csharp", "python", "rust"})
LANGUAGE_OWNERS = {
    "actions": frozenset({"GOVERNANCE"}),
    "python": frozenset({"GOVERNANCE"}),
    "csharp": frozenset({"WINDOWS"}),
    "rust": frozenset({"LINUX_BACKEND", "LINUX_UI"}),
}
ALL_JOBS = frozenset(OWNER_JOBS.values()) | {
    "codeql-quality",
    LINUX_DISTRIBUTION_JOB,
}


class QualitySelectionError(ValueError):
    pass


def _object(raw: str, label: str) -> dict[str, Any]:
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise QualitySelectionError(f"{label} is not valid JSON") from exc
    if not isinstance(value, dict):
        raise QualitySelectionError(f"{label} is not an object")
    return value


def validate(
    selection_raw: str,
    results_raw: str,
    *,
    release_candidate: bool = False,
) -> None:
    selection = _object(selection_raw, "selection")
    results = _object(results_raw, "results")
    owners = selection.get("owners")
    languages = selection.get("codeql_languages")
    binary_impact = selection.get("binary_impact")
    distribution_required = selection.get("distribution_required")
    if not isinstance(owners, list) or not owners or any(
        owner not in OWNER_JOBS for owner in owners
    ):
        raise QualitySelectionError("selection has no finite owner set")
    if len(owners) != len(set(owners)):
        raise QualitySelectionError("selection contains duplicate owners")
    if not isinstance(languages, list) or any(
        language not in CODEQL_LANGUAGES for language in languages
    ):
        raise QualitySelectionError("selection has no CodeQL language list")
    if len(languages) != len(set(languages)):
        raise QualitySelectionError("selection contains duplicate CodeQL languages")
    if not isinstance(binary_impact, bool):
        raise QualitySelectionError("selection has no binary-impact decision")
    if not isinstance(distribution_required, bool):
        raise QualitySelectionError("selection has no distribution decision")
    selected = set(owners)
    for language in languages:
        if not LANGUAGE_OWNERS[language].intersection(selected):
            raise QualitySelectionError(
                f"CodeQL language has no selected source owner: {language}"
            )
    if binary_impact and not PRODUCT_OWNERS.intersection(selected):
        raise QualitySelectionError("binary impact has no product owner")
    if not release_candidate and distribution_required:
        raise QualitySelectionError(
            "feat selection must not select distribution"
        )
    if release_candidate and distribution_required != binary_impact:
        raise QualitySelectionError(
            "release candidate distribution decision must equal binary impact"
        )
    for owner, job in OWNER_JOBS.items():
        # Candidate UI acceptance runs on the single packaged Linux build.
        if owner == "LINUX_UI" and distribution_required:
            job = LINUX_DISTRIBUTION_JOB
        if owner in selected and results.get(job) != "success":
            raise QualitySelectionError(
                f"{job} must succeed, found {results.get(job)!r}"
            )
    if languages and results.get("codeql-quality") != "success":
        raise QualitySelectionError(
            f"codeql-quality must succeed, found {results.get('codeql-quality')!r}"
        )
    if distribution_required and results.get(LINUX_DISTRIBUTION_JOB) != "success":
        raise QualitySelectionError(
            "linux-distribution must succeed, "
            f"found {results.get(LINUX_DISTRIBUTION_JOB)!r}"
        )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", required=True)
    parser.add_argument("--results", required=True)
    parser.add_argument(
        "--release-candidate",
        required=True,
        choices=("true", "false"),
        help="require the platform-complete release-candidate owner set",
    )
    args = parser.parse_args(argv)
    try:
        validate(
            args.selection,
            args.results,
            release_candidate=args.release_candidate == "true",
        )
    except QualitySelectionError as exc:
        print(f"selected-quality-gate: FAIL {exc}", file=sys.stderr)
        return 1
    print("selected-quality-gate: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

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
MODEL_HISTORY_OWNERS = frozenset(
    {"DOCS", "LINUX_BACKEND", "LINUX_UI", "WINDOWS"}
)
CODEQL_LANGUAGES = frozenset({"actions", "csharp", "python", "rust"})
QUALITY_PROFILES = frozenset(
    {
        "authority-only",
        "history-graph",
        "model-history",
        "workflow-selection",
        "release",
    }
)
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
    quality_profile = selection.get("quality_profile")
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
    if quality_profile not in QUALITY_PROFILES:
        raise QualitySelectionError("selection has no finite quality profile")
    selected = set(owners)
    for language in languages:
        if not LANGUAGE_OWNERS[language].intersection(selected):
            raise QualitySelectionError(
                f"CodeQL language has no selected source owner: {language}"
            )
    if binary_impact and not PRODUCT_OWNERS.intersection(selected):
        raise QualitySelectionError("binary impact has no product owner")
    if quality_profile == "authority-only" and selected != {"DOCS"}:
        raise QualitySelectionError("authority-only profile must contain only DOCS")
    if quality_profile == "history-graph" and not PRODUCT_OWNERS.intersection(selected):
        raise QualitySelectionError("history-graph profile has no product owner")
    if quality_profile == "history-graph" and distribution_required:
        raise QualitySelectionError("history-graph profile must not select distribution")
    if quality_profile == "model-history":
        if not selected.issubset(MODEL_HISTORY_OWNERS) or not PRODUCT_OWNERS.intersection(
            selected
        ):
            raise QualitySelectionError(
                "model-history profile must select at least one allowed product owner"
            )
        if distribution_required:
            raise QualitySelectionError(
                "model-history profile must not select distribution"
            )
        if not set(languages).issubset({"csharp", "rust"}):
            raise QualitySelectionError(
                "model-history profile may select only Rust and C# CodeQL"
            )
    if quality_profile == "workflow-selection":
        if "GOVERNANCE" not in selected or PRODUCT_OWNERS.intersection(selected):
            raise QualitySelectionError(
                "workflow-selection profile must contain GOVERNANCE and no product owner"
            )
        if distribution_required:
            raise QualitySelectionError(
                "workflow-selection profile must not select distribution"
            )
    if release_candidate and binary_impact and "WINDOWS" not in owners:
        raise QualitySelectionError(
            "release candidate binary impact must select WINDOWS"
        )
    if release_candidate and quality_profile != "release":
        raise QualitySelectionError("release candidate must use release quality profile")
    if release_candidate and distribution_required != binary_impact:
        raise QualitySelectionError(
            "release candidate distribution decision must equal binary impact"
        )
    if not release_candidate and quality_profile == "release":
        raise QualitySelectionError("feat quality cannot use release profile")
    if set(results) != ALL_JOBS:
        raise QualitySelectionError("quality result keys do not match the job set")

    for owner, job in OWNER_JOBS.items():
        expected = "success" if owner in selected else "skipped"
        if results[job] != expected:
            raise QualitySelectionError(
                f"{job} must be {expected}, found {results[job]!r}"
            )
    expected_codeql = "success" if languages else "skipped"
    if results["codeql-quality"] != expected_codeql:
        raise QualitySelectionError(
            f"codeql-quality must be {expected_codeql}, "
            f"found {results['codeql-quality']!r}"
        )
    expected_distribution = "success" if distribution_required else "skipped"
    if results[LINUX_DISTRIBUTION_JOB] != expected_distribution:
        raise QualitySelectionError(
            f"linux-distribution must be {expected_distribution}, "
            f"found {results[LINUX_DISTRIBUTION_JOB]!r}"
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

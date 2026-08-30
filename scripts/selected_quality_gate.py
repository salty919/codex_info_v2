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
ALL_JOBS = frozenset(OWNER_JOBS.values()) | {"codeql-quality"}


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


def validate(selection_raw: str, results_raw: str) -> None:
    selection = _object(selection_raw, "selection")
    results = _object(results_raw, "results")
    owners = selection.get("owners")
    languages = selection.get("codeql_languages")
    if not isinstance(owners, list) or not owners or any(
        owner not in OWNER_JOBS for owner in owners
    ):
        raise QualitySelectionError("selection has no finite owner set")
    if not isinstance(languages, list):
        raise QualitySelectionError("selection has no CodeQL language list")
    if set(results) != ALL_JOBS:
        raise QualitySelectionError("quality result keys do not match the job set")

    selected = set(owners)
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


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selection", required=True)
    parser.add_argument("--results", required=True)
    args = parser.parse_args(argv)
    try:
        validate(args.selection, args.results)
    except QualitySelectionError as exc:
        print(f"selected-quality-gate: FAIL {exc}", file=sys.stderr)
        return 1
    print("selected-quality-gate: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

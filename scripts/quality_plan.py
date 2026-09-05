#!/usr/bin/env python3
"""Build the finite, pre-approved quality-check plan for changed paths.

This module deliberately only classifies paths and validates requested check
identifiers.  It never starts a build, test, network request, or other
external command.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import sys
from typing import Sequence

from ci_change_scope import ScopeError, selection_for_paths


OWNER_CHECKS: dict[str, tuple[str, ...]] = {
    "DOCS": ("requirements-authority",),
    "GOVERNANCE": ("requirements-authority", "governance-contract"),
    "LINUX_BACKEND": ("requirements-authority", "rust-format", "rust-test"),
    "LINUX_UI": ("requirements-authority", "rust-format", "rust-test"),
    "WINDOWS": ("requirements-authority", "windows-contract"),
}
HISTORY_GRAPH_OWNER_CHECKS: dict[str, tuple[str, ...]] = {
    "DOCS": ("requirements-authority",),
    "LINUX_BACKEND": ("requirements-authority", "rust-history-graph"),
    "LINUX_UI": ("requirements-authority", "linux-ui-history-graph"),
    "WINDOWS": ("requirements-authority", "windows-history-graph"),
}
MODEL_HISTORY_OWNER_CHECKS: dict[str, tuple[str, ...]] = {
    "DOCS": ("requirements-authority",),
    "LINUX_BACKEND": ("requirements-authority", "rust-model-history"),
    "LINUX_UI": ("requirements-authority", "linux-ui-model-history"),
    "WINDOWS": ("requirements-authority", "windows-model-history"),
}
RESIDENT_PUBLICATION_OWNER_CHECKS: dict[str, tuple[str, ...]] = {
    "DOCS": ("requirements-authority",),
    "LINUX_BACKEND": ("requirements-authority", "rust-resident-publication"),
}
WORKFLOW_SELECTION_OWNER_CHECKS: dict[str, tuple[str, ...]] = {
    "DOCS": ("requirements-authority",),
    "GOVERNANCE": ("requirements-authority", "governance-workflow-selection"),
}
ALL_CHECK_IDS = frozenset(
    check
    for mapping in (
        OWNER_CHECKS,
        HISTORY_GRAPH_OWNER_CHECKS,
        MODEL_HISTORY_OWNER_CHECKS,
        RESIDENT_PUBLICATION_OWNER_CHECKS,
        WORKFLOW_SELECTION_OWNER_CHECKS,
    )
    for checks in mapping.values()
    for check in checks
)


class QualityPlanError(ValueError):
    """The requested quality plan is malformed or not allowed."""


@dataclass(frozen=True)
class QualityPlan:
    """The deterministic output of :func:`plan_for_paths`."""

    affected_owners: tuple[str, ...]
    checks: tuple[str, ...]
    quality_profile: str

    def as_dict(self) -> dict[str, object]:
        return {
            "affected_owners": list(self.affected_owners),
            "checks": list(self.checks),
            "quality_profile": self.quality_profile,
        }

    def as_json(self) -> str:
        return json.dumps(
            self.as_dict(), separators=(",", ":"), sort_keys=True
        )


def _validate_requested_checks(
    requested_checks: Sequence[str], planned_checks: tuple[str, ...]
) -> None:
    """Reject every request that is not an exact member of the plan.

    A request is intentionally not silently deduplicated: duplicate or
    additional checks are an over-quality attempt and must fail before any
    caller can execute a check.
    """

    requested = tuple(requested_checks)
    seen: set[str] = set()
    for check in requested:
        if check in seen:
            raise QualityPlanError("requested check is duplicated")
        seen.add(check)

    unknown = next((check for check in requested if check not in ALL_CHECK_IDS), None)
    if unknown is not None:
        raise QualityPlanError("requested check is unknown")

    planned = set(planned_checks)
    if any(check not in planned for check in requested):
        raise QualityPlanError("requested check is outside the quality plan")


def plan_for_paths(
    paths: Sequence[str],
    *,
    quality_profile: str | None = None,
    requested_checks: Sequence[str] = (),
) -> QualityPlan:
    """Return the checks required by ``paths``.

    ``selection_for_paths`` is the sole path-to-owner classifier.  Calling it
    once also preserves its fail-closed handling for empty, malformed, and
    unknown paths.
    """

    try:
        selection = selection_for_paths(paths, quality_profile=quality_profile)
    except (ScopeError, TypeError) as exc:
        raise QualityPlanError(str(exc)) from exc

    checks: list[str] = []
    seen: set[str] = set()
    owner_checks = {
        "history-graph": HISTORY_GRAPH_OWNER_CHECKS,
        "model-history": MODEL_HISTORY_OWNER_CHECKS,
        "resident-publication": RESIDENT_PUBLICATION_OWNER_CHECKS,
        "workflow-selection": WORKFLOW_SELECTION_OWNER_CHECKS,
    }.get(selection.quality_profile, OWNER_CHECKS)
    for owner in selection.owners:
        for check in owner_checks[owner]:
            if check not in seen:
                checks.append(check)
                seen.add(check)
    planned_checks = tuple(checks)
    _validate_requested_checks(requested_checks, planned_checks)
    return QualityPlan(
        affected_owners=selection.owners,
        checks=planned_checks,
        quality_profile=selection.quality_profile,
    )


def _arguments() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "paths",
        nargs="*",
        metavar="PATH",
        help="changed repository path (may also be supplied with --path)",
    )
    parser.add_argument(
        "--path",
        dest="option_paths",
        action="append",
        default=[],
        metavar="PATH",
        help="changed repository path; repeat for multiple paths",
    )
    parser.add_argument(
        "--quality-profile",
        default=None,
        metavar="PROFILE",
        help="registered finite product-risk profile",
    )
    parser.add_argument(
        "--requested-check",
        action="append",
        default=[],
        metavar="CHECK_ID",
        help="check to authorize; repeat for multiple checks",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _arguments().parse_args(argv)
    paths = tuple(args.paths) + tuple(args.option_paths)
    try:
        plan = plan_for_paths(
            paths,
            quality_profile=args.quality_profile,
            requested_checks=tuple(args.requested_check),
        )
    except QualityPlanError as exc:
        print(f"quality-plan: FAIL {exc}", file=sys.stderr)
        return 1
    print(plan.as_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

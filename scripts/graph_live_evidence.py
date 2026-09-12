#!/usr/bin/env python3
"""Capture and independently verify live graph evidence for Issue #137.

The oracle intentionally does not import product code.  It turns the strict
v3 history wire into endpoint-pair line roles and observed token/quota idle intervals.
Linux and Windows test projections are then compared with these expectations.
Evidence is written only to an explicitly supplied directory outside the repo.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import math
import os
import struct
import sys
import urllib.parse
from collections import Counter
from collections.abc import Iterable
from dataclasses import dataclass
from itertools import pairwise
from pathlib import Path
from typing import Any


SUSTAINED_UNUSED_MIN_DURATION_SECONDS = 30 * 60

PAIR_HEADER = "Codex-Info-Published-Pair"
CAUSE_ORDER = (
    "confirmed_gap",
    "remaining_missing",
    "model_missing",
    "source_unavailable",
    "source_mismatch",
    "model_set_change",
    "remaining_anomaly",
    "model_token_anomaly",
    "model_dollar_anomaly",
    "quota_unattributed",
    "terminal_unobserved",
)
CAUSE_RANK = {cause: index for index, cause in enumerate(CAUSE_ORDER)}
METRIC_RANK = {"remaining": 0, "tokens": 1, "dollars": 2}
MAX_PAGES = 32
MAX_RESPONSE_BYTES = 32 * 1024 * 1024
MAX_EVIDENCE_BYTES = 64 * 1024 * 1024
REPOSITORY_ROOT = Path(__file__).resolve().parents[1]


def _validate_account_id(account_id: str | None) -> str | None:
    if account_id is None:
        return None
    prefix = "account-"
    suffix = account_id[len(prefix) :] if isinstance(account_id, str) and account_id.startswith(prefix) else ""
    if (
        not isinstance(account_id, str)
        or not account_id.isascii()
        or not suffix
        or suffix[0] == "0"
        or any(character < "0" or character > "9" for character in suffix)
    ):
        raise EvidenceError("account id must match account-N for a positive decimal N")
    return account_id


class EvidenceError(RuntimeError):
    """A fail-closed capture or comparison error."""


@dataclass(frozen=True)
class ModelEvidence:
    value: float | None
    reliable: bool
    origin: str
    synthetic: bool = False


@dataclass(frozen=True)
class RemainingEvidence:
    timestamp: int
    raw: float | None
    effective: float
    origin: str


def _json_loads(raw: bytes) -> Any:
    def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    try:
        return json.loads(
            raw.decode("utf-8", errors="strict"),
            object_pairs_hook=unique_object,
            parse_constant=lambda value: (_ for _ in ()).throw(
                ValueError(f"non-finite JSON number: {value}")
            ),
        )
    except (UnicodeDecodeError, ValueError, json.JSONDecodeError) as error:
        raise EvidenceError(f"invalid UTF-8 JSON response: {error}") from error


def _integer(value: Any, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise EvidenceError(f"{name} must be an integer")
    return value


def _number(value: Any, name: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise EvidenceError(f"{name} must be numeric")
    result = float(value)
    if not math.isfinite(result):
        raise EvidenceError(f"{name} must be finite")
    return result


def _causes(values: Iterable[str]) -> list[str]:
    unique = set(values)
    unknown = unique.difference(CAUSE_RANK)
    if unknown:
        raise EvidenceError(f"unknown cause IDs: {sorted(unknown)}")
    return sorted(unique, key=CAUSE_RANK.__getitem__)


def _segment_key(segment: dict[str, Any]) -> tuple[Any, ...]:
    return (
        METRIC_RANK[segment["metric"]],
        str(segment["series"]).encode("utf-8"),
        segment["start_at"],
        segment["end_at"],
        segment["style"],
    )


def _hard_break(start: int, end: int, gaps: list[dict[str, Any]]) -> bool:
    return any(start < gap["end_at"] and end > gap["start_at"] for gap in gaps)


def _validate_fixture(fixture: dict[str, Any]) -> tuple[dict[str, Any], list[dict[str, Any]], list[dict[str, Any]]]:
    period = fixture["period"]
    start = _integer(period["start_at"], "period.start_at")
    end = _integer(period["end_at"], "period.end_at")
    reset = _integer(period["reset_at"], "period.reset_at")
    if end <= start or reset < end or not isinstance(period["id"], str) or not period["id"]:
        raise EvidenceError("invalid selected period bounds or identity")
    page = fixture["history_page"]
    if page.get("api_version") != "v3":
        raise EvidenceError("history page api_version is not v3")
    samples = page.get("history_samples")
    gaps = page.get("history_gaps")
    if not isinstance(samples, list) or not isinstance(gaps, list) or not samples:
        raise EvidenceError("history page must contain non-empty samples and a gap array")
    previous = None
    for index, sample in enumerate(samples):
        if not isinstance(sample, dict):
            raise EvidenceError(f"sample {index} is not an object")
        timestamp = _integer(sample.get("timestamp"), f"sample[{index}].timestamp")
        if timestamp < start or timestamp > end or timestamp % 60 != 0:
            raise EvidenceError(f"sample {index} timestamp is outside the minute-aligned period")
        if previous is not None and timestamp <= previous:
            raise EvidenceError("history timestamps are not strictly increasing")
        previous = timestamp
        if _integer(sample.get("reset_at"), f"sample[{index}].reset_at") != reset:
            raise EvidenceError(f"sample {index} reset_at does not match the period")
        remaining = sample.get("remaining_percent")
        if remaining is not None and not 0 <= _number(remaining, f"sample[{index}].remaining_percent") <= 100:
            raise EvidenceError(f"sample {index} remaining_percent is outside 0..100")
        source = sample.get("model_source")
        if source not in {
            "confirmed",
            "legacy-unknown",
            "reconstructed-from-session",
            "unavailable",
        }:
            raise EvidenceError(f"sample {index} has an invalid model_source")
        task_active = sample.get("task_active_since_previous")
        if task_active is not None and not isinstance(task_active, bool):
            raise EvidenceError(
                f"sample {index} task_active_since_previous must be boolean or null"
            )
        complete = sample.get("models_complete")
        if not isinstance(complete, bool):
            raise EvidenceError(f"sample {index} models_complete is not boolean")
        models = sample.get("models")
        if source in {"unavailable", "reconstructed-from-session"}:
            if models is not None:
                raise EvidenceError(
                    f"sample {index} non-public model source has model numerics"
                )
            if complete:
                raise EvidenceError(
                    f"sample {index} non-public model source claims a complete model set"
                )
            continue
        if source == "legacy-unknown" and complete:
            raise EvidenceError(f"sample {index} legacy source claims a complete model set")
        if models is None and source == "legacy-unknown":
            continue
        if source == "confirmed" and not complete:
            raise EvidenceError(f"sample {index} confirmed source is not complete")
        if not isinstance(models, list):
            raise EvidenceError(f"sample {index} observed source has no model array")
        names: set[str] = set()
        for model_index, model in enumerate(models):
            name = model.get("model")
            if not isinstance(name, str) or not name or name in names:
                raise EvidenceError(f"sample {index} has an invalid/duplicate model name")
            names.add(name)
            tokens = _integer(model.get("total_tokens"), f"sample[{index}].models[{model_index}].total_tokens")
            raw_dollars = model.get("total_dollars")
            dollars = (
                None
                if raw_dollars is None
                else _number(
                    raw_dollars,
                    f"sample[{index}].models[{model_index}].total_dollars",
                )
            )
            if tokens < 0 or dollars is not None and dollars < 0:
                raise EvidenceError(f"sample {index} has a negative cumulative model value")
        if complete and source != "confirmed":
            raise EvidenceError(f"sample {index} complete model set has no direct source")
    normalized_gaps: list[dict[str, Any]] = []
    for index, gap in enumerate(gaps):
        gap_start = _integer(gap.get("start_at"), f"gap[{index}].start_at")
        gap_end = _integer(gap.get("end_at"), f"gap[{index}].end_at")
        if gap_end <= gap_start or gap_start < start or gap_end > end:
            raise EvidenceError(f"gap {index} is outside the selected period")
        normalized_gaps.append({"start_at": gap_start, "end_at": gap_end})
    return period, samples, normalized_gaps


def _rows_with_tail(period: dict[str, Any], samples: list[dict[str, Any]]) -> list[dict[str, Any]]:
    rows = [dict(sample, synthetic=False) for sample in samples]
    if rows[-1]["timestamp"] < period["end_at"]:
        rows.append(
            {
                "timestamp": period["end_at"],
                "reset_at": period["reset_at"],
                "remaining_percent": None,
                "models": None,
                "models_complete": False,
                "model_source": "unavailable",
                "synthetic": True,
            }
        )
    return rows


def _raw_model_values(
    rows: list[dict[str, Any]], model: str, metric: str
) -> list[float | None]:
    """Resolve sparse cumulative rows without resetting a used model.

    A published row must carry the model explicitly.  Row completeness proves
    that the published vector is complete; it does not turn an omitted model
    into a numeric zero for a model-specific cumulative series.
    """

    raw: list[float | None] = []
    field = "total_tokens" if metric == "tokens" else "total_dollars"
    for row in rows:
        if row.get("synthetic", False) or row["model_source"] not in {
            "confirmed",
            "legacy-unknown",
        }:
            raw.append(None)
            continue
        by_name = {item["model"]: item for item in row.get("models") or []}
        item = by_name.get(model)
        if item is None:
            raw.append(None)
            continue
        value = item.get(field)
        raw.append(None if value is None else float(value))
    return raw


def _model_projection(
    rows: list[dict[str, Any]], model: str, metric: str
) -> list[ModelEvidence]:
    raw = _raw_model_values(rows, model, metric)
    direct = [
        row["model_source"] == "confirmed"
        and bool(row.get("models_complete"))
        and not row.get("synthetic", False)
        for row in rows
    ]
    isolated: set[int] = set()
    for index in range(1, len(rows) - 1):
        left, middle, right = raw[index - 1], raw[index], raw[index + 1]
        if (
            rows[index]["timestamp"] - rows[index - 1]["timestamp"] == 60
            and rows[index + 1]["timestamp"] - rows[index]["timestamp"] == 60
            and left is not None
            and middle is not None
            and right is not None
            and direct[index - 1]
            and direct[index]
            and direct[index + 1]
            and left <= right
            and (middle < left or middle > right)
        ):
            isolated.add(index)
    baseline: float | None = None
    result: list[ModelEvidence] = []
    for index, row in enumerate(rows):
        value = raw[index]
        if row.get("synthetic", False):
            result.append(ModelEvidence(baseline, False, "held", True))
        elif value is None:
            result.append(ModelEvidence(None, False, "unknown"))
        elif not direct[index]:
            result.append(ModelEvidence(value, False, "legacy"))
        elif index in isolated or baseline is not None and value < baseline:
            result.append(ModelEvidence(baseline, False, "rejected"))
        else:
            baseline = value
            result.append(ModelEvidence(value, True, "direct"))

    # Legacy values are saved display evidence, never an arithmetic baseline.
    # Keep only monotonic values bounded by surrounding direct observations.
    direct_anchors = [
        (index, point.value)
        for index, point in enumerate(result)
        if point.origin == "direct" and point.value is not None
    ]
    display_floor: float | None = None
    for index, point in enumerate(result):
        if point.origin == "direct":
            display_floor = point.value
            continue
        if point.origin != "legacy" or point.value is None:
            continue
        upper = next(
            (value for anchor, value in direct_anchors if anchor > index),
            None,
        )
        if (
            display_floor is not None
            and point.value < display_floor
            or upper is not None
            and point.value > upper
        ):
            result[index] = ModelEvidence(None, False, "unknown")
        else:
            display_floor = point.value

    # Complete every bounded hole on the row timebase without inventing a
    # measured point. Equal monotonic endpoints prove an exact flat value;
    # differing endpoints only admit the unique affine/minimum-curvature
    # estimate. Exact session event-minute anchors remain in `anchors`, so a
    # reconstructed curve is never collapsed into one endpoint-to-endpoint
    # straight line.
    anchors = [
        index
        for index, point in enumerate(result)
        if point.value is not None and point.origin == "direct"
    ]
    result = list(result)
    for left, right in pairwise(anchors):
        left_value = result[left].value
        right_value = result[right].value
        if left_value is None or right_value is None or right_value < left_value:
            continue
        elapsed = rows[right]["timestamp"] - rows[left]["timestamp"]
        if elapsed <= 0:
            continue
        for index in range(left + 1, right):
            if result[index].value is not None:
                continue
            if right_value == left_value:
                result[index] = ModelEvidence(left_value, True, "bounded_flat")
            else:
                fraction = (rows[index]["timestamp"] - rows[left]["timestamp"]) / elapsed
                result[index] = ModelEvidence(
                    left_value + (right_value - left_value) * fraction,
                    False,
                    "interpolated",
                )

    last_anchor = next(
        (
            index
            for index in range(len(result) - 1, -1, -1)
            if result[index].value is not None and result[index].reliable
        ),
        None,
    )
    if last_anchor is not None:
        last_value = result[last_anchor].value
        for index in range(last_anchor + 1, len(result)):
            if result[index].value is None:
                result[index] = ModelEvidence(
                    last_value,
                    False,
                    "held",
                    rows[index].get("synthetic", False),
                )

    # A later valid legacy observation is still the newest saved display
    # point.  It may extend only the UI tail and remains non-authoritative.
    last_display = next(
        (
            index
            for index in range(len(result) - 1, -1, -1)
            if result[index].value is not None
            and result[index].origin in {"direct", "legacy"}
        ),
        None,
    )
    if last_display is not None:
        last_value = result[last_display].value
        for index in range(last_display + 1, len(result)):
            if result[index].origin in {"unknown", "held"}:
                result[index] = ModelEvidence(
                    last_value,
                    False,
                    "held",
                    rows[index].get("synthetic", False),
                )
    return result


def _period_model_universe(
    samples: list[dict[str, Any]],
) -> tuple[str, ...]:
    return tuple(
        sorted(
            {
                model["model"]
                for sample in samples
                for model in sample.get("models") or []
            },
            key=lambda value: value.encode("utf-8"),
        )
    )


def _token_interval_evidence(
    rows: list[dict[str, Any]],
    projections: dict[str, list[ModelEvidence]],
    before: int,
    after: int,
    universe: Iterable[str] | None = None,
) -> tuple[bool, bool, float, bool, list[str]]:
    """Validate a complete adjacent raw-token interval.

    The boolean result is intentionally strict: a projection value that was
    held after an anomaly, a missing period model, or a non-monotonic raw
    vector is not evidence.  The returned delta is only meaningful when the
    interval is complete and reliable.
    """

    start = rows[before]["timestamp"]
    end = rows[after]["timestamp"]
    if end <= start or end - start > 60:
        return False, False, 0.0, True, ["model_missing"]
    names = tuple(universe or projections.keys())
    if not names:
        return False, False, 0.0, True, ["model_missing"]
    causes: list[str] = []
    total_delta = 0.0
    inferred = False
    for model in names:
        left_projection = projections.get(model, [])[before] if model in projections else None
        right_projection = projections.get(model, [])[after] if model in projections else None
        if left_projection is None or right_projection is None:
            causes.append("model_missing")
            continue
        left = left_projection.value
        right = right_projection.value
        if left is None or right is None:
            causes.append("model_missing")
            continue
        if (
            not left_projection.reliable
            or not right_projection.reliable
            or left < 0
            or right < 0
            or right < left
        ):
            causes.append("model_token_anomaly")
            continue
        total_delta += right - left
    if causes or not math.isfinite(total_delta):
        return (
            False,
            False,
            0.0,
            True,
            list(dict.fromkeys(causes or ["model_token_anomaly"])),
        )
    return True, total_delta > 0.0, total_delta, inferred, []


def _projected_token_interval_delta(
    rows: list[dict[str, Any]],
    projections: dict[str, list[ModelEvidence]],
    before: int,
    after: int,
    universe: Iterable[str] | None = None,
) -> tuple[bool, float, bool, bool]:
    """Return direct/bounded token shape for UI quota presentation only.

    Legacy observations may be drawn, but they cannot weight a second
    prediction. Doing so would promote an explicitly incomplete saved model
    vector into arithmetic authority.
    """

    start = rows[before]["timestamp"]
    end = rows[after]["timestamp"]
    names = tuple(universe or projections.keys())
    if end <= start or not names:
        return False, 0.0, True, False
    total_delta = 0.0
    exact = True
    inferred = end - start > 60
    for model in names:
        values = projections.get(model)
        if values is None:
            return False, 0.0, True, False
        left = values[before]
        right = values[after]
        if (
            left.value is None
            or right.value is None
            or left.origin in {"held", "legacy", "rejected"}
            or right.origin in {"held", "legacy", "rejected"}
            or left.value < 0
            or right.value < left.value
        ):
            return False, 0.0, True, False
        total_delta += right.value - left.value
        if not math.isfinite(total_delta):
            return False, 0.0, True, False
        exact &= left.reliable and right.reliable
        inferred |= not left.reliable or not right.reliable
    return True, total_delta, inferred, exact and total_delta <= sys.float_info.epsilon


def _remaining_projection(
    period: dict[str, Any],
    rows: list[dict[str, Any]],
    token_models: dict[str, list[ModelEvidence]],
    gaps: list[dict[str, Any]],
) -> list[RemainingEvidence]:
    raw = [None if row.get("synthetic", False) else row["remaining_percent"] for row in rows]
    isolated: set[int] = set()
    for index in range(1, len(rows) - 1):
        left, middle, right = raw[index - 1], raw[index], raw[index + 1]
        if (
            rows[index]["timestamp"] - rows[index - 1]["timestamp"] == 60
            and rows[index + 1]["timestamp"] - rows[index]["timestamp"] == 60
            and left is not None
            and middle is not None
            and right is not None
            and left >= right
            and (middle > left or middle < right)
        ):
            isolated.add(index)
    values: list[float | None] = [None] * len(rows)
    origins: list[str | None] = [None] * len(rows)
    raw_reliable = [False] * len(rows)
    baseline: float | None = None
    for index, value in enumerate(raw):
        if value is None:
            continue
        if index in isolated or baseline is not None and value > baseline:
            values[index] = baseline
            origins[index] = "monotonic_hold"
        else:
            baseline = value
            values[index] = value
            origins[index] = "raw"
            raw_reliable[index] = True

    universe = tuple(token_models)

    def token_interval(before: int, after: int) -> tuple[bool, bool, float, bool, list[str]]:
        if _hard_break(rows[before]["timestamp"], rows[after]["timestamp"], gaps):
            return False, False, 0.0, True, ["confirmed_gap"]
        return _token_interval_evidence(
            rows,
            token_models,
            before,
            after,
            universe,
        )

    def span_weights(left: int, right: int) -> tuple[list[float], list[bool], bool]:
        """Choose exactly one weighting basis for a whole quota-drop span."""

        elapsed = [
            max(0, rows[index + 1]["timestamp"] - rows[index]["timestamp"])
            for index in range(left, right)
        ]
        evidence = []
        for index in range(left, right):
            start = rows[index]["timestamp"]
            end = rows[index + 1]["timestamp"]
            if _hard_break(start, end, gaps):
                evidence.append((False, 0.0, True, False))
            else:
                evidence.append(
                    _projected_token_interval_delta(
                        rows,
                        token_models,
                        index,
                        index + 1,
                        universe,
                    )
                )
        if evidence and all(item[0] for item in evidence):
            token_weights = [item[1] for item in evidence]
            if sum(token_weights) > 0.0:
                return token_weights, [item[2] for item in evidence], True
            return elapsed, [True] * len(elapsed), False
        # Do not mix token units and seconds. Unknown intervals use elapsed
        # weights, while exact token-flat intervals keep weight zero so a
        # later data gap cannot smear a quota drop into proven idle time.
        fallback = [
            0 if item[0] and item[3] else elapsed[index]
            for index, item in enumerate(evidence)
        ]
        inferred = [not (item[0] and item[3]) for item in evidence]
        if sum(fallback) <= 0:
            return elapsed, [True] * len(elapsed), False
        return fallback, inferred, False

    run_start = 0
    while run_start < len(rows):
        if raw[run_start] is not None:
            run_start += 1
            continue
        run_end = run_start
        while run_end < len(rows) and raw[run_end] is None:
            run_end += 1
        left = run_start - 1
        if left < 0 or values[left] is None:
            run_start = run_end
            continue
        bounded = run_end < len(rows) and values[run_end] is not None
        interpolated = False
        if bounded and values[run_end] < values[left]:
            weights, inferred, _ = span_weights(left, run_end)
            weighted_seconds = sum(weights)
            if weighted_seconds > 0:
                weighted_elapsed = 0
                for index in range(run_start, run_end):
                    weight = weights[index - left - 1]
                    weighted_elapsed += weight
                    values[index] = values[left] + (values[run_end] - values[left]) * (
                        weighted_elapsed / weighted_seconds
                    )
                    origins[index] = "interpolated"
                interpolated = True
        if not interpolated:
            for index in range(run_start, run_end):
                if values[index - 1] is None:
                    break
                values[index] = values[index - 1]
                origins[index] = (
                    "synthetic_tail_hold"
                    if rows[index].get("synthetic", False)
                    else "bounded_null_hold"
                    if bounded
                    else "terminal_null_hold"
                )
        run_start = run_end

    anchors: list[int] = []
    for index, value in enumerate(raw):
        if value is None or not raw_reliable[index]:
            continue
        if not anchors or raw[anchors[-1]] != value:
            anchors.append(index)
    for left, right in pairwise(anchors):
        if right - left < 2 or values[right] >= values[left]:
            continue
        if any(raw[index] is not None and not raw_reliable[index] for index in range(left + 1, right)):
            continue
        weights, inferred, model_shaped = span_weights(left, right)
        weighted_seconds = sum(weights)
        if weighted_seconds <= 0:
            continue
        weighted_elapsed = 0
        for offset, index in enumerate(range(left + 1, right)):
            weight = weights[offset]
            weighted_elapsed += weight
            smoothed = values[left] + (values[right] - values[left]) * (
                weighted_elapsed / weighted_seconds
            )
            prior_value = values[index]
            prior_origin = origins[index]
            values[index] = smoothed
            if raw_reliable[index] and raw[index] == smoothed:
                continue
            if (
                raw[index] is None
                and prior_value == smoothed
                and prior_origin == "bounded_null_hold"
            ):
                continue
            origins[index] = (
                "activity_smoothed"
                if model_shaped and not inferred[offset] and raw_reliable[index]
                else "interpolated"
            )

    return [
        RemainingEvidence(row["timestamp"], raw[index], values[index], origins[index])
        for index, row in enumerate(rows)
        if values[index] is not None and origins[index] is not None
    ]


def _model_segments(
    rows: list[dict[str, Any]],
    model: str,
    metric: str,
    projection: list[ModelEvidence],
    gaps: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    segments: list[dict[str, Any]] = []
    previous: int | None = None
    for index, point in enumerate(projection):
        if point.value is None:
            continue
        if previous is None:
            previous = index
            continue
        start, end = rows[previous]["timestamp"], rows[index]["timestamp"]
        causes: list[str] = []
        if _hard_break(start, end, gaps):
            causes.append("confirmed_gap")
        if index != previous + 1 or end - start > 60:
            causes.append("model_missing")
        interval_sources = {row["model_source"] for row in rows[previous : index + 1]}
        if "unavailable" in interval_sources:
            causes.append("source_unavailable")
        if len(interval_sources) > 1 and causes:
            causes.append("source_mismatch")
        origins = {projection[previous].origin, point.origin}
        if "rejected" in origins:
            causes.append("model_token_anomaly" if metric == "tokens" else "model_dollar_anomaly")
        elif (
            not origins.issubset({"direct", "session"})
            or not projection[previous].reliable
            or not point.reliable
        ):
            causes.append("model_missing")
        if rows[index].get("synthetic", False) or rows[previous].get("synthetic", False):
            causes.append("terminal_unobserved")
        style = (
            "dashed"
            if causes
            else "flat"
            if projection[previous].value == point.value
            else "rising"
        )
        segments.append(
            {
                "metric": metric,
                "series": model,
                "start_at": start,
                "end_at": end,
                "style": style,
                "causes": _causes(causes),
            }
        )
        previous = index
    return segments


def _remaining_segments(
    samples: list[dict[str, Any]],
    rows: list[dict[str, Any]],
    evidence: list[RemainingEvidence],
    token_models: dict[str, list[ModelEvidence]],
    gaps: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    row_index = {row["timestamp"]: index for index, row in enumerate(rows)}
    observed_minutes = {
        sample["timestamp"] for sample in samples if sample["remaining_percent"] is not None
    }
    result: list[dict[str, Any]] = []
    for before, after in pairwise(evidence):
        left, right = row_index[before.timestamp], row_index[after.timestamp]
        causes: list[str] = []
        if _hard_break(before.timestamp, after.timestamp, gaps):
            causes.append("confirmed_gap")
        measured_origins = {"raw", "activity_smoothed"}
        if before.origin not in measured_origins or after.origin not in measured_origins:
            causes.append("remaining_missing")
        if before.origin == "monotonic_hold" or after.origin == "monotonic_hold":
            causes.append("remaining_anomaly")
        if after.origin == "synthetic_tail_hold" or before.origin == "synthetic_tail_hold":
            causes.append("terminal_unobserved")
        contiguous_quota = (
            before.timestamp in observed_minutes
            and after.timestamp in observed_minutes
            and after.timestamp - before.timestamp <= 60
        )
        if _hard_break(before.timestamp, after.timestamp, gaps):
            available, advanced, _, inferred, model_causes = (
                False,
                False,
                0.0,
                True,
                ["confirmed_gap"],
            )
        else:
            available, advanced, _, inferred, model_causes = _token_interval_evidence(
                rows,
                token_models,
                left,
                right,
                tuple(token_models),
            )
        if after.effective < before.effective and (not available or not advanced):
            causes.append("quota_unattributed")
            causes.extend(model_causes)
        elif after.effective < before.effective and inferred:
            # The drop is token-shaped, but the model cadence was restored
            # from session evidence or an exact bounded-flat proof rather
            # than observed directly by the periodic recorder.
            causes.append("model_missing")
        if not contiguous_quota:
            causes.append("remaining_missing")
        result.append(
            {
                "metric": "remaining",
                "series": "remaining",
                "start_at": before.timestamp,
                "end_at": after.timestamp,
                "style": "dashed" if causes else "solid",
                "causes": _causes(causes),
            }
        )
    return result


def _idle_intervals(
    period: dict[str, Any],
    samples: list[dict[str, Any]],
    token_models: dict[str, list[ModelEvidence]],
    gaps: list[dict[str, Any]],
) -> list[dict[str, int]]:
    rows = [dict(sample, synthetic=False) for sample in samples]
    timestamps = [row["timestamp"] for row in rows]
    universe = tuple(token_models)
    remaining = {
        point.timestamp: point
        for point in _remaining_projection(period, rows, token_models, gaps)
    }

    intervals: list[tuple[int, int, int]] = []
    for index in range(len(rows) - 1):
        elapsed = timestamps[index + 1] - timestamps[index]
        start, end = timestamps[index], timestamps[index + 1]
        # Reliable token/quota observations are the idle authority. A known
        # active task disproves idle, while unavailable historical lifecycle
        # evidence must not erase an otherwise observed flat interval.
        if rows[index + 1].get("task_active_since_previous") is True:
            continue
        if _hard_break(start, end, gaps):
            continue
        if elapsed == 60:
            available, advanced, _, _, _ = _token_interval_evidence(
                rows,
                token_models,
                index,
                index + 1,
                universe,
            )
            observed_count = 1 if available and not advanced else 0
        elif elapsed >= 120:
            exact_origins = {"direct", "session"}
            observed_count = 2 if universe and all(
                token_models[model][index].value is not None
                and token_models[model][index + 1].value is not None
                and token_models[model][index].origin in exact_origins
                and token_models[model][index + 1].origin in exact_origins
                and token_models[model][index].value
                == token_models[model][index + 1].value
                for model in universe
            ) else 0
        else:
            observed_count = 0
        if observed_count == 0:
            continue
        before = remaining.get(start)
        after = remaining.get(end)
        if before is None or after is None:
            continue
        if before.origin not in {
            "raw",
            "activity_smoothed",
            "bounded_null_hold",
            "interpolated",
        }:
            continue
        if after.origin not in {
            "raw",
            "activity_smoothed",
            "bounded_null_hold",
            "interpolated",
        }:
            continue
        if before.effective != after.effective:
            continue
        intervals.append((start, end, observed_count))
    merged: list[list[int]] = []
    for start, end, observed_count in sorted(intervals):
        if merged and start == merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], end)
            merged[-1][2] += observed_count
        else:
            merged.append([start, end, observed_count])
    confirmed = [
        [max(start, period["start_at"]), min(end, period["end_at"])]
        for start, end, observed_count in merged
        if observed_count >= 2
        and min(end, period["end_at"]) > max(start, period["start_at"])
    ]
    by_timestamp = {timestamp: index for index, timestamp in enumerate(timestamps)}
    exact_model_origins = {"direct", "session", "bounded_flat"}
    accepted_remaining_origins = {
        "raw",
        "activity_smoothed",
        "bounded_null_hold",
        "interpolated",
    }

    def bridge_is_confirmed_flat(start: int, end: int) -> bool:
        if end <= start or _hard_break(start, end, gaps):
            return False
        left_index = by_timestamp.get(start)
        right_index = by_timestamp.get(end)
        if left_index is None or right_index is None or right_index <= left_index:
            return False
        if any(
            row.get("task_active_since_previous") is True
            for row in rows[left_index + 1 : right_index + 1]
        ):
            return False
        for model in universe:
            projection = token_models.get(model)
            if projection is None:
                return False
            left = projection[left_index]
            right = projection[right_index]
            if (
                left.value is None
                or right.value is None
                or left.origin not in exact_model_origins
                or right.origin not in exact_model_origins
                or left.value != right.value
            ):
                return False
            if any(
                point.value != left.value
                or not point.reliable
                or point.origin not in exact_model_origins
                for point in projection[left_index : right_index + 1]
            ):
                return False
        left_remaining = remaining.get(start)
        right_remaining = remaining.get(end)
        if (
            left_remaining is None
            or right_remaining is None
            or left_remaining.origin not in accepted_remaining_origins
            or right_remaining.origin not in accepted_remaining_origins
            or left_remaining.effective != right_remaining.effective
        ):
            return False
        return all(
            point.origin in accepted_remaining_origins
            and point.effective == left_remaining.effective
            for point in (
                remaining[timestamp]
                for timestamp in timestamps[left_index : right_index + 1]
            )
        )

    bridged: list[list[int]] = []
    for start, end in confirmed:
        if bridged and start > bridged[-1][1] and bridge_is_confirmed_flat(bridged[-1][1], start):
            bridged[-1][1] = end
        else:
            bridged.append([start, end])
    return [
        {"start_at": start, "end_at": end}
        for start, end in bridged
        if end - start >= SUSTAINED_UNUSED_MIN_DURATION_SECONDS
    ]


def build_expected(fixture: dict[str, Any]) -> tuple[list[dict[str, Any]], list[dict[str, int]]]:
    period, samples, gaps = _validate_fixture(fixture)
    rows = _rows_with_tail(period, samples)
    universe = _period_model_universe(samples)
    projections = {
        metric: {model: _model_projection(rows, model, metric) for model in universe}
        for metric in ("tokens", "dollars")
    }
    segments: list[dict[str, Any]] = []
    token_models = projections["tokens"]
    remaining = _remaining_projection(period, rows, token_models, gaps)
    segments.extend(_remaining_segments(samples, rows, remaining, token_models, gaps))
    for metric in ("tokens", "dollars"):
        for model in universe:
            segments.extend(_model_segments(rows, model, metric, projections[metric][model], gaps))
    segments.sort(key=_segment_key)
    return segments, _idle_intervals(period, samples, token_models, gaps)


def _canonical_coordinate(
    timestamp: int,
    value: float,
    period_start: int,
    period_end: int,
    maximum: float,
    remaining: bool,
) -> tuple[float, float]:
    span = max(1, period_end - period_start)
    x = min(100.0, max(0.0, (timestamp - period_start) / span * 100.0))
    if remaining:
        y = min(99.0, max(1.0, 99.0 - min(100.0, max(0.0, value)) * 0.98))
    else:
        y = min(99.0, max(1.0, 99.0 - max(0.0, value) / max(1.0, maximum) * 98.0))
    return round(x, 12), round(y, 12)


def _canonical_segment(start: tuple[float, float], end: tuple[float, float]) -> str:
    return f"M{start[0]:.2f} {start[1]:.2f} L{end[0]:.2f} {end[1]:.2f}"


def _canonical_dashes(start: tuple[float, float], end: tuple[float, float]) -> list[str]:
    dx = end[0] - start[0]
    dy = end[1] - start[1]
    length = math.hypot(dx, dy)
    if not math.isfinite(length) or length <= sys.float_info.epsilon:
        return []
    result: list[str] = []
    offset = 0.0
    while offset < length:
        dash_end = min(offset + 0.45, length)
        start_fraction = offset / length
        end_fraction = dash_end / length
        result.append(
            _canonical_segment(
                (
                    start[0] + dx * start_fraction,
                    start[1] + dy * start_fraction,
                ),
                (
                    start[0] + dx * end_fraction,
                    start[1] + dy * end_fraction,
                ),
            )
        )
        offset += 0.75
    return result


def _canonical_path(
    segments: list[dict[str, Any]],
    style: str,
    values: dict[int, float],
    period: dict[str, Any],
    maximum: float,
    remaining: bool,
) -> str:
    commands: list[str] = []
    for segment in segments:
        if segment["style"] != style:
            continue
        start = _canonical_coordinate(
            segment["start_at"],
            values[segment["start_at"]],
            period["start_at"],
            period["end_at"],
            maximum,
            remaining,
        )
        end = _canonical_coordinate(
            segment["end_at"],
            values[segment["end_at"]],
            period["start_at"],
            period["end_at"],
            maximum,
            remaining,
        )
        if style == "dashed":
            commands.extend(_canonical_dashes(start, end))
        else:
            commands.append(_canonical_segment(start, end))
    return " ".join(commands)


def _remaining_markers(
    evidence: list[RemainingEvidence],
    period: dict[str, Any],
) -> list[dict[str, Any]]:
    span = max(1, period["end_at"] - period["start_at"])
    seen: set[int] = set()
    markers: list[dict[str, Any]] = []
    for before, after in pairwise(evidence):
        if after.timestamp < before.timestamp or after.effective >= before.effective:
            continue
        boundary = math.floor(before.effective)
        if abs(before.effective - boundary) <= sys.float_info.epsilon:
            boundary -= 1
        lowest = math.ceil(after.effective)
        while boundary >= lowest:
            if boundary < before.effective and boundary >= after.effective and boundary not in seen:
                seen.add(boundary)
                fraction = min(
                    1.0,
                    max(
                        0.0,
                        (boundary - before.effective)
                        / (after.effective - before.effective),
                    ),
                )
                timestamp = before.timestamp + (after.timestamp - before.timestamp) * fraction
                markers.append(
                    {
                        "x": f"{(timestamp - period['start_at']) / span * 100.0:.12f}",
                        "y_top": f"{99.0 - boundary * 0.98:.12f}",
                        "boundary": boundary,
                    }
                )
            boundary -= 1
    return markers


def _f32(value: float) -> float:
    return struct.unpack("!f", struct.pack("!f", value))[0]


def _native_graph_y(value: float, maximum: float) -> float:
    return _f32(
        min(0.99, max(0.01, (99.0 - value / max(1.0, maximum) * 98.0) / 100.0))
    )


def _format_token_count(value: float) -> str:
    return f"{math.floor(max(0.0, value) + 0.5):,}"


def _format_token_axis_value(value: float) -> str:
    value = max(0.0, value)
    if value >= 1_000_000_000:
        return f"{value / 1_000_000_000:.1f}B"
    if value >= 1_000_000:
        return f"{value / 1_000_000:.1f}M"
    if value >= 1_000:
        return f"{value / 1_000:.1f}K"
    return _format_token_count(value)


def _format_percent(value: float) -> str:
    return f"{value:.0f}%" if abs(value % 1.0) < 0.0001 else f"{value:.1f}%"


def _endpoint_labels(
    projections: dict[str, list[ModelEvidence]],
    maximum: float,
    metric: str,
    remaining: list[RemainingEvidence],
) -> list[dict[str, str]]:
    rank = {"remaining": 0, "LUNA": 1, "TERRA": 2, "SOL": 3, "ASTRA": 4}
    candidates: list[dict[str, Any]] = []
    for model in ("ASTRA", "LUNA", "TERRA", "SOL"):
        values = projections.get(model)
        if values is None:
            continue
        latest = next(
            (
                point.value
                for point in reversed(values)
                if point.value is not None and math.isfinite(point.value) and point.value >= 0
            ),
            None,
        )
        if latest is None:
            continue
        candidates.append(
            {
                "series": model,
                "text": f"${latest:.2f}" if metric == "dollars" else _format_token_count(latest),
                "point_y": _native_graph_y(latest, maximum),
            }
        )
    if remaining:
        latest_remaining = remaining[-1].effective
        candidates.append(
            {
                "series": "remaining",
                "text": _format_percent(latest_remaining),
                "point_y": _native_graph_y(latest_remaining, 100.0),
            }
        )
    candidates.sort(key=lambda item: (item["point_y"], rank[item["series"]]))
    if not candidates:
        return []

    half = _f32(8.0 / 204.0)
    separation = _f32(16.0 / 204.0)
    lower = half
    upper = _f32(1.0 - half)
    label_y = [min(upper, max(lower, item["point_y"])) for item in candidates]
    for index in range(1, len(label_y)):
        label_y[index] = max(
            min(upper, max(lower, label_y[index])),
            _f32(label_y[index - 1] + separation),
        )
    if label_y[-1] > upper:
        label_y[-1] = upper
        for index in range(len(label_y) - 2, -1, -1):
            label_y[index] = min(
                label_y[index],
                _f32(label_y[index + 1] - separation),
            )

    return [
        {
            "series": item["series"],
            "text": item["text"],
            "point_y": f"{item['point_y']:.9f}",
            "label_y": f"{label_y[index]:.9f}",
        }
        for index, item in enumerate(candidates)
    ]


def build_expected_render_contracts(fixture: dict[str, Any]) -> dict[str, Any]:
    period, samples, gaps = _validate_fixture(fixture)
    rows = _rows_with_tail(period, samples)
    universe = _period_model_universe(samples)
    token_models = {
        model: _model_projection(rows, model, "tokens") for model in universe
    }
    remaining_evidence = _remaining_projection(period, rows, token_models, gaps)
    remaining_values = {
        point.timestamp: point.effective for point in remaining_evidence
    }
    remaining_segments = _remaining_segments(
        samples,
        rows,
        remaining_evidence,
        token_models,
        gaps,
    )
    idle = _idle_intervals(period, samples, token_models, gaps)
    idle_geometry = [
        {
            "start": f"{(interval['start_at'] - period['start_at']) / max(1, period['end_at'] - period['start_at']) * 100.0:.12f}",
            "width": f"{(interval['end_at'] - interval['start_at']) / max(1, period['end_at'] - period['start_at']) * 100.0:.12f}",
        }
        for interval in idle
    ]
    markers = _remaining_markers(remaining_evidence, period)
    remaining_points = [
        {
            "timestamp": point.timestamp,
            "value": f"{point.effective:.12f}",
            "origin": point.origin,
        }
        for point in remaining_evidence
    ]
    time_ticks = [
        period["start_at"]
        + int((period["end_at"] - period["start_at"]) * fraction)
        for fraction in (0.0, 0.25, 0.5, 0.75, 1.0)
    ]
    contracts: dict[str, Any] = {}
    for metric in ("dollars", "tokens"):
        projections = {
            model: _model_projection(rows, model, metric) for model in universe
        }
        finite_values = [
            point.value
            for projection in projections.values()
            for point in projection
            if point.value is not None and math.isfinite(point.value) and point.value >= 0
        ]
        maximum = max([1.0, *finite_values])
        axis_labels = [
            (
                f"${maximum * fraction:.2f}"
                if metric == "dollars"
                else _format_token_axis_value(maximum * fraction)
            )
            for fraction in (1.0, 0.75, 0.5, 0.25, 0.0)
        ]
        models = []
        for model in universe:
            segments = _model_segments(rows, model, metric, projections[model], gaps)
            values = {
                row["timestamp"]: point.value
                for row, point in zip(rows, projections[model], strict=True)
                if point.value is not None
            }
            models.append(
                {
                    "series": model,
                    "flat": _canonical_path(
                        segments, "flat", values, period, maximum, False
                    ),
                    "rising": _canonical_path(
                        segments, "rising", values, period, maximum, False
                    ),
                    "dashed": _canonical_path(
                        segments, "dashed", values, period, maximum, False
                    ),
                }
            )
        endpoint_values = [
            {
                "series": model,
                "timestamp": period["end_at"],
                "value": projections[model][-1].value,
            }
            for model in universe
        ]
        endpoint_values.append(
            {
                "series": "remaining",
                "timestamp": period["end_at"],
                "value": remaining_evidence[-1].effective,
            }
        )
        contracts[metric] = {
            "viewbox": [100, 100],
            "model_maximum": f"{maximum:.12f}",
            "time_ticks": time_ticks,
            "axis_labels": axis_labels,
            "axis_grid_y": [f"{fraction:.12f}" for fraction in (0.0, 0.25, 0.5, 0.75, 1.0)],
            "endpoint_labels": _endpoint_labels(
                projections,
                maximum,
                metric,
                remaining_evidence,
            ),
            "endpoint_values": endpoint_values,
            "latest_timestamp": period["end_at"],
            "layout": {
                "reference_data_width": 788,
                "plot_width": 694 if metric == "dollars" else 662,
                "gutter_width": 94 if metric == "dollars" else 126,
                "label_gap": 10,
                "label_width": 80 if metric == "dollars" else 112,
                "right_padding": 4,
                "minimum_plot_height": 204,
            },
            "styles": {
                "plot_surface": "#121c2c",
                "grid": "#263850",
                "axis_text": "#78879c",
                "idle_band": "#1a2838",
                "remaining": "#56b2f5",
                "sol": "#a88cf5",
                "terra": "#5dc98a",
                "luna": "#e6a23c",
                "astra": "#ef6a6a",
                "flat_width": 1,
                "rising_width": 3,
                "inferred_width": 1,
                "remaining_width": 3,
                "marker_size": 2,
                "flat_opacity": "0.95",
                "rising_opacity": "0.95",
                "inferred_opacity": "0.72",
            },
            "idle_geometry": idle_geometry,
            "models": models,
            "remaining": {
                "solid": _canonical_path(
                    remaining_segments,
                    "solid",
                    remaining_values,
                    period,
                    100,
                    True,
                ),
                "dashed": _canonical_path(
                    remaining_segments,
                    "dashed",
                    remaining_values,
                    period,
                    100,
                    True,
                ),
            },
            "remaining_markers": markers,
            "remaining_points": remaining_points,
        }
    return contracts


def _fetch(url: str) -> tuple[bytes, str]:
    parsed = urllib.parse.urlsplit(url)
    if (
        parsed.scheme != "http"
        or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}
        or parsed.username is not None
        or parsed.password is not None
        or parsed.fragment
    ):
        raise EvidenceError(f"{url} is not an uncredentialed loopback HTTP URL")
    try:
        port = parsed.port or 80
    except ValueError as error:
        raise EvidenceError(f"{url} has an invalid port") from error
    target = urllib.parse.urlunsplit(("", "", parsed.path or "/", parsed.query, ""))
    connection = http.client.HTTPConnection(parsed.hostname, port, timeout=10)
    try:
        # The product's deliberately narrow loopback parser rejects implicit
        # Accept-Encoding. http.client adds it unless this flag is explicit.
        connection.putrequest("GET", target, skip_accept_encoding=True)
        connection.putheader("Accept", "application/json")
        connection.endheaders()
        response = connection.getresponse()
        if response.status != 200:
            raise EvidenceError(f"{url} returned HTTP {response.status}")
        pairs = response.headers.get_all(PAIR_HEADER) or []
        if len(pairs) != 1 or not pairs[0]:
            raise EvidenceError(f"{url} did not return exactly one published pair")
        body = response.read(MAX_RESPONSE_BYTES + 1)
        if len(body) > MAX_RESPONSE_BYTES:
            raise EvidenceError(f"{url} exceeded the {MAX_RESPONSE_BYTES}-byte response bound")
        return body, pairs[0]
    except (OSError, http.client.HTTPException) as error:
        raise EvidenceError(f"{url} transport failed: {error}") from error
    finally:
        connection.close()


def _select_period(
    periods: list[dict[str, Any]],
    period_id: str | None,
    account_id: str | None = None,
) -> dict[str, Any]:
    account_id = _validate_account_id(account_id)
    if account_id is not None:
        for index, period in enumerate(periods):
            owner = period.get("account_id")
            if owner is not None and owner != account_id:
                raise EvidenceError(
                    f"period {index} belongs to a different account than {account_id}"
                )
    if period_id is not None:
        matches = [period for period in periods if period.get("id") == period_id]
        if len(matches) != 1:
            raise EvidenceError(
                f"expected one period with id {period_id!r}, found {len(matches)}"
            )
        return matches[0]
    current = [period for period in periods if period.get("current") is True]
    if len(current) != 1:
        raise EvidenceError(f"expected one current period, found {len(current)}")
    return current[0]


def capture(
    base_url: str,
    output_directory: Path,
    source_sha: str,
    period_id: str | None = None,
    account_id: str | None = None,
) -> Path:
    if not source_sha.isascii() or len(source_sha) != 40 or any(character not in "0123456789abcdef" for character in source_sha):
        raise EvidenceError("source SHA must be 40 lowercase hexadecimal characters")
    account_id = _validate_account_id(account_id)
    resolved_output = output_directory.resolve(strict=False)
    if resolved_output == REPOSITORY_ROOT or REPOSITORY_ROOT in resolved_output.parents:
        raise EvidenceError("evidence output directory must be outside the repository")
    if output_directory.exists():
        raise EvidenceError(f"output directory already exists: {output_directory}")
    output_directory.mkdir(mode=0o700, parents=False)
    inputs: list[tuple[str, int, bytes]] = []
    account_query = (
        "?" + urllib.parse.urlencode({"account": account_id})
        if account_id is not None
        else ""
    )
    periods_raw, pair = _fetch(f"{base_url.rstrip('/')}/v3/history/periods{account_query}")
    inputs.append(("periods", 0, periods_raw))
    periods_document = _json_loads(periods_raw)
    if set(periods_document) != {"api_version", "history_periods"} or periods_document["api_version"] != "v3":
        raise EvidenceError("periods response has an unexpected schema")
    periods = periods_document["history_periods"]
    if not isinstance(periods, list):
        raise EvidenceError("periods response history_periods is not an array")
    period = _select_period(periods, period_id, account_id)
    encoded_period = urllib.parse.quote(period["id"], safe="")
    samples: list[dict[str, Any]] = []
    gaps: list[dict[str, Any]] = []
    cursor: str | None = None
    resume_cursor: str | None = None
    for page_index in range(MAX_PAGES):
        query = [("period", period["id"])]
        if account_id is not None:
            query.insert(0, ("account", account_id))
        if cursor is not None:
            query.append(("cursor", cursor))
        suffix = "?" + urllib.parse.urlencode(query)
        raw, actual_pair = _fetch(f"{base_url.rstrip('/')}/v3/history{suffix}")
        if actual_pair != pair:
            raise EvidenceError("history page published pair differs from periods")
        inputs.append(("history", page_index, raw))
        page = _json_loads(raw)
        if set(page) != {"api_version", "history_samples", "history_gaps", "next_cursor", "resume_cursor"} or page["api_version"] != "v3":
            raise EvidenceError(f"history page {page_index} has an unexpected schema")
        samples.extend(page["history_samples"])
        if page_index == 0:
            gaps = page["history_gaps"]
        elif page["history_gaps"]:
            raise EvidenceError("a non-first history page repeated the gap set")
        resume_cursor = page["resume_cursor"]
        cursor = page["next_cursor"]
        if cursor is None:
            break
        if not isinstance(cursor, str) or not cursor:
            raise EvidenceError("history next_cursor is not a non-empty string or null")
    else:
        raise EvidenceError(f"history exceeded the {MAX_PAGES}-page bound")
    history_page = {
        "api_version": "v3",
        "history_samples": samples,
        "history_gaps": gaps,
        "next_cursor": None,
        "resume_cursor": resume_cursor,
    }
    fixture = {
        "account_id": account_id,
        "published_pair": pair,
        "period": period,
        "history_page": history_page,
    }
    expected_segments, expected_idle = build_expected(fixture)
    aggregate = hashlib.sha256()
    input_records = []
    for ordinal, (resource, page_index, body) in enumerate(inputs):
        aggregate.update(struct.pack(">Q", len(body)))
        aggregate.update(body)
        input_records.append(
            {
                "ordinal": ordinal,
                "resource": resource,
                "page_index": page_index,
                "byte_length": len(body),
                "sha256": hashlib.sha256(body).hexdigest(),
            }
        )
    artifact = {
        "schema_version": "graph-evidence-v1",
        "source_sha": source_sha,
        "account_id": account_id,
        "published_pair": pair,
        "period": {key: period[key] for key in ("id", "start_at", "end_at", "reset_at")},
        "inputs": input_records,
        "input_sha256": aggregate.hexdigest(),
        "fixture": fixture,
        "expected_segments": expected_segments,
        "expected_render_contracts": build_expected_render_contracts(fixture),
        "actual_projection_segments": [],
        "expected_idle_intervals": expected_idle,
        "actual_idle_intervals": {"linux": [], "windows": []},
        "unmatched_expected": {"linux": [], "windows": []},
        "unmatched_actual": {"linux": [], "windows": []},
        "cross_platform_mismatches": [],
    }
    through = samples[-1]["timestamp"]
    output = output_directory / f"live-v3-history-through-{through}.json"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(output, flags, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
        json.dump(artifact, stream, ensure_ascii=False, separators=(",", ":"))
    return output


def _actual_document(path: Path, platform: str, artifact: dict[str, Any]) -> dict[str, Any]:
    raw = path.read_bytes()
    if len(raw) > MAX_EVIDENCE_BYTES:
        raise EvidenceError(f"{platform} actual document exceeded the size bound")
    document = _json_loads(raw)
    required = {
        "schema_version",
        "source_sha",
        "input_sha256",
        "published_pair",
        "platform",
        "segments",
        "idle_intervals",
        "render_contracts",
    }
    if set(document) != required or document["schema_version"] != "graph-actual-v1":
        raise EvidenceError(f"{platform} actual document has an unexpected schema")
    for key in ("source_sha", "input_sha256", "published_pair"):
        if document[key] != artifact[key]:
            raise EvidenceError(f"{platform} actual {key} does not match captured evidence")
    if document["platform"] != platform:
        raise EvidenceError(f"actual platform is not {platform}")
    if not isinstance(document["segments"], list) or not isinstance(document["idle_intervals"], list):
        raise EvidenceError(f"{platform} actual projection arrays are invalid")
    if not isinstance(document["render_contracts"], dict):
        raise EvidenceError(f"{platform} actual render contracts are invalid")
    prior_key: tuple[Any, ...] | None = None
    seen_segments: set[tuple[Any, ...]] = set()
    for segment in document["segments"]:
        if not isinstance(segment, dict) or set(segment) != {"metric", "series", "start_at", "end_at", "style"}:
            raise EvidenceError(f"{platform} actual segment has an unexpected schema")
        metric, series, style = segment["metric"], segment["series"], segment["style"]
        start, end = segment["start_at"], segment["end_at"]
        allowed_styles = {"solid", "dashed"} if metric == "remaining" else {"flat", "rising", "dashed"}
        if (
            metric not in METRIC_RANK
            or not isinstance(series, str)
            or not series
            or style not in allowed_styles
            or isinstance(start, bool)
            or not isinstance(start, int)
            or isinstance(end, bool)
            or not isinstance(end, int)
            or end <= start
        ):
            raise EvidenceError(f"{platform} actual segment value is invalid")
        key = _segment_key(segment)
        if key in seen_segments or prior_key is not None and key < prior_key:
            raise EvidenceError(f"{platform} actual segments are duplicate or non-canonical")
        seen_segments.add(key)
        prior_key = key
    prior_idle: tuple[int, int] | None = None
    for interval in document["idle_intervals"]:
        if not isinstance(interval, dict) or set(interval) != {"start_at", "end_at"}:
            raise EvidenceError(f"{platform} actual idle interval has an unexpected schema")
        start, end = interval["start_at"], interval["end_at"]
        if (
            isinstance(start, bool)
            or not isinstance(start, int)
            or isinstance(end, bool)
            or not isinstance(end, int)
            or end <= start
            or prior_idle is not None and start <= prior_idle[1]
        ):
            raise EvidenceError(f"{platform} actual idle intervals are invalid or unmerged")
        prior_idle = (start, end)
    return document


def verify(evidence_path: Path, linux_path: Path, windows_path: Path) -> None:
    raw = evidence_path.read_bytes()
    if len(raw) > MAX_EVIDENCE_BYTES:
        raise EvidenceError("evidence document exceeded the size bound")
    artifact = _json_loads(raw)
    if artifact.get("schema_version") != "graph-evidence-v1":
        raise EvidenceError("evidence document schema_version is invalid")
    recomputed_segments, recomputed_idle = build_expected(artifact["fixture"])
    recomputed_render = build_expected_render_contracts(artifact["fixture"])
    if artifact["expected_segments"] != recomputed_segments or artifact["expected_idle_intervals"] != recomputed_idle:
        raise EvidenceError("captured expectations do not match the independent oracle")
    if artifact.get("expected_render_contracts") != recomputed_render:
        raise EvidenceError("captured render contract does not match the independent oracle")
    artifact_account_id = _validate_account_id(artifact.get("account_id"))
    fixture_account_id = _validate_account_id(artifact["fixture"].get("account_id"))
    if artifact_account_id != fixture_account_id:
        raise EvidenceError("captured fixture account id does not match its envelope")
    if artifact["fixture"].get("published_pair") != artifact.get("published_pair"):
        raise EvidenceError("captured fixture published pair does not match its envelope")
    expected = [
        {key: segment[key] for key in ("metric", "series", "start_at", "end_at", "style")}
        for segment in artifact["expected_segments"]
    ]
    actual_by_platform = {
        "linux": _actual_document(linux_path, "linux", artifact),
        "windows": _actual_document(windows_path, "windows", artifact),
    }
    combined: list[dict[str, Any]] = []
    comparison_keys = ("metric", "series", "start_at", "end_at", "style")
    expected_counter = Counter(tuple(item[key] for key in comparison_keys) for item in expected)
    for platform, document in actual_by_platform.items():
        actual = document["segments"]
        actual_counter = Counter(tuple(item[key] for key in comparison_keys) for item in actual)
        missing = expected_counter - actual_counter
        unexpected = actual_counter - expected_counter
        artifact["unmatched_expected"][platform] = []
        artifact["unmatched_actual"][platform] = []
        for item in expected:
            key = tuple(item[field] for field in comparison_keys)
            if missing[key] > 0:
                artifact["unmatched_expected"][platform].append(item)
                missing[key] -= 1
        for item in actual:
            key = tuple(item[field] for field in comparison_keys)
            if unexpected[key] > 0:
                artifact["unmatched_actual"][platform].append(item)
                unexpected[key] -= 1
        artifact["actual_idle_intervals"][platform] = document["idle_intervals"]
        combined.extend(dict(item, platform=platform) for item in actual)
    linux_styles = {
        tuple(item[key] for key in ("metric", "series", "start_at", "end_at")): item["style"]
        for item in actual_by_platform["linux"]["segments"]
    }
    windows_styles = {
        tuple(item[key] for key in ("metric", "series", "start_at", "end_at")): item["style"]
        for item in actual_by_platform["windows"]["segments"]
    }
    artifact["cross_platform_mismatches"] = [
        {
            "metric": key[0],
            "series": key[1],
            "start_at": key[2],
            "end_at": key[3],
            "linux_style": linux_styles.get(key),
            "windows_style": windows_styles.get(key),
        }
        for key in sorted(set(linux_styles) | set(windows_styles))
        if linux_styles.get(key) != windows_styles.get(key)
    ]
    artifact["actual_projection_segments"] = sorted(
        combined,
        key=lambda item: (item["platform"],) + _segment_key(item),
    )
    failures = (
        any(artifact["unmatched_expected"].values())
        or any(artifact["unmatched_actual"].values())
        or artifact["cross_platform_mismatches"]
        or any(
            actual_by_platform[platform]["render_contracts"]
            != artifact["expected_render_contracts"]
            for platform in ("linux", "windows")
        )
        or any(
            actual_by_platform[platform]["idle_intervals"] != artifact["expected_idle_intervals"]
            for platform in ("linux", "windows")
        )
    )
    evidence_path.write_text(
        json.dumps(artifact, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )
    if failures:
        raise EvidenceError("live graph projection differs from the independent oracle")


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    capture_parser = subparsers.add_parser("capture")
    capture_parser.add_argument("--base-url", default="http://127.0.0.1:8787")
    capture_parser.add_argument("--output-directory", type=Path, required=True)
    capture_parser.add_argument("--source-sha", required=True)
    capture_parser.add_argument("--period-id")
    capture_parser.add_argument("--account-id")
    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--evidence", type=Path, required=True)
    verify_parser.add_argument("--linux-actual", type=Path, required=True)
    verify_parser.add_argument("--windows-actual", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "capture":
            print(
                capture(
                    arguments.base_url,
                    arguments.output_directory,
                    arguments.source_sha,
                    arguments.period_id,
                    arguments.account_id,
                )
            )
        else:
            verify(arguments.evidence, arguments.linux_actual, arguments.windows_actual)
            print(arguments.evidence)
        return 0
    except (EvidenceError, KeyError, TypeError) as error:
        print(f"graph-live-evidence: FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

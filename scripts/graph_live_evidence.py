#!/usr/bin/env python3
"""Capture and independently verify live graph evidence for Issue #137.

The oracle intentionally does not import product code.  It turns the strict
v3 history wire into endpoint-pair line roles and token-based idle intervals.
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


class EvidenceError(RuntimeError):
    """A fail-closed capture or comparison error."""


@dataclass(frozen=True)
class ModelEvidence:
    value: float | None
    reliable: bool
    published: bool
    source: str
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


def _published_names(row: dict[str, Any]) -> frozenset[str]:
    if row.get("synthetic", False) or row["model_source"] == "unavailable":
        return frozenset()
    models = row["models"]
    if models is None:
        return frozenset()
    return frozenset(model["model"] for model in models if model.get("total_tokens") is not None)


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
        if source not in {"confirmed", "legacy-unknown", "unavailable"}:
            raise EvidenceError(f"sample {index} has an invalid model_source")
        complete = sample.get("models_complete")
        if not isinstance(complete, bool):
            raise EvidenceError(f"sample {index} models_complete is not boolean")
        models = sample.get("models")
        if source == "unavailable":
            if models is not None:
                raise EvidenceError(f"sample {index} unavailable source has models")
            continue
        if not isinstance(models, list):
            raise EvidenceError(f"sample {index} observed source has no model array")
        names: set[str] = set()
        for model_index, model in enumerate(models):
            name = model.get("model")
            if not isinstance(name, str) or not name or name in names:
                raise EvidenceError(f"sample {index} has an invalid/duplicate model name")
            names.add(name)
            tokens = _integer(model.get("total_tokens"), f"sample[{index}].models[{model_index}].total_tokens")
            dollars = _number(model.get("total_dollars"), f"sample[{index}].models[{model_index}].total_dollars")
            if tokens < 0 or dollars < 0:
                raise EvidenceError(f"sample {index} has a negative cumulative model value")
        if complete and source != "confirmed":
            raise EvidenceError(f"sample {index} complete model set is not confirmed")
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


def _raw_model_value(row: dict[str, Any], model: str, metric: str) -> tuple[float | None, bool]:
    if row.get("synthetic", False) or row["model_source"] == "unavailable":
        return None, False
    by_name = {item["model"]: item for item in row["models"]}
    if model in by_name:
        field = "total_tokens" if metric == "tokens" else "total_dollars"
        return float(by_name[model][field]), True
    if row["model_source"] == "confirmed" and row["models_complete"]:
        return 0.0, False
    return None, False


def _model_projection(
    rows: list[dict[str, Any]], model: str, metric: str
) -> list[ModelEvidence]:
    raw = [_raw_model_value(row, model, metric) for row in rows]
    isolated: set[int] = set()
    for index in range(1, len(rows) - 1):
        left, middle, right = raw[index - 1][0], raw[index][0], raw[index + 1][0]
        if (
            rows[index]["timestamp"] - rows[index - 1]["timestamp"] == 60
            and rows[index + 1]["timestamp"] - rows[index]["timestamp"] == 60
            and left is not None
            and middle is not None
            and right is not None
            and left <= right
            and (middle < left or middle > right)
        ):
            isolated.add(index)
    baseline: float | None = None
    result: list[ModelEvidence] = []
    for index, row in enumerate(rows):
        value, published = raw[index]
        if row.get("synthetic", False):
            result.append(ModelEvidence(baseline, False, False, row["model_source"], True))
        elif value is None:
            result.append(ModelEvidence(None, False, published, row["model_source"]))
        elif index in isolated or baseline is not None and value < baseline:
            result.append(ModelEvidence(baseline, False, published, row["model_source"]))
        else:
            baseline = value
            result.append(ModelEvidence(value, True, published, row["model_source"]))
    return result


def _model_interval(
    rows: list[dict[str, Any]],
    projections: dict[str, list[ModelEvidence]],
    before: int,
    after: int,
) -> tuple[bool, bool, list[str]]:
    start = rows[before]["timestamp"]
    end = rows[after]["timestamp"]
    if end - start > 60 or end <= start:
        return False, False, ["model_missing"]
    names_before = _published_names(rows[before])
    names_after = _published_names(rows[after])
    causes: list[str] = []
    if rows[before]["model_source"] == "unavailable" or rows[after]["model_source"] == "unavailable":
        causes.append("source_unavailable")
    if rows[before]["model_source"] != rows[after]["model_source"]:
        causes.append("source_mismatch")
    if names_before != names_after:
        causes.append("model_set_change")
    if not names_before or names_before != names_after:
        return False, False, causes or ["model_missing"]
    advanced = False
    for model in names_before:
        left = projections[model][before]
        right = projections[model][after]
        if left.value is None or right.value is None:
            causes.append("model_missing")
            return False, False, causes
        if not left.reliable or not right.reliable:
            causes.append("model_token_anomaly")
            return False, False, causes
        advanced |= right.value > left.value
    # A transition between two present observed source classes does not make
    # the listed model values unknown. It remains a cause annotation only
    # when another source/model failure already makes the interval dashed.
    causes = [cause for cause in causes if cause != "source_mismatch"]
    return True, advanced, causes


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

    def token_interval(before: int, after: int) -> tuple[bool, bool, list[str]]:
        if _hard_break(rows[before]["timestamp"], rows[after]["timestamp"], gaps):
            return False, False, ["confirmed_gap"]
        return _model_interval(rows, token_models, before, after)

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
            activity: list[tuple[int, bool]] = []
            weighted_seconds = 0
            for segment in range(left, run_end):
                available, advanced, _ = token_interval(segment, segment + 1)
                elapsed = rows[segment + 1]["timestamp"] - rows[segment]["timestamp"]
                weight = 0 if available and not advanced else elapsed
                weighted_seconds += weight
                activity.append((weight, not available))
            if weighted_seconds > 0:
                weighted_elapsed = 0
                for index in range(run_start, run_end):
                    weight, _ = activity[index - left - 1]
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
        activity: list[tuple[int, bool]] = []
        weighted_seconds = 0
        for segment in range(left, right):
            available, advanced, _ = token_interval(segment, segment + 1)
            elapsed = rows[segment + 1]["timestamp"] - rows[segment]["timestamp"]
            weight = 0 if available and not advanced else elapsed
            weighted_seconds += weight
            activity.append((weight, not available))
        if weighted_seconds <= 0:
            continue
        weighted_elapsed = 0
        for offset, index in enumerate(range(left + 1, right)):
            weight, inferred = activity[offset]
            weighted_elapsed += weight
            smoothed = values[left] + (values[right] - values[left]) * (
                weighted_elapsed / weighted_seconds
            )
            values[index] = smoothed
            if not (raw_reliable[index] and raw[index] == smoothed):
                origins[index] = (
                    "activity_smoothed"
                    if not inferred and raw_reliable[index]
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
        if not projection[previous].reliable or not point.reliable:
            causes.append("model_token_anomaly" if metric == "tokens" else "model_dollar_anomaly")
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
        available, advanced, model_causes = _model_interval(rows, token_models, left, right)
        if after.effective < before.effective and (not available or not advanced):
            causes.append("quota_unattributed")
            causes.extend(model_causes)
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
    remaining = {
        point.timestamp: point
        for point in _remaining_projection(period, rows, token_models, gaps)
    }

    def token_equal(before: int, after: int) -> bool:
        left_row, right_row = rows[before], rows[after]
        if (
            _hard_break(left_row["timestamp"], right_row["timestamp"], gaps)
        ):
            return False
        names = _published_names(left_row)
        if not names or names != _published_names(right_row):
            return False
        return all(
            token_models[name][before].reliable
            and token_models[name][after].reliable
            and token_models[name][before].value == token_models[name][after].value
            for name in names
        )

    def remaining_contradicts(start: int, end: int) -> bool:
        values = [
            point.raw
            for timestamp, point in remaining.items()
            if start <= timestamp <= end
            and point.origin in {"raw", "activity_smoothed"}
            and point.raw is not None
        ]
        return len(values) > 1 and any(value != values[0] for value in values[1:])

    intervals: list[tuple[int, int, int]] = []
    basic: set[tuple[int, int]] = set()
    for index in range(len(rows) - 1):
        elapsed = timestamps[index + 1] - timestamps[index]
        start, end = timestamps[index], timestamps[index + 1]
        if (
            0 < elapsed <= 60
            and token_equal(index, index + 1)
            and not remaining_contradicts(start, end)
        ):
            intervals.append((start, end, 1))
            basic.add((start, end))
    for index in range(len(rows) - 3):
        t0, t1, t2, t3 = timestamps[index : index + 4]
        if (
            t1 - t0 == 60
            and t2 - t1 == 120
            and t3 - t2 == 60
            and (t0, t1) in basic
            and (t2, t3) in basic
            and token_equal(index + 1, index + 2)
            and not remaining_contradicts(t1, t2)
        ):
            intervals.append((t1, t2, 0))
    merged: list[list[int]] = []
    for start, end, observed_count in sorted(intervals):
        if merged and start <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], end)
            merged[-1][2] += observed_count
        else:
            merged.append([start, end, observed_count])
    return [
        {"start_at": max(start, period["start_at"]), "end_at": min(end, period["end_at"])}
        for start, end, observed_count in merged
        if observed_count >= 2
        and min(end, period["end_at"]) > max(start, period["start_at"])
    ]


def build_expected(fixture: dict[str, Any]) -> tuple[list[dict[str, Any]], list[dict[str, int]]]:
    period, samples, gaps = _validate_fixture(fixture)
    rows = _rows_with_tail(period, samples)
    universe = sorted(
        {model["model"] for sample in samples for model in sample.get("models") or []},
        key=lambda value: value.encode("utf-8"),
    )
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


def capture(base_url: str, output_directory: Path, source_sha: str) -> Path:
    if not source_sha.isascii() or len(source_sha) != 40 or any(character not in "0123456789abcdef" for character in source_sha):
        raise EvidenceError("source SHA must be 40 lowercase hexadecimal characters")
    resolved_output = output_directory.resolve(strict=False)
    if resolved_output == REPOSITORY_ROOT or REPOSITORY_ROOT in resolved_output.parents:
        raise EvidenceError("evidence output directory must be outside the repository")
    if output_directory.exists():
        raise EvidenceError(f"output directory already exists: {output_directory}")
    output_directory.mkdir(mode=0o700, parents=False)
    inputs: list[tuple[str, int, bytes]] = []
    periods_raw, pair = _fetch(f"{base_url.rstrip('/')}/v3/history/periods")
    inputs.append(("periods", 0, periods_raw))
    periods_document = _json_loads(periods_raw)
    if set(periods_document) != {"api_version", "history_periods"} or periods_document["api_version"] != "v3":
        raise EvidenceError("periods response has an unexpected schema")
    current = [period for period in periods_document["history_periods"] if period.get("current") is True]
    if len(current) != 1:
        raise EvidenceError(f"expected one current period, found {len(current)}")
    period = current[0]
    encoded_period = urllib.parse.quote(period["id"], safe="")
    samples: list[dict[str, Any]] = []
    gaps: list[dict[str, Any]] = []
    cursor: str | None = None
    resume_cursor: str | None = None
    for page_index in range(MAX_PAGES):
        suffix = f"?period={encoded_period}"
        if cursor is not None:
            suffix += "&cursor=" + urllib.parse.quote(cursor, safe="")
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
    fixture = {"published_pair": pair, "period": period, "history_page": history_page}
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
        "published_pair": pair,
        "period": {key: period[key] for key in ("id", "start_at", "end_at", "reset_at")},
        "inputs": input_records,
        "input_sha256": aggregate.hexdigest(),
        "fixture": fixture,
        "expected_segments": expected_segments,
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
    required = {"schema_version", "source_sha", "input_sha256", "published_pair", "platform", "segments", "idle_intervals"}
    if set(document) != required or document["schema_version"] != "graph-actual-v1":
        raise EvidenceError(f"{platform} actual document has an unexpected schema")
    for key in ("source_sha", "input_sha256", "published_pair"):
        if document[key] != artifact[key]:
            raise EvidenceError(f"{platform} actual {key} does not match captured evidence")
    if document["platform"] != platform:
        raise EvidenceError(f"actual platform is not {platform}")
    if not isinstance(document["segments"], list) or not isinstance(document["idle_intervals"], list):
        raise EvidenceError(f"{platform} actual projection arrays are invalid")
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
    if artifact["expected_segments"] != recomputed_segments or artifact["expected_idle_intervals"] != recomputed_idle:
        raise EvidenceError("captured expectations do not match the independent oracle")
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
    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--evidence", type=Path, required=True)
    verify_parser.add_argument("--linux-actual", type=Path, required=True)
    verify_parser.add_argument("--windows-actual", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        if arguments.command == "capture":
            print(capture(arguments.base_url, arguments.output_directory, arguments.source_sha))
        else:
            verify(arguments.evidence, arguments.linux_actual, arguments.windows_actual)
            print(arguments.evidence)
        return 0
    except (EvidenceError, KeyError, TypeError) as error:
        print(f"graph-live-evidence: FAIL: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

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


SUSTAINED_UNUSED_MIN_DURATION_SECONDS = 10 * 60

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
    "terminal_unobserved",
)
CAUSE_RANK = {cause: index for index, cause in enumerate(CAUSE_ORDER)}
METRIC_RANK = {"remaining": 0, "tokens": 1, "dollars": 2}
# The wire model universe is intentionally open-ended so unknown models remain
# part of quota attribution and idle authority. The graph itself has four
# fixed colored model slots; unknown names are retained semantically but are
# not emitted as painted model paths.
RENDERABLE_MODELS = frozenset(("ASTRA", "LUNA", "SOL", "TERRA"))
# ASTRA is the only model whose history rows carry token components but no
# stored cumulative-dollar column.  Keep this oracle calculation byte-for-
# byte aligned with the production history projection; it is still derived
# evidence, never a replacement for a missing SOL/TERRA/LUNA dollar value.
ASTRA_PRICE_PER_MILLION = (10.0, 1.0, 12.5, 50.0)
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


def _float_bits(value: float) -> bytes:
    return struct.pack("!d", value)


def _direct_sampling_signature(row: dict[str, Any]) -> tuple[Any, ...] | None:
    remaining = row.get("remaining_percent")
    models = row.get("models")
    if (
        row.get("synthetic", False)
        or row.get("model_source") != "confirmed"
        or row.get("models_complete") is not True
        or not isinstance(remaining, (int, float))
        or isinstance(remaining, bool)
        or not math.isfinite(float(remaining))
        or not isinstance(models, list)
        or not models
    ):
        return None
    model_signature = tuple(
        (
            model["model"],
            model.get("total_tokens"),
            model.get("input_tokens"),
            model.get("cached_input_tokens"),
            model.get("cache_write_input_tokens"),
            model.get("output_tokens"),
        )
        for model in sorted(models, key=lambda item: item["model"].encode("utf-8"))
    )
    return row["reset_at"], _float_bits(float(remaining)), model_signature


def _without_recoverable_sampling_jitter(
    rows: list[dict[str, Any]],
    gaps: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    recovered: set[int] = set()
    for index in range(1, len(rows) - 1):
        left, middle, right = rows[index - 1 : index + 2]
        if (
            middle.get("synthetic", False)
            or middle.get("model_source") != "unavailable"
            or middle.get("models_complete") is not False
            or middle["timestamp"] - left["timestamp"] != 60
            or right["timestamp"] - middle["timestamp"] != 60
            or middle.get("reset_at") != left.get("reset_at")
            or right.get("reset_at") != left.get("reset_at")
            or _hard_break(left["timestamp"], right["timestamp"], gaps)
        ):
            continue
        left_signature = _direct_sampling_signature(left)
        right_signature = _direct_sampling_signature(right)
        if left_signature is None or left_signature != right_signature:
            continue
        middle_remaining = middle.get("remaining_percent")
        if middle_remaining is not None and (
            not isinstance(middle_remaining, (int, float))
            or isinstance(middle_remaining, bool)
            or not math.isfinite(float(middle_remaining))
            or _float_bits(float(middle_remaining)) != left_signature[1]
        ):
            continue
        recovered.add(index)
    return [row for index, row in enumerate(rows) if index not in recovered]


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
        if value is None and metric == "dollars" and model == "ASTRA":
            input_tokens = item.get("input_tokens")
            cached_tokens = item.get("cached_input_tokens")
            write_tokens = item.get("cache_write_input_tokens")
            output_tokens = item.get("output_tokens")
            if all(
                isinstance(token, int) and not isinstance(token, bool)
                for token in (
                    input_tokens,
                    cached_tokens,
                    write_tokens,
                    output_tokens,
                )
            ) and input_tokens >= cached_tokens + write_tokens:
                input_rate, cached_rate, write_rate, output_rate = ASTRA_PRICE_PER_MILLION
                ordinary_tokens = input_tokens - cached_tokens - write_tokens
                value = (
                    ordinary_tokens * input_rate
                    + cached_tokens * cached_rate
                    + write_tokens * write_rate
                    + output_tokens * output_rate
                ) / 1_000_000.0
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
            left is not None
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

    # A sparse interpolation can fall below a preceding legacy display value
    # even when both direct anchors are monotonic.  Keep the visible cumulative
    # line monotonic by holding that inferred point at the last display value;
    # it remains non-authoritative and is emitted as a dashed bridge.
    display_floor: float | None = None
    for index, point in enumerate(result):
        if point.value is None or not math.isfinite(point.value) or point.value < 0:
            continue
        if display_floor is not None and point.value < display_floor:
            if point.origin != "direct":
                result[index] = ModelEvidence(display_floor, False, "held", point.synthetic)
                point = result[index]
            else:
                result[index] = ModelEvidence(display_floor, False, "rejected", point.synthetic)
                point = result[index]
        display_floor = max(display_floor, point.value) if display_floor is not None else point.value
    return result


def _normalize_dollars_from_token_identity(
    rows: list[dict[str, Any]],
    model: str,
    dollars: list[ModelEvidence],
    tokens: list[ModelEvidence],
    idle_intervals: list[dict[str, int]],
) -> list[ModelEvidence]:
    """Correct only the presentation copy of a token-flat dollar run."""

    normalized = list(dollars)
    runs: list[tuple[str, list[int]]] = []
    run: list[int] = []
    run_identity: tuple[str, int] | None = None
    for index, (row, token) in enumerate(zip(rows, tokens, strict=True)):
        raw_model = next(
            (item for item in row.get("models") or [] if item.get("model") == model),
            None,
        )
        raw_tokens = None if raw_model is None else raw_model.get("total_tokens")
        legacy_idle = token.origin == "legacy" and any(
            interval["start_at"] <= row["timestamp"] <= interval["end_at"]
            for interval in idle_intervals
        )
        token_origin = (
            token.origin
            if ((token.origin == "direct" and token.reliable) or legacy_idle)
            and isinstance(raw_tokens, int)
            and not isinstance(raw_tokens, bool)
            and raw_tokens >= 0
            else None
        )
        identity = (
            (token_origin, raw_tokens)
            if token_origin is not None and isinstance(raw_tokens, int)
            else None
        )
        if identity is not None and identity == run_identity:
            run.append(index)
        elif identity is not None:
            if run:
                assert run_identity is not None
                runs.append((run_identity[0], run))
            run = [index]
            run_identity = identity
        else:
            if run:
                assert run_identity is not None
                runs.append((run_identity[0], run))
            run = []
            run_identity = None
    if run:
        assert run_identity is not None
        runs.append((run_identity[0], run))

    for run_origin, run in (
        candidate for candidate in runs if len(candidate[1]) >= 2
    ):
        baseline = next(
            (
                normalized[index].value
                for index in run
                if (
                    normalized[index].origin == run_origin
                    or run_origin == "legacy"
                )
                and normalized[index].value is not None
                and math.isfinite(normalized[index].value)
                and normalized[index].value >= 0
            ),
            None,
        )
        if baseline is None:
            continue
        for index in run:
            normalized[index] = ModelEvidence(
                baseline,
                run_origin == "direct",
                run_origin,
                normalized[index].synthetic,
            )
    return normalized


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


def _remaining_projection(
    _period: dict[str, Any],
    rows: list[dict[str, Any]],
    _token_models: dict[str, list[ModelEvidence]],
    _gaps: list[dict[str, Any]],
) -> list[RemainingEvidence]:
    raw = [None if row.get("synthetic", False) else row["remaining_percent"] for row in rows]
    isolated: set[int] = set()
    for index in range(1, len(rows) - 1):
        left, middle, right = raw[index - 1], raw[index], raw[index + 1]
        if (
            left is not None
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
        if (
            bounded
            and values[run_end] < values[left]
            and rows[run_end]["timestamp"] > rows[left]["timestamp"]
        ):
            elapsed = rows[run_end]["timestamp"] - rows[left]["timestamp"]
            if elapsed > 0:
                for index in range(run_start, run_end):
                    values[index] = values[left] + (values[run_end] - values[left]) * (
                        (rows[index]["timestamp"] - rows[left]["timestamp"]) / elapsed
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
    idle_intervals: list[dict[str, int]],
) -> list[dict[str, Any]]:
    segments: list[dict[str, Any]] = []

    def is_idle_timestamp(timestamp: int) -> bool:
        return any(
            interval["start_at"] <= timestamp <= interval["end_at"]
            for interval in idle_intervals
        )

    exact = [
        index
        for index, point in enumerate(projection)
        if point.value is not None
        and math.isfinite(point.value)
        and point.value >= 0
        and (
            (point.origin == "direct" and point.reliable)
            or point.origin == "legacy"
        )
    ]
    for previous, index in pairwise(exact):
        start, end = rows[previous]["timestamp"], rows[index]["timestamp"]
        confirmed_idle = _is_idle_interval(start, end, idle_intervals)
        causes: list[str] = []
        if _hard_break(start, end, gaps):
            causes.append("confirmed_gap")
        predicted = list(range(previous + 1, index))
        if predicted:
            causes.append("model_missing")
        interval_sources = {rows[offset]["model_source"] for offset in predicted}
        if "unavailable" in interval_sources:
            causes.append("source_unavailable")
        if any(projection[offset].origin == "rejected" for offset in predicted):
            causes.append("model_token_anomaly" if metric == "tokens" else "model_dollar_anomaly")
        previous_value = projection[previous].value
        current_value = projection[index].value
        assert previous_value is not None and current_value is not None
        if current_value < previous_value:
            causes.append("model_token_anomaly" if metric == "tokens" else "model_dollar_anomaly")
        style = (
            "dashed"
            if causes
            else "idle"
            if confirmed_idle
            else "flat"
            if previous_value == current_value
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
    if exact:
        last_anchor = exact[-1]
        last_renderable = next(
            (
                index
                for index in range(len(projection) - 1, last_anchor, -1)
                if projection[index].value is not None
                and math.isfinite(projection[index].value)
                and projection[index].value >= 0
            ),
            None,
        )
        if last_renderable is not None:
            start = rows[last_anchor]["timestamp"]
            end = rows[last_renderable]["timestamp"]
            causes = ["model_missing"]
            if _hard_break(start, end, gaps):
                causes.append("confirmed_gap")
            if rows[last_renderable].get("synthetic", False):
                causes.append("terminal_unobserved")
            segments.append(
                {
                    "metric": metric,
                    "series": model,
                    "start_at": start,
                    "end_at": end,
                    "style": "dashed",
                    "causes": _causes(causes),
                }
            )
    return segments


def _remaining_segments(
    _samples: list[dict[str, Any]],
    rows: list[dict[str, Any]],
    evidence: list[RemainingEvidence],
    _token_models: dict[str, list[ModelEvidence]],
    gaps: list[dict[str, Any]],
    idle_intervals: list[dict[str, int]],
) -> list[dict[str, Any]]:
    result: list[dict[str, Any]] = []
    anchors = [
        index
        for index, point in enumerate(evidence)
        if point.origin == "raw"
        and math.isfinite(point.effective)
    ]
    for left, right in pairwise(anchors):
        before = evidence[left]
        after = evidence[right]
        causes: list[str] = []
        if _hard_break(before.timestamp, after.timestamp, gaps):
            causes.append("confirmed_gap")
        predicted = evidence[left + 1 : right]
        measured = before.origin == "raw" and after.origin == "raw"
        if not measured or predicted:
            causes.append("remaining_missing")
        if after.effective > before.effective or any(
            point.origin == "monotonic_hold" for point in predicted
        ):
            causes.append("remaining_anomaly")
        result.append(
            {
                "metric": "remaining",
                "series": "remaining",
                "start_at": before.timestamp,
                "end_at": after.timestamp,
                "style": (
                    "dashed"
                    if causes
                    else "idle"
                    if before.effective == after.effective
                    and _is_idle_interval(before.timestamp, after.timestamp, idle_intervals)
                    else "solid"
                ),
                "causes": _causes(causes),
            }
        )
    if anchors:
        last_anchor = anchors[-1]
        if last_anchor + 1 < len(evidence):
            after = evidence[-1]
            if after.timestamp > evidence[last_anchor].timestamp:
                causes = ["remaining_missing"]
                if _hard_break(evidence[last_anchor].timestamp, after.timestamp, gaps):
                    causes.append("confirmed_gap")
                if after.origin == "synthetic_tail_hold":
                    causes.append("terminal_unobserved")
                result.append(
                    {
                        "metric": "remaining",
                        "series": "remaining",
                        "start_at": evidence[last_anchor].timestamp,
                        "end_at": after.timestamp,
                        "style": "dashed",
                        "causes": _causes(causes),
                    }
                )
    return result


def _idle_intervals(
    period: dict[str, Any],
    rows: list[dict[str, Any]],
    token_models: dict[str, list[ModelEvidence]],
    gaps: list[dict[str, Any]],
) -> list[dict[str, int]]:
    def raw_idle_origin(row: dict[str, Any], point: ModelEvidence) -> bool:
        if row.get("synthetic", False):
            return False
        if row.get("model_source") == "confirmed" and row.get("models_complete") is True:
            return point.origin == "direct" and point.reliable
        if row.get("model_source") == "legacy-unknown":
            return point.origin == "legacy" and point.value is not None
        return False

    remaining = {
        point.timestamp: point
        for point in _remaining_projection(period, rows, token_models, gaps)
    }
    intervals: list[tuple[int, int]] = []
    for index in range(len(rows) - 1):
        left = rows[index]
        right = rows[index + 1]
        start, end = left["timestamp"], right["timestamp"]
        if end <= start:
            continue
        if _hard_break(start, end, gaps):
            continue
        if (
            left.get("synthetic", False)
            or right.get("synthetic", False)
            or left.get("model_source") != right.get("model_source")
        ):
            continue
        left_names = frozenset(model["model"] for model in left.get("models") or [])
        right_names = frozenset(model["model"] for model in right.get("models") or [])
        if not left_names or left_names != right_names:
            continue
        if any(
            name not in token_models
            or index >= len(token_models[name])
            or index + 1 >= len(token_models[name])
            or not raw_idle_origin(left, token_models[name][index])
            or not raw_idle_origin(right, token_models[name][index + 1])
            or token_models[name][index].value != token_models[name][index + 1].value
            for name in left_names
        ):
            continue
        left_models = {model["model"]: model for model in left.get("models") or []}
        right_models = {model["model"]: model for model in right.get("models") or []}
        if any(
            not isinstance(left_models[name].get("total_tokens"), int)
            or isinstance(left_models[name].get("total_tokens"), bool)
            or left_models[name]["total_tokens"] < 0
            or not isinstance(right_models[name].get("total_tokens"), int)
            or isinstance(right_models[name].get("total_tokens"), bool)
            or right_models[name]["total_tokens"] < 0
            or left_models[name]["total_tokens"] != right_models[name]["total_tokens"]
            for name in left_names
        ):
            continue
        before = remaining.get(start)
        after = remaining.get(end)
        if (
            before is None
            or after is None
            or before.origin != "raw"
            or after.origin != "raw"
            or before.raw is None
            or after.raw is None
            or not math.isfinite(before.raw)
            or not math.isfinite(after.raw)
            or struct.pack("!d", before.raw) != struct.pack("!d", after.raw)
        ):
            continue
        intervals.append((start, end))

    merged: list[list[int]] = []
    for start, end in intervals:
        if merged and start == merged[-1][1]:
            merged[-1][1] = end
        else:
            merged.append([start, end])
    return [
        {"start_at": start, "end_at": end}
        for start, end in merged
        if end - start >= SUSTAINED_UNUSED_MIN_DURATION_SECONDS
    ]


def _is_idle_interval(
    start: int,
    end: int,
    idle_intervals: list[dict[str, int]],
) -> bool:
    return any(
        start >= interval["start_at"] and end <= interval["end_at"]
        for interval in idle_intervals
    )


def build_expected(fixture: dict[str, Any]) -> tuple[list[dict[str, Any]], list[dict[str, int]]]:
    period, samples, gaps = _validate_fixture(fixture)
    rows = _without_recoverable_sampling_jitter(_rows_with_tail(period, samples), gaps)
    universe = _period_model_universe(samples)
    renderable_universe = tuple(model for model in universe if model in RENDERABLE_MODELS)
    token_models = {
        model: _model_projection(rows, model, "tokens") for model in universe
    }
    idle = _idle_intervals(period, rows, token_models, gaps)
    dollar_models = {
        model: _normalize_dollars_from_token_identity(
            rows,
            model,
            _model_projection(rows, model, "dollars"),
            token_models[model],
            idle,
        )
        for model in universe
    }
    projections = {"tokens": token_models, "dollars": dollar_models}
    segments: list[dict[str, Any]] = []
    remaining = _remaining_projection(period, rows, token_models, gaps)
    segments.extend(_remaining_segments(samples, rows, remaining, token_models, gaps, idle))
    for metric in ("tokens", "dollars"):
        for model in renderable_universe:
            segments.extend(
                _model_segments(
                    rows,
                    model,
                    metric,
                    projections[metric][model],
                    gaps,
                    idle,
                )
            )
    segments.sort(key=_segment_key)
    return segments, idle


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


def _canonical_dashes_polyline(points: list[tuple[float, float]]) -> list[str]:
    result: list[str] = []
    dash, period = 0.45, 0.75
    epsilon = 1e-12
    phase = 0.0
    for start, end in pairwise(points):
        dx = end[0] - start[0]
        dy = end[1] - start[1]
        length = math.hypot(dx, dy)
        if not math.isfinite(length) or length <= epsilon:
            continue
        offset = 0.0
        while offset < length:
            in_dash = phase < dash
            phase_end = dash if in_dash else period
            advance = min(phase_end - phase, length - offset)
            if advance <= epsilon:
                phase = 0.0 if phase_end >= period else phase_end
                continue
            if in_dash:
                start_fraction = offset / length
                end_fraction = (offset + advance) / length
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
            offset += advance
            phase += advance
            if phase >= period - epsilon:
                phase = 0.0
            elif abs(phase - dash) <= epsilon:
                phase = dash
    return result


def _canonical_dashes(start: tuple[float, float], end: tuple[float, float]) -> list[str]:
    return _canonical_dashes_polyline([start, end])


def _segment_is_smoothable(
    segment: dict[str, Any],
    values: dict[int, float],
    remaining: bool,
) -> bool:
    if segment["style"] == "idle":
        return False
    causes = set(segment.get("causes", []))
    if "terminal_unobserved" in causes or any(
        cause in causes
        for cause in (
            "model_token_anomaly",
            "model_dollar_anomaly",
            "remaining_anomaly",
        )
    ):
        return False
    start = values.get(segment["start_at"])
    end = values.get(segment["end_at"])
    if start is None or end is None or not math.isfinite(start) or not math.isfinite(end):
        return False
    return end <= start if remaining else end >= start


def _canonical_curve_interval(
    timestamps: list[int],
    raw_values: list[float],
    interval: int,
    period: dict[str, Any],
    maximum: float,
    remaining: bool,
) -> list[tuple[float, float]]:
    period_span = max(1, period["end_at"] - period["start_at"])
    viewbox_width = (
        abs(timestamps[interval + 1] - timestamps[interval])
        / period_span
        * 100.0
    )
    steps = max(1, math.ceil(viewbox_width / 0.25))
    fractions = [step / steps for step in range(steps + 1)]
    projected = _monotone_cubic_interval_values(
        [float(timestamp) for timestamp in timestamps],
        raw_values,
        interval,
        fractions,
    )
    return [
        _canonical_coordinate(
            timestamps[interval]
            + (timestamps[interval + 1] - timestamps[interval]) * fraction,
            projected[step],
            period["start_at"],
            period["end_at"],
            maximum,
            remaining,
        )
        for step, fraction in enumerate(fractions)
    ]


def _monotone_cubic_slopes(x: list[float], y: list[float]) -> list[float]:
    if (
        len(x) != len(y)
        or len(x) < 2
        or any(not math.isfinite(value) for value in [*x, *y])
        or any(right <= left for left, right in pairwise(x))
    ):
        raise ValueError("monotone cubic anchors must be finite and strictly ordered")
    widths = [right - left for left, right in pairwise(x)]
    deltas = [
        (right - left) / width
        for (left, right), width in zip(pairwise(y), widths, strict=True)
    ]
    if len(x) == 2:
        return [deltas[0], deltas[0]]

    def endpoint(width: float, next_width: float, delta: float, next_delta: float) -> float:
        candidate = ((2.0 * width + next_width) * delta - width * next_delta) / (
            width + next_width
        )
        if candidate * delta <= 0.0:
            return 0.0
        if delta * next_delta < 0.0 and abs(candidate) > 3.0 * abs(delta):
            return 3.0 * delta
        return candidate

    slopes = [0.0] * len(x)
    slopes[0] = endpoint(widths[0], widths[1], deltas[0], deltas[1])
    for index in range(1, len(x) - 1):
        before = deltas[index - 1]
        after = deltas[index]
        if before * after <= 0.0:
            slopes[index] = 0.0
            continue
        before_width = widths[index - 1]
        after_width = widths[index]
        first_weight = 2.0 * after_width + before_width
        second_weight = after_width + 2.0 * before_width
        slopes[index] = (first_weight + second_weight) / (
            first_weight / before + second_weight / after
        )
    slopes[-1] = endpoint(widths[-1], widths[-2], deltas[-1], deltas[-2])
    return slopes


def _monotone_cubic_interval_values(
    x: list[float],
    y: list[float],
    interval: int,
    fractions: list[float],
) -> list[float]:
    slopes = _monotone_cubic_slopes(x, y)
    if interval < 0 or interval + 1 >= len(x):
        raise ValueError("monotone cubic interval is outside the anchor range")
    if any(not math.isfinite(fraction) or not 0.0 <= fraction <= 1.0 for fraction in fractions):
        raise ValueError("monotone cubic fractions must be finite and within zero to one")
    width = x[interval + 1] - x[interval]
    result: list[float] = []
    for fraction in fractions:
        squared = fraction * fraction
        cubed = squared * fraction
        result.append(
            (2.0 * cubed - 3.0 * squared + 1.0) * y[interval]
            + (cubed - 2.0 * squared + fraction) * width * slopes[interval]
            + (-2.0 * cubed + 3.0 * squared) * y[interval + 1]
            + (cubed - squared) * width * slopes[interval + 1]
        )
    return result


def _sampling_smoothed_values(
    timestamps: list[int],
    values: list[float],
    preserved_timestamps: set[int],
    preserved_indices: set[int],
) -> list[float]:
    if len(timestamps) != len(values) or len(values) < 3:
        return list(values)
    last = len(values) - 1
    knots = [
        index
        for index in range(len(values))
        if index in {0, last}
        or timestamps[index] in preserved_timestamps
        or index in preserved_indices
        or values[index] != values[index - 1]
        and values[index] != values[index + 1]
    ]
    knot_timestamps = [float(timestamps[index]) for index in knots]
    knot_values = [values[index] for index in knots]
    if len(knots) < 2:
        return list(values)
    smoothed: list[float] = []
    for timestamp in timestamps:
        if timestamp in (timestamps[index] for index in knots):
            smoothed.append(knot_values[knot_timestamps.index(float(timestamp))])
            continue
        right = next(
            (index for index, candidate in enumerate(knot_timestamps) if candidate > timestamp),
            len(knot_timestamps),
        )
        if right == 0:
            smoothed.append(knot_values[0])
        elif right >= len(knot_timestamps):
            smoothed.append(knot_values[-1])
        else:
            left = right - 1
            fraction = (timestamp - knot_timestamps[left]) / (
                knot_timestamps[right] - knot_timestamps[left]
            )
            smoothed.append(
                _monotone_cubic_interval_values(
                    knot_timestamps,
                    knot_values,
                    left,
                    [fraction],
                )[0]
            )
    return smoothed


def _canonical_smooth_path(
    segments: list[dict[str, Any]],
    style: str,
    values: dict[int, float],
    period: dict[str, Any],
    maximum: float,
    remaining: bool,
    idle_intervals: list[dict[str, int]],
) -> str:
    commands: list[str] = []
    last_end: tuple[float, float] | None = None

    def append_solid(start: tuple[float, float], end: tuple[float, float]) -> None:
        nonlocal last_end
        if last_end == start:
            commands.append(f"L{end[0]:.2f} {end[1]:.2f}")
        else:
            commands.append(_canonical_segment(start, end))
        last_end = end

    run_start = 0
    while run_start < len(segments):
        if not _segment_is_smoothable(segments[run_start], values, remaining):
            segment = segments[run_start]
            if segment["style"] == style:
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
                if style == "idle":
                    end = (end[0], start[1])
                if style == "dashed":
                    commands.extend(_canonical_dashes(start, end))
                else:
                    append_solid(start, end)
            elif style != "dashed":
                last_end = None
            run_start += 1
            continue
        run_end = run_start + 1
        while (
            run_end < len(segments)
            and _segment_is_smoothable(segments[run_end], values, remaining)
            and segments[run_end]["start_at"] == segments[run_end - 1]["end_at"]
        ):
            run_end += 1
        run = segments[run_start:run_end]
        timestamps = [run[0]["start_at"], *[segment["end_at"] for segment in run]]
        raw_values = [values[timestamp] for timestamp in timestamps]
        preserved_timestamps = {
            timestamp
            for interval in idle_intervals
            for timestamp in (interval["start_at"], interval["end_at"])
        }
        preserved_indices = {
            index
            for interval, segment in enumerate(run)
            if segment["style"] == "dashed"
            for index in (interval, interval + 1)
        }
        smoothed_values = _sampling_smoothed_values(
            timestamps,
            raw_values,
            preserved_timestamps,
            preserved_indices,
        )
        last_end = None
        for interval, segment in enumerate(run):
            if segment["style"] != style:
                if style != "dashed":
                    last_end = None
                continue
            points = _canonical_curve_interval(
                timestamps,
                smoothed_values,
                interval,
                period,
                maximum,
                remaining,
            )
            if style == "dashed":
                commands.extend(_canonical_dashes_polyline(points))
            else:
                for start, end in pairwise(points):
                    append_solid(start, end)
        run_start = run_end
    return " ".join(commands)


def _canonical_path(
    segments: list[dict[str, Any]],
    style: str,
    values: dict[int, float],
    period: dict[str, Any],
    maximum: float,
    remaining: bool,
    idle_intervals: list[dict[str, int]],
) -> str:
    return _canonical_smooth_path(
        segments,
        style,
        values,
        period,
        maximum,
        remaining,
        idle_intervals,
    )


def _remaining_markers(
    _evidence: list[RemainingEvidence],
    _period: dict[str, Any],
) -> list[dict[str, Any]]:
    # The smooth measured quota path is the sole trajectory. Integer-boundary
    # dots sampled from raw rows would visually create a second quota line.
    return []


def _f32(value: float) -> float:
    return struct.unpack("!f", struct.pack("!f", value))[0]


def _native_graph_y(value: float, maximum: float) -> float:
    return _f32(
        min(0.99, max(0.01, (99.0 - value / max(1.0, maximum) * 98.0) / 100.0))
    )


def _format_token_count(value: float) -> str:
    return f"{math.floor(max(0.0, value) + 0.5):,}"


def _json_number(value: float) -> int | float:
    """Use one JSON spelling for integral values across both renderers."""

    if math.isfinite(value) and value.is_integer() and -(2**53) < value < 2**53:
        return int(value)
    return value


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

    # Match GraphPlotProjection.BuildEndpointLabels exactly. NativeGraphY is
    # quantized to float, but ArrangeEndpointLabelTops uses doubles and only
    # the final arranged center is quantized to float. Quantizing the
    # intermediate arithmetic causes one-ULP disagreements for tight labels.
    half = 8.0 / 204.0
    label_height = 16.0 / 204.0
    separation = label_height
    lower = 0.0
    upper = 1.0
    maximum_top = max(lower, upper - label_height)
    ideal_tops = [item["point_y"] - half for item in candidates]
    label_tops = [min(maximum_top, max(lower, ideal)) for ideal in ideal_tops]
    for index in range(1, len(label_tops)):
        label_tops[index] = max(
            label_tops[index], label_tops[index - 1] + separation
        )
    if label_tops[-1] > maximum_top:
        label_tops[-1] = maximum_top
        for index in range(len(label_tops) - 2, -1, -1):
            label_tops[index] = min(
                label_tops[index], label_tops[index + 1] - separation
            )
    if label_tops[0] < lower:
        shift = lower - label_tops[0]
        label_tops = [value + shift for value in label_tops]
    label_y = [_f32(top + half) for top in label_tops]

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
    rows = _without_recoverable_sampling_jitter(_rows_with_tail(period, samples), gaps)
    universe = _period_model_universe(samples)
    renderable_universe = tuple(model for model in universe if model in RENDERABLE_MODELS)
    token_models = {
        model: _model_projection(rows, model, "tokens") for model in universe
    }
    idle = _idle_intervals(period, rows, token_models, gaps)
    dollar_models = {
        model: _normalize_dollars_from_token_identity(
            rows,
            model,
            _model_projection(rows, model, "dollars"),
            token_models[model],
            idle,
        )
        for model in universe
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
        idle,
    )
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
        projections = token_models if metric == "tokens" else dollar_models
        renderable_projections = {
            model: projections[model] for model in renderable_universe
        }
        finite_values = [
            point.value
            for projection in renderable_projections.values()
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
        for model in renderable_universe:
            segments = _model_segments(
                rows,
                model,
                metric,
                projections[model],
                gaps,
                idle,
            )
            values = {
                row["timestamp"]: point.value
                for row, point in zip(rows, projections[model], strict=True)
                if point.value is not None
            }
            models.append(
                {
                    "series": model,
                    "idle": _canonical_path(
                        segments, "idle", values, period, maximum, False, idle
                    ),
                    "flat": _canonical_path(
                        segments, "flat", values, period, maximum, False, idle
                    ),
                    "rising": _canonical_path(
                        segments, "rising", values, period, maximum, False, idle
                    ),
                    "dashed": _canonical_path(
                        segments, "dashed", values, period, maximum, False, idle
                    ),
                }
            )
        endpoint_values = [
            {
                "series": model,
                "timestamp": period["end_at"],
                "value": (
                    None
                    if projections[model][-1].value is None
                    else _json_number(projections[model][-1].value)
                ),
            }
            for model in renderable_universe
        ]
        endpoint_values.append(
            {
                "series": "remaining",
                "timestamp": period["end_at"],
                "value": _json_number(remaining_evidence[-1].effective),
            }
        )
        contracts[metric] = {
            "viewbox": [100, 100],
            "model_maximum": f"{maximum:.12f}",
            "time_ticks": time_ticks,
            "axis_labels": axis_labels,
            "axis_grid_y": [f"{fraction:.12f}" for fraction in (0.0, 0.25, 0.5, 0.75, 1.0)],
            "endpoint_labels": _endpoint_labels(
                renderable_projections,
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
                "idle_width": 1,
                "flat_width": 3,
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
                "idle": _canonical_path(
                    remaining_segments,
                    "idle",
                    remaining_values,
                    period,
                    100,
                    True,
                    idle,
                ),
                "solid": _canonical_path(
                    remaining_segments,
                    "solid",
                    remaining_values,
                    period,
                    100,
                    True,
                    idle,
                ),
                "dashed": _canonical_path(
                    remaining_segments,
                    "dashed",
                    remaining_values,
                    period,
                    100,
                    True,
                    idle,
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
        allowed_styles = (
            {"idle", "solid", "dashed"}
            if metric == "remaining"
            else {"idle", "flat", "rising", "dashed"}
        )
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

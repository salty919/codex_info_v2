#!/usr/bin/env python3
"""Fetch and append official Standard short-context model prices."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import sys
import tempfile
import time
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any, Callable
from urllib import request
from urllib.error import URLError
from urllib.parse import urljoin, urlparse


SOURCE_PAGE_URL = "https://developers.openai.com/api/docs/pricing"
MARKDOWN_URL = SOURCE_PAGE_URL + ".md"
STANDARD_HEADING = "### Standard pricing data"
MAX_SOURCE_BYTES = 2 * 1024 * 1024
FETCH_TIMEOUT_SECONDS = 20
DEFAULT_SNAPSHOT = Path("crates/codex-info-pricing/data/standard-short.json")
BASIS = "standard-short"
CURRENCY = "USD"
UNIT_TOKENS = 1_000_000

MODEL_IDS = (
    "gpt-6-astra",
    "gpt-6.1-sol",
    "gpt-6-luna",
    "gpt-6-sol",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
)
RATE_FIELDS = ("input", "cached_input", "cache_write_input", "output")
EXPECTED_HEADERS = (
    "Model",
    "Short context input",
    "Short context cached input",
    "Short context cache writes",
    "Short context output",
    "Long context input",
    "Long context cached input",
    "Long context cache writes",
    "Long context output",
)
SNAPSHOT_KEYS = {
    "schema_version",
    "source",
    "basis",
    "currency",
    "unit_tokens",
    "revisions",
}
REVISION_KEYS = {"id", "observed_at", "models"}
PRICE_PATTERN = re.compile(
    r"\$\s*((?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?|\.\d+)\s*"
)


class PricingUpdateError(Exception):
    """A fail-closed source, schema, or snapshot error."""


def _same_official_https_origin(url: str) -> bool:
    parsed = urlparse(url)
    return (
        parsed.scheme == "https"
        and parsed.hostname is not None
        and parsed.hostname.lower() == "developers.openai.com"
        and parsed.port in (None, 443)
        and parsed.username is None
        and parsed.password is None
    )


class OfficialRedirectHandler(request.HTTPRedirectHandler):
    """Allow redirects only within the official OpenAI documentation origin."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        target = urljoin(req.full_url, newurl)
        if not _same_official_https_origin(target):
            raise PricingUpdateError("pricing source redirected outside the official HTTPS origin")
        return super().redirect_request(req, fp, code, msg, headers, target)


def fetch_standard_markdown() -> str:
    """Fetch the fixed official Markdown endpoint once with bounded I/O."""

    if not _same_official_https_origin(MARKDOWN_URL):
        raise PricingUpdateError("configured pricing source is not the official HTTPS origin")
    opener = request.build_opener(OfficialRedirectHandler())
    request_object = request.Request(
        MARKDOWN_URL,
        headers={"User-Agent": "codex-info-model-pricing/1.0", "Accept": "text/markdown"},
    )
    try:
        with opener.open(request_object, timeout=FETCH_TIMEOUT_SECONDS) as response:
            status = getattr(response, "status", None)
            if status is None:
                status = response.getcode()
            if status != 200:
                raise PricingUpdateError(f"pricing source returned HTTP {status}")
            if not _same_official_https_origin(response.geturl()):
                raise PricingUpdateError("pricing source response has a non-official final URL")
            body = response.read(MAX_SOURCE_BYTES + 1)
    except PricingUpdateError:
        raise
    except (OSError, URLError, TimeoutError) as exc:
        raise PricingUpdateError(f"could not fetch official pricing Markdown: {exc}") from exc
    if len(body) > MAX_SOURCE_BYTES:
        raise PricingUpdateError("official pricing Markdown exceeds the 2 MiB limit")
    try:
        return body.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise PricingUpdateError("official pricing Markdown is not valid UTF-8") from exc


def _split_table_row(line: str) -> list[str] | None:
    stripped = line.strip()
    if not (stripped.startswith("|") and stripped.endswith("|")):
        return None
    return [cell.strip() for cell in stripped[1:-1].split("|")]


def _is_separator_row(cells: list[str]) -> bool:
    return len(cells) == len(EXPECTED_HEADERS) and all(
        re.fullmatch(r":?-{3,}:?", cell) is not None for cell in cells
    )


def _parse_price(cell: str, model_id: str, field: str) -> float:
    match = PRICE_PATTERN.fullmatch(cell.strip())
    if match is None:
        raise PricingUpdateError(
            f"official Standard price for {model_id} {field} is not a USD amount"
        )
    try:
        amount = Decimal(match.group(1).replace(",", ""))
    except InvalidOperation as exc:
        raise PricingUpdateError(
            f"official Standard price for {model_id} {field} is invalid"
        ) from exc
    value = float(amount)
    if not amount.is_finite() or amount <= 0 or not math.isfinite(value) or value <= 0:
        raise PricingUpdateError(
            f"official Standard price for {model_id} {field} must be finite and positive"
        )
    return value


def parse_standard_pricing(markdown: str) -> dict[str, dict[str, float]]:
    """Parse only the exact Standard pricing data table and its short-context cells."""

    lines = markdown.splitlines()
    heading_positions = [i for i, line in enumerate(lines) if line.strip() == STANDARD_HEADING]
    if len(heading_positions) != 1:
        raise PricingUpdateError(
            f"expected exactly one {STANDARD_HEADING!r} section, found {len(heading_positions)}"
        )

    section_start = heading_positions[0] + 1
    section_end = len(lines)
    for i in range(section_start, len(lines)):
        if re.match(r"^#{1,6}\s", lines[i].lstrip()):
            section_end = i
            break
    section = lines[section_start:section_end]

    matching_headers = [
        i for i, line in enumerate(section) if _split_table_row(line) == list(EXPECTED_HEADERS)
    ]
    if len(matching_headers) != 1:
        raise PricingUpdateError(
            f"expected exactly one 9-column Standard pricing table, found {len(matching_headers)}"
        )
    header_index = matching_headers[0]
    if header_index + 1 >= len(section):
        raise PricingUpdateError("Standard pricing table is missing its separator row")
    separator = _split_table_row(section[header_index + 1])
    if separator is None or not _is_separator_row(separator):
        raise PricingUpdateError("Standard pricing table has an invalid separator row")

    parsed: dict[str, dict[str, float]] = {}
    for line in section[header_index + 2 :]:
        cells = _split_table_row(line)
        if cells is None:
            if "|" in line:
                raise PricingUpdateError("Standard pricing table contains a malformed row")
            if line.strip():
                break
            continue
        if len(cells) != len(EXPECTED_HEADERS):
            raise PricingUpdateError("Standard pricing table row has an unexpected column count")
        model_id = cells[0]
        if model_id not in MODEL_IDS:
            continue
        if model_id in parsed:
            raise PricingUpdateError(f"official Standard table has duplicate model {model_id}")
        parsed[model_id] = {
            "input": _parse_price(cells[1], model_id, "input"),
            "cached_input": _parse_price(cells[2], model_id, "cached_input"),
            "cache_write_input": _parse_price(cells[3], model_id, "cache_write_input"),
            "output": _parse_price(cells[4], model_id, "output"),
        }

    missing = [model_id for model_id in MODEL_IDS if model_id not in parsed]
    if missing:
        raise PricingUpdateError(
            "official Standard table is missing target models: " + ", ".join(missing)
        )
    return {model_id: parsed[model_id] for model_id in MODEL_IDS}


def _normalized_rates(rates: dict[str, Any]) -> dict[str, dict[str, float]]:
    if not isinstance(rates, dict) or set(rates) != set(MODEL_IDS):
        raise PricingUpdateError("pricing rates must contain exactly the seven target model IDs")
    normalized: dict[str, dict[str, float]] = {}
    for model_id in MODEL_IDS:
        row = rates[model_id]
        if not isinstance(row, dict) or set(row) != set(RATE_FIELDS):
            raise PricingUpdateError(f"pricing rates for {model_id} have an invalid field set")
        normalized_row: dict[str, float] = {}
        for field in RATE_FIELDS:
            value = row[field]
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise PricingUpdateError(f"pricing rate for {model_id} {field} is not numeric")
            try:
                numeric = float(value)
            except (OverflowError, ValueError) as exc:
                raise PricingUpdateError(
                    f"pricing rate for {model_id} {field} is outside the supported range"
                ) from exc
            if not math.isfinite(numeric) or numeric <= 0:
                raise PricingUpdateError(
                    f"pricing rate for {model_id} {field} must be finite and positive"
                )
            normalized_row[field] = numeric
        normalized[model_id] = normalized_row
    return normalized


def _reject_duplicate_json_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise PricingUpdateError(f"snapshot contains duplicate JSON key {key!r}")
        value[key] = item
    return value


def _validate_snapshot(snapshot: Any) -> dict[str, Any]:
    if not isinstance(snapshot, dict) or set(snapshot) != SNAPSHOT_KEYS:
        raise PricingUpdateError("price snapshot has an invalid top-level schema")
    if (
        isinstance(snapshot["schema_version"], bool)
        or not isinstance(snapshot["schema_version"], int)
        or snapshot["schema_version"] != 1
    ):
        raise PricingUpdateError("price snapshot has an unsupported schema_version")
    if snapshot["source"] != SOURCE_PAGE_URL:
        raise PricingUpdateError("price snapshot source does not match the official pricing page")
    if snapshot["basis"] != BASIS or snapshot["currency"] != CURRENCY:
        raise PricingUpdateError("price snapshot basis or currency is invalid")
    if (
        isinstance(snapshot["unit_tokens"], bool)
        or not isinstance(snapshot["unit_tokens"], int)
        or snapshot["unit_tokens"] != UNIT_TOKENS
    ):
        raise PricingUpdateError("price snapshot unit_tokens is invalid")
    revisions = snapshot["revisions"]
    if not isinstance(revisions, list):
        raise PricingUpdateError("price snapshot revisions must be an array")

    ids: set[str] = set()
    previous_observed_at = 0
    for revision in revisions:
        if not isinstance(revision, dict) or set(revision) != REVISION_KEYS:
            raise PricingUpdateError("price snapshot revision has an invalid schema")
        revision_id = revision["id"]
        observed_at = revision["observed_at"]
        if not isinstance(revision_id, str) or not revision_id:
            raise PricingUpdateError("price snapshot revision id must be a non-empty string")
        if revision_id in ids:
            raise PricingUpdateError("price snapshot revision ids must be unique")
        if isinstance(observed_at, bool) or not isinstance(observed_at, int) or observed_at <= 0:
            raise PricingUpdateError("price snapshot observed_at must be a positive Unix timestamp")
        if observed_at <= previous_observed_at:
            raise PricingUpdateError("price snapshot observed_at values must be strictly ascending")
        _normalized_rates(revision["models"])
        ids.add(revision_id)
        previous_observed_at = observed_at
    return snapshot


def _canonical_price_hash(rates: dict[str, dict[str, float]]) -> str:
    encoded = json.dumps(
        rates,
        ensure_ascii=False,
        separators=(",", ":"),
        allow_nan=False,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _read_snapshot(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {
            "schema_version": 1,
            "source": SOURCE_PAGE_URL,
            "basis": BASIS,
            "currency": CURRENCY,
            "unit_tokens": UNIT_TOKENS,
            "revisions": [],
        }
    try:
        raw = path.read_text(encoding="utf-8")
        snapshot = json.loads(raw, object_pairs_hook=_reject_duplicate_json_keys)
    except PricingUpdateError:
        raise
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise PricingUpdateError(f"could not read existing price snapshot: {exc}") from exc
    return _validate_snapshot(snapshot)


def _atomic_write(path: Path, snapshot: dict[str, Any]) -> None:
    if not path.parent.is_dir():
        raise PricingUpdateError(f"snapshot directory does not exist: {path.parent}")
    serialized = json.dumps(snapshot, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    temporary_path: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            newline="\n",
            prefix=f".{path.name}.",
            suffix=".tmp",
            dir=path.parent,
            delete=False,
        ) as temporary:
            temporary_path = temporary.name
            temporary.write(serialized)
            temporary.flush()
            os.fsync(temporary.fileno())
        os.replace(temporary_path, path)
        temporary_path = None
        if hasattr(os, "O_DIRECTORY"):
            directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(directory_fd)
            finally:
                os.close(directory_fd)
    except OSError as exc:
        raise PricingUpdateError(f"could not atomically write price snapshot: {exc}") from exc
    finally:
        if temporary_path is not None:
            try:
                os.unlink(temporary_path)
            except FileNotFoundError:
                pass


def update_snapshot(
    snapshot_path: str | os.PathLike[str],
    rates: dict[str, Any],
    observed_at: int | None = None,
) -> dict[str, str]:
    """Append one immutable revision when rates differ from the latest revision."""

    normalized = _normalized_rates(rates)
    price_hash = _canonical_price_hash(normalized)
    path = Path(snapshot_path)
    snapshot = _read_snapshot(path)
    revisions = snapshot["revisions"]

    if revisions and _normalized_rates(revisions[-1]["models"]) == normalized:
        return {
            "status": "unchanged",
            "price_hash": price_hash,
            "revision_id": revisions[-1]["id"],
        }

    timestamp = int(time.time()) if observed_at is None else observed_at
    if isinstance(timestamp, bool) or not isinstance(timestamp, int) or timestamp <= 0:
        raise PricingUpdateError("observed_at must be a positive Unix timestamp")
    if revisions and timestamp <= revisions[-1]["observed_at"]:
        raise PricingUpdateError("new observed_at must be later than the latest revision")

    revision_id = f"{timestamp}-{price_hash[:16]}"
    if any(revision["id"] == revision_id for revision in revisions):
        raise PricingUpdateError("new price revision id already exists")
    updated = dict(snapshot)
    updated["revisions"] = [
        *revisions,
        {"id": revision_id, "observed_at": timestamp, "models": normalized},
    ]
    _validate_snapshot(updated)
    _atomic_write(path, updated)
    return {"status": "changed", "price_hash": price_hash, "revision_id": revision_id}


def update_from_markdown(
    snapshot_path: str | os.PathLike[str], markdown: str, observed_at: int | None = None
) -> dict[str, str]:
    return update_snapshot(snapshot_path, parse_standard_pricing(markdown), observed_at)


def update_from_source(
    snapshot_path: str | os.PathLike[str],
    fetcher: Callable[[], str] | None = None,
    observed_at: int | None = None,
) -> dict[str, str]:
    try:
        markdown = (fetch_standard_markdown if fetcher is None else fetcher)()
    except PricingUpdateError:
        raise
    except Exception as exc:
        raise PricingUpdateError(f"could not obtain official pricing Markdown: {exc}") from exc
    return update_from_markdown(snapshot_path, markdown, observed_at)


def _read_fixture(path: Path) -> str:
    try:
        with path.open("rb") as source:
            content = source.read(MAX_SOURCE_BYTES + 1)
    except OSError as exc:
        raise PricingUpdateError(f"could not read offline Markdown fixture: {exc}") from exc
    if len(content) > MAX_SOURCE_BYTES:
        raise PricingUpdateError("offline Markdown fixture exceeds the 2 MiB limit")
    try:
        return content.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise PricingUpdateError("offline Markdown fixture is not valid UTF-8") from exc


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", type=Path, default=DEFAULT_SNAPSHOT)
    parser.add_argument(
        "--input-file",
        type=Path,
        help="read an offline Markdown fixture instead of making the official HTTPS request",
    )
    parser.add_argument("--observed-at", type=int, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    try:
        if args.input_file is None:
            result = update_from_source(args.snapshot, observed_at=args.observed_at)
        else:
            result = update_from_markdown(
                args.snapshot, _read_fixture(args.input_file), observed_at=args.observed_at
            )
    except PricingUpdateError as exc:
        print(f"model price update failed: {exc}", file=sys.stderr)
        return 1
    print(json.dumps(result, separators=(",", ":"), allow_nan=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

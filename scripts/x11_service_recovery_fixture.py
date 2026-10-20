#!/usr/bin/env python3
"""Generate and validate the fixed Linux service recovery gate fixtures."""

from __future__ import annotations

import argparse
import datetime
import json
import sys
import time
from collections.abc import Sequence
from pathlib import Path

BASELINE_EVENTS = (
    ("gpt-5.6-sol", 1_000, 700, 100, 300),
    ("gpt-5.6-terra", 2_000, 1_400, 200, 600),
    ("gpt-5.6-luna", 3_000, 2_100, 300, 900),
)

APPEND_EVENTS = (
    ("gpt-5.6-sol", 4_000, 2_800, 400, 1_200),
    ("gpt-5.6-sol", 5_000, 3_500, 500, 1_500),
    ("gpt-5.6-terra", 6_000, 4_200, 600, 1_800),
    ("gpt-5.6-terra", 7_000, 4_900, 700, 2_100),
    ("gpt-5.6-luna", 8_000, 5_600, 800, 2_400),
    ("gpt-5.6-luna", 9_000, 6_300, 900, 2_700),
)

EXPECTED_MODELS = frozenset({"gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"})


def _utc_timestamp(seconds_ago: int = 0) -> str:
    return (
        datetime.datetime.fromtimestamp(time.time() - seconds_ago, datetime.timezone.utc)
        .isoformat()
        .replace("+00:00", "Z")
    )


def _write_events(session_path: Path, events: tuple[tuple, ...], *, append: bool) -> None:
    timestamp = _utc_timestamp(120 if not append else 0)
    mode = "a" if append else "w"
    with session_path.open(mode, encoding="utf-8") as stream:
        if not append:
            stream.write(
                json.dumps(
                    {
                        "type": "session_meta",
                        "payload": {
                            "id": "fixture-thread",
                            "session_id": "session-fixture-thread",
                        },
                    }
                )
                + "\n"
            )
        for model, total, input_tokens, cached, output in events:
            stream.write(
                json.dumps(
                    {
                        "type": "thread_context",
                        "model": model,
                        "timestamp": timestamp,
                    }
                )
                + "\n"
            )
            stream.write(
                json.dumps(
                    {
                        "type": "event_msg",
                        "timestamp": timestamp,
                        "payload": {
                            "type": "token_count",
                            "info": {
                                "total_token_usage": {
                                    "total_tokens": total,
                                    "input_tokens": input_tokens,
                                    "cached_input_tokens": cached,
                                    "output_tokens": output,
                                }
                            },
                        },
                    }
                )
                + "\n"
            )
        stream.write(
            json.dumps(
                {
                    "type": "thread_context",
                    "model": "gpt-5.6-sol",
                    "timestamp": timestamp,
                }
            )
            + "\n"
        )
        stream.write(json.dumps({"type": "task_started", "timestamp": timestamp}) + "\n")


def write_baseline(session_path: Path) -> None:
    _write_events(session_path, BASELINE_EVENTS, append=False)


def append_verified_usage(session_path: Path) -> None:
    _write_events(session_path, APPEND_EVENTS, append=True)


def readiness(operation: str, raw_document: str) -> bool:
    try:
        document = json.loads(raw_document)
    except json.JSONDecodeError:
        return False
    if not isinstance(document, dict):
        return False
    if operation == "ready":
        return (
            document.get("api_version") == "v4"
            and document.get("state") == "ready"
            and document.get("authenticated") is True
        )
    if operation == "models":
        models = document.get("models")
        model_names = [
            item.get("model")
            for item in models
            if isinstance(item, dict)
        ] if isinstance(models, list) else []
        return (
            document.get("api_version") == "v4"
            and document.get("state") == "ready"
            and document.get("authenticated") is True
            and isinstance(models, list)
            and len(models) == len(EXPECTED_MODELS)
            and len(model_names) == len(models)
            and all(isinstance(name, str) for name in model_names)
            and set(model_names) == EXPECTED_MODELS
        )
    raise ValueError(f"unsupported readiness operation: {operation}")


def _parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="operation", required=True)
    for operation in ("baseline", "append"):
        command = commands.add_parser(operation)
        command.add_argument("session_path", type=Path)
    commands.add_parser("ready")
    commands.add_parser("models")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = _parse_args(argv)
    if arguments.operation == "baseline":
        write_baseline(arguments.session_path)
        return 0
    if arguments.operation == "append":
        append_verified_usage(arguments.session_path)
        return 0
    return 0 if readiness(arguments.operation, sys.stdin.read()) else 1


if __name__ == "__main__":
    raise SystemExit(main())

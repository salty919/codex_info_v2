#!/usr/bin/env python3
"""Direct regression for the Linux service recovery gate's API and session fixtures."""

from __future__ import annotations

import json
import signal
import sys
import tempfile
from datetime import datetime, timedelta
from pathlib import Path

import x11_service_recovery_fixture as fixture

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "scripts" / "x11_service_recovery_visual_gate.sh"
FAKE_SERVER = ROOT / "scripts" / "fake_codex_app_server.py"
THREAD_ID = "fixture-thread"
SESSION_ID = "session-fixture-thread"
EXPECTED_EVENTS = [
    ("gpt-5.6-sol", 1_000, 700, 100, 300),
    ("gpt-5.6-terra", 2_000, 1_400, 200, 600),
    ("gpt-5.6-luna", 3_000, 2_100, 300, 900),
    ("gpt-5.6-sol", 4_000, 2_800, 400, 1_200),
    ("gpt-5.6-sol", 5_000, 3_500, 500, 1_500),
    ("gpt-5.6-terra", 6_000, 4_200, 600, 1_800),
    ("gpt-5.6-terra", 7_000, 4_900, 700, 2_100),
    ("gpt-5.6-luna", 8_000, 5_600, 800, 2_400),
    ("gpt-5.6-luna", 9_000, 6_300, 900, 2_700),
]
EXPECTED_MODELS = {"gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def record(failures: list[str], name: str, action) -> None:
    try:
        action()
    except AssertionError as error:
        failures.append(f"{name}: {error}")


def make_current(
    *,
    api_version: str = "v4",
    state: str = "ready",
    authenticated: bool = True,
    models: list[dict[str, str]] | None = None,
) -> bytes:
    body: dict[str, object] = {
        "api_version": api_version,
        "state": state,
        "observed_at": 1_792_478_500,
        "authenticated": authenticated,
        "plan_label": None,
        "quota": {
            "remaining_percent": 44.0,
            "reset_at": 1_792_785_000,
            "window_seconds": 604800,
            "monthly": False,
        },
        "models_status": "partial",
        "models": [] if models is None else models,
        "active_thread_count": 0,
        "open_session_thread_count": 0,
    }
    return json.dumps(body).encode("utf-8")


def read_session(path: Path) -> tuple[dict, list[tuple], list[dict]]:
    rows = [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    metadata_rows = [row for row in rows if row.get("type") == "session_meta"]
    require(len(metadata_rows) == 1, f"expected one session_meta row, got {len(metadata_rows)}")
    metadata = metadata_rows[0].get("payload", {})
    current_model = None
    events: list[tuple] = []
    for row in rows:
        if row.get("type") == "thread_context":
            current_model = row.get("model")
        if row.get("type") != "event_msg":
            continue
        usage = row.get("payload", {}).get("info", {}).get("total_token_usage")
        if isinstance(usage, dict):
            events.append(
                (
                    current_model,
                    usage.get("total_tokens"),
                    usage.get("input_tokens"),
                    usage.get("cached_input_tokens"),
                    usage.get("output_tokens"),
                )
            )
    return metadata, events, rows


def valid_utc_timestamp(value: object) -> bool:
    if not isinstance(value, str) or not value:
        return False
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError:
        return False
    return parsed.tzinfo is not None and parsed.utcoffset() == timedelta(0)


def fixture_timestamps_check(rows: list[dict]) -> None:
    segments: list[list[dict]] = []
    current: list[dict] = []
    for row in rows:
        if row.get("type") == "session_meta":
            continue
        current.append(row)
        if row.get("type") == "task_started":
            segments.append(current)
            current = []

    require(not current, "fixture records remain after the final task_started")
    require(len(segments) == 2, f"expected baseline and append segments, got {len(segments)}")
    expected_counts = ((4, 3, 1), (7, 6, 1))
    violations: list[str] = []
    for index, (segment, expected) in enumerate(zip(segments, expected_counts), 1):
        contexts = [row for row in segment if row.get("type") == "thread_context"]
        token_counts = [
            row
            for row in segment
            if row.get("type") == "event_msg"
            and row.get("payload", {}).get("type") == "token_count"
        ]
        lifecycle = [row for row in segment if row.get("type") == "task_started"]
        actual = (len(contexts), len(token_counts), len(lifecycle))
        if actual != expected:
            violations.append(
                f"segment {index} context/token_count/task_started count "
                f"expected {expected}, got {actual}"
            )

        records = contexts + token_counts + lifecycle
        timestamps = [row.get("timestamp") for row in records]
        invalid = sum(not valid_utc_timestamp(value) for value in timestamps)
        if invalid:
            violations.append(
                f"segment {index} has {invalid}/{len(records)} missing or invalid UTC timestamps"
            )
        elif len(set(timestamps)) != 1:
            violations.append(f"segment {index} records do not share the generator timestamp")

    require(not violations, "fixture timestamp contract failed: " + "; ".join(violations))


def timestamp_rejection_check() -> None:
    invalid_values = (
        None,
        "",
        "not-a-timestamp",
        "2026-10-20T12:00:00+09:00",
    )
    for value in invalid_values:
        require(
            not valid_utc_timestamp(value),
            f"missing, malformed, or non-UTC timestamp was accepted: {value!r}",
        )


def fixture_identity_check(source: str, metadata: dict) -> None:
    fake = FAKE_SERVER.read_text(encoding="utf-8")
    require(
        f'"CODEX_INFO_FAKE_THREAD_ID={THREAD_ID}"' in source,
        "gate thread identity differs from the independent fixture identity",
    )
    require(
        'thread_id = os.environ.get("CODEX_INFO_FAKE_THREAD_ID")' in fake
        and '"sessionId": f"session-{thread_id}"' in fake,
        "fake RPC no longer derives sessionId from its configured thread identity",
    )
    require(metadata.get("id") == THREAD_ID, f"session metadata id was {metadata.get('id')!r}")
    require(
        metadata.get("session_id") == SESSION_ID,
        f"session metadata session_id was {metadata.get('session_id')!r}",
    )


def fixture_event_check(events: list[tuple]) -> None:
    require(len(events) == 9, f"expected 9 token events, got {len(events)}")
    require(
        events == EXPECTED_EVENTS,
        f"fixture values/order differ from the fixed oracle:\nactual={events!r}",
    )
    violations = []
    for index, (_model, total, input_tokens, cached, output) in enumerate(events, 1):
        if total != input_tokens + output:
            violations.append(
                f"event {index}: total={total}, input={input_tokens}, output={output}"
            )
        if cached > input_tokens:
            violations.append(
                f"event {index}: cached input={cached} exceeds input={input_tokens}"
            )
    require(not violations, "invalid token accounting: " + "; ".join(violations))


def readiness_cases() -> list[tuple[str, str, bool, bytes]]:
    model_rows = [{"model": model} for model in sorted(EXPECTED_MODELS)]
    duplicate_rows = [
        {"model": "gpt-5.6-sol"},
        {"model": "gpt-5.6-sol"},
        {"model": "gpt-5.6-luna"},
    ]
    missing_rows = [{"model": "gpt-5.6-sol"}, {"model": "gpt-5.6-luna"}]
    return [
        ("ready-valid", "ready", True, make_current(models=[])),
        ("ready-error", "ready", False, make_current(state="error", models=[])),
        ("ready-unauthenticated", "ready", False, make_current(authenticated=False, models=[])),
        ("ready-malformed", "ready", False, b"{malformed"),
        ("ready-wrong-api", "ready", False, make_current(api_version="v3", models=[])),
        ("models-valid", "models", True, make_current(models=model_rows)),
        ("models-duplicate", "models", False, make_current(models=duplicate_rows)),
        ("models-missing", "models", False, make_current(models=missing_rows)),
        ("models-error", "models", False, make_current(state="error", models=model_rows)),
        (
            "models-unauthenticated",
            "models",
            False,
            make_current(authenticated=False, models=model_rows),
        ),
        ("models-malformed", "models", False, b"{malformed"),
        ("models-wrong-api", "models", False, make_current(api_version="v3", models=model_rows)),
    ]


def readiness_check(failures: list[str]) -> None:
    cases = readiness_cases()
    require(len(cases) == 12, f"expected 12 readiness cases, got {len(cases)}")
    for name, operation, expected, body in cases:
        actual = fixture.readiness(operation, body.decode("utf-8"))
        if actual != expected:
            failures.append(f"readiness {name}: expected {expected}, got {actual}")


def gate_helper_wiring_check(source: str) -> None:
    require(
        'fixture_helper="$root_dir/scripts/x11_service_recovery_fixture.py"' in source,
        "gate does not bind the fixed service recovery helper",
    )
    markers = (
        'python3 -B "$fixture_helper" baseline "$session_fixture"',
        'python3 -B "$fixture_helper" append "$session_fixture"',
        'python3 -B "$fixture_helper" ready',
        'python3 -B "$fixture_helper" models',
    )
    for marker in markers:
        require(source.count(marker) == 1, f"gate helper call count differs: {marker}")
    require(
        'http://127.0.0.1:$port/v4/current' in source,
        "gate no longer reads the v4 current endpoint",
    )
    require(
        """python3 - "$session_fixture" <<'PY'""" not in source
        and """python3 - "$details" <<'PY'""" not in source,
        "gate still contains inline fixture or readiness Python",
    )


def main() -> int:
    signal.alarm(55)
    failures: list[str] = []
    try:
        source = GATE.read_text(encoding="utf-8")
    except OSError as error:
        print(f"FAIL: read gate source: {error}", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix="x11-service-recovery-fixture-") as temporary:
        session_path = Path(temporary) / "session.jsonl"
        fixture.write_baseline(session_path)
        fixture.append_verified_usage(session_path)
        metadata, events, rows = read_session(session_path)

        record(
            failures,
            "session/RPC identity",
            lambda: fixture_identity_check(source, metadata),
        )
        record(failures, "token fixture values and accounting", lambda: fixture_event_check(events))
        record(failures, "fixture timestamps", lambda: fixture_timestamps_check(rows))
        record(failures, "timestamp rejection", timestamp_rejection_check)
        record(failures, "gate helper wiring", lambda: gate_helper_wiring_check(source))
        readiness_check(failures)

    if failures:
        for failure in failures:
            print(f"FAIL: {failure}", file=sys.stderr)
        print(f"FAIL: {len(failures)} independent contract check(s)", file=sys.stderr)
        return 1
    print("PASS: session identity, all token events, gate helper wiring, and readiness predicates")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

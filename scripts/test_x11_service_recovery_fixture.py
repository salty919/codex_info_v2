#!/usr/bin/env python3
"""Direct regression for the Linux service recovery gate's API and session fixtures."""

from __future__ import annotations

import json
import re
import shlex
import signal
import subprocess  # noqa: B404  # nosec B404 - fixed offline fixture processes only
import sys
import tempfile
import threading
import urllib.error
import urllib.request
from datetime import datetime, timedelta
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit

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
    except Exception as error:  # aggregate independent contract failures
        failures.append(f"{name}: {type(error).__name__}: {error}")


def read_source() -> str:
    return GATE.read_text(encoding="utf-8")


def extract_heredoc(source: str, marker: str, start: int = 0) -> str:
    marker_at = source.find(marker, start)
    require(marker_at >= 0, f"missing actual fixture generator marker: {marker!r}")
    body_start = marker_at + len(marker)
    body_end = source.find("\nPY\n", body_start)
    require(body_end >= 0, f"unterminated fixture generator: {marker!r}")
    return source[body_start:body_end]


def extract_function(source: str, name: str) -> str:
    match = re.search(
        rf"(?ms)^{re.escape(name)}\(\) \{{\n(.*?)^\}}",
        source,
    )
    require(match is not None, f"missing actual shell predicate {name}()")
    return match.group(0)


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


def execute_actual_fixture_generators(
    source: str, session_path: Path
) -> tuple[dict, list[tuple], list[dict]]:
    baseline_marker = "python3 - \"$session_fixture\" <<'PY'\n"
    baseline = extract_heredoc(source, baseline_marker)
    append_start = source.find("append_verified_usage() {")
    require(append_start >= 0, "missing append_verified_usage() fixture generator")
    append = extract_heredoc(source, baseline_marker, append_start)

    for label, code in (("baseline", baseline), ("append", append)):
        completed = subprocess.run(  # noqa: S603, B603  # nosec B603 - fixed offline fixture code only
            [sys.executable, "-", str(session_path)],
            input=code,
            text=True,
            capture_output=True,
            timeout=5,
            check=False,
        )
        require(
            completed.returncode == 0,
            f"actual {label} fixture generator exited {completed.returncode}: "
            f"{completed.stderr.strip()}",
        )

    rows = [
        json.loads(line)
        for line in session_path.read_text(encoding="utf-8").splitlines()
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
        usage = (
            row.get("payload", {})
            .get("info", {})
            .get("total_token_usage")
        )
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
            row for row in segment
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


def fixture_event_count_check(events: list[tuple]) -> None:
    require(len(events) == 9, f"expected 9 token events, got {len(events)}")


def fixture_values_check(events: list[tuple]) -> None:
    require(
        events == EXPECTED_EVENTS,
        f"fixture values/order differ from the fixed oracle:\nactual={events!r}",
    )


def fixture_accounting_check(events: list[tuple]) -> None:
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


def api_readiness_check(source: str, failures: list[str]) -> None:
    ready_function = extract_function(source, "service_ready")
    models_function = extract_function(source, "service_models_ready")

    model_rows = [{"model": model} for model in sorted(EXPECTED_MODELS)]
    duplicate_rows = [
        {"model": "gpt-5.6-sol"},
        {"model": "gpt-5.6-sol"},
        {"model": "gpt-5.6-luna"},
    ]
    missing_rows = [{"model": "gpt-5.6-sol"}, {"model": "gpt-5.6-luna"}]

    cases = [
        ("ready-valid", "service_ready", "accept", make_current(models=[])),
        ("ready-error", "service_ready", "reject",
         make_current(state="error", models=[])),
        ("ready-unauthenticated", "service_ready", "reject",
         make_current(authenticated=False, models=[])),
        ("ready-malformed", "service_ready", "reject", b"{malformed"),
        ("ready-wrong-api", "service_ready", "reject",
         make_current(api_version="v3", models=[])),
        ("models-valid", "service_models_ready", "accept",
         make_current(models=model_rows)),
        ("models-duplicate", "service_models_ready", "reject",
         make_current(models=duplicate_rows)),
        ("models-missing", "service_models_ready", "reject",
         make_current(models=missing_rows)),
        ("models-error", "service_models_ready", "reject",
         make_current(state="error", models=model_rows)),
        ("models-unauthenticated", "service_models_ready", "reject",
         make_current(authenticated=False, models=model_rows)),
        ("models-malformed", "service_models_ready", "reject", b"{malformed"),
        ("models-wrong-api", "service_models_ready", "reject",
         make_current(api_version="v3", models=model_rows)),
    ]
    responses: list[tuple[int, bytes]] = [(200, make_current(models=[]))]
    responses.extend((200, body) for _, _, _, body in cases)
    routes: list[str] = []
    route_lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def do_GET(self) -> None:  # BaseHTTPRequestHandler interface
            path = urlsplit(self.path).path
            with route_lock:
                routes.append(path)
                if path == "/v4/current":
                    status, body = responses.pop(0) if responses else (
                        500,
                        b'{"error":"unexpected extra request"}',
                    )
                elif path == "/v1/details":
                    status, body = 503, b'{"api_version":"v1","error":"history_unavailable"}'
                else:
                    status, body = 404, b'{"error":"not_found"}'
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, _format: str, *_args) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
    thread.start()
    try:
        port = server.server_address[1]
        base = f"http://127.0.0.1:{port}"

        def legacy_route_check() -> None:
            try:
                urllib.request.urlopen(f"{base}/v1/details", timeout=1)  # noqa: S310  # nosec B310 - controlled loopback fixture; no external URL input
            except urllib.error.HTTPError as error:
                require(error.code == 503, f"legacy details returned HTTP {error.code}")
                require(
                    b"history_unavailable" in error.read(),
                    "legacy details 503 did not identify history_unavailable",
                )
            else:
                raise AssertionError("legacy details unexpectedly returned success")

        def v4_route_check() -> None:
            with urllib.request.urlopen(f"{base}/v4/current", timeout=1) as response:  # noqa: S310  # nosec B310 - controlled loopback fixture; no external URL input
                document = json.loads(response.read())
            require(response.status == 200, f"v4 current returned HTTP {response.status}")
            require(document.get("api_version") == "v4", "v4 current fixture has the wrong api_version")
            require(document.get("state") == "ready", "v4 current fixture is not ready")
            require(document.get("authenticated") is True, "v4 current fixture is not authenticated")
            require(document.get("models_status") == "partial", "fixture lost partial model state")
            require(document.get("models") == [], "fixture invented model values before they exist")

        record(failures, "controlled legacy route", legacy_route_check)
        record(failures, "controlled v4 route", v4_route_check)

        function_code = "\n\n".join((ready_function, models_function))
        cases_code = "\n".join(
            f"report_case {shlex.quote(name)} {shlex.quote(function)}"
            for name, function, _, _ in cases
        )
        shell = (
            f"port={port}\n"
            + function_code
            + "\n\nreport_case() {\n"
            + "  local name=\"$1\" function_name=\"$2\" result\n"
            + "  if \"$function_name\"; then result=accept; else result=reject; fi\n"
            + "  printf '%s=%s\\n' \"$name\" \"$result\"\n"
            + "}\n"
            + cases_code
            + "\n"
        )
        completed = subprocess.run(  # noqa: S603, B603  # nosec B603 - fixed offline fixture code only
            ["/bin/bash", "--noprofile", "--norc", "-c", shell],
            text=True,
            capture_output=True,
            timeout=5,
            check=False,
        )
        if completed.returncode != 0:
            failures.append(
                "extracted readiness functions: "
                f"exit={completed.returncode}, stderr={completed.stderr.strip()}"
            )
        actual = {}
        for line in completed.stdout.splitlines():
            name, separator, result = line.partition("=")
            require(separator == "=", f"malformed readiness result line: {line!r}")
            actual[name] = result
        expected = {name: result for name, _, result, _ in cases}
        for name, expected_result in expected.items():
            if actual.get(name) != expected_result:
                failures.append(
                    f"readiness {name}: expected {expected_result}, got {actual.get(name)!r}"
                )

        expected_routes = ["/v1/details", "/v4/current"] + ["/v4/current"] * len(cases)
        if routes != expected_routes:
            failures.append(
                "gate readiness route sequence differs from v4 contract: "
                f"expected={expected_routes!r}, actual={routes!r}"
            )
        if responses:
            failures.append(f"{len(responses)} controlled v4 responses were not consumed")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=1)
        if thread.is_alive():
            failures.append("controlled loopback server did not stop within 1 second")

def main() -> int:
    signal.alarm(55)
    failures: list[str] = []
    source = ""
    try:
        source = read_source()
    except Exception as error:
        failures.append(f"read gate source: {type(error).__name__}: {error}")

    with tempfile.TemporaryDirectory(prefix="x11-service-recovery-fixture-") as temporary:
        session_path = Path(temporary) / "session.jsonl"
        metadata: dict = {}
        events: list[tuple] = []
        rows: list[dict] = []
        if source:
            try:
                metadata, events, rows = execute_actual_fixture_generators(source, session_path)
            except Exception as error:
                failures.append(
                    f"actual shell fixture generators: {type(error).__name__}: {error}"
                )

            record(
                failures,
                "session/RPC identity",
                lambda: fixture_identity_check(source, metadata),
            )
            record(failures, "token event count", lambda: fixture_event_count_check(events))
            record(failures, "token fixture values", lambda: fixture_values_check(events))
            record(failures, "token accounting invariants", lambda: fixture_accounting_check(events))
            record(
                failures,
                "fixture timestamps",
                lambda: fixture_timestamps_check(rows),
            )
            record(
                failures,
                "missing/malformed/non-UTC timestamp rejection",
                timestamp_rejection_check,
            )
            try:
                api_readiness_check(source, failures)
            except Exception as error:
                failures.append(
                    f"actual readiness functions and routes: {type(error).__name__}: {error}"
                )

    if failures:
        for failure in failures:
            print(f"FAIL: {failure}", file=sys.stderr)
        print(f"FAIL: {len(failures)} independent contract check(s)", file=sys.stderr)
        return 1
    print("PASS: session identity, all token events, route contract, and readiness predicates")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

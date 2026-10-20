#!/usr/bin/env python3
"""Check the real recorder E2E fixture against source-binding identity."""

from __future__ import annotations

import json
import os
import re
import sqlite3
import subprocess
import tempfile
import time
from datetime import datetime
from pathlib import Path

SCRIPT = Path(__file__).with_name("record_daemon_e2e.sh")
FAKE_APP_SERVER = Path(__file__).with_name("fake_codex_app_server.py")
FIXTURE_THREAD_ID = "record-daemon-e2e-fixture-thread"
FIXTURE_THREAD_TITLE = "record daemon baseline fixture"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise SystemExit(f"record-daemon-fixture: FAIL: {message}")


def extract_shell_function(source: str, name: str) -> str:
    marker = f"{name}() {{\n"
    start = source.find(marker)
    require(start >= 0, f"shell function {name} was not found")
    function_source = source[start:]
    end = re.search(r"(?m)^}\n\n[A-Za-z_][A-Za-z0-9_]*\(\) \{", function_source)
    require(end is not None, f"shell function {name} has no bounded closing delimiter")
    return function_source[: end.start() + 2]


def run_real_write_fixture(root: Path) -> tuple[Path, list[dict[str, object]], dict[str, str]]:
    source = SCRIPT.read_text(encoding="utf-8")
    match = re.search(r"(?ms)^write_fixture\(\) \{\n.*?^\}", source)
    require(match is not None, "write_fixture function was not found")

    case_home = root / "codex"
    sessions_root = case_home / "sessions"
    case_data = root / "data"
    wrapper = "\n".join(
        (
            "set -euo pipefail",
            "common_env=()",
            "session_file=''",
            match.group(0),
            "write_fixture",
            "printf '__fixture_path__\\t%s\\n' \"$session_file\"",
            'for fixture_env_entry in "${common_env[@]}"; do',
            "    printf '__fixture_env__\\t%s\\n' \"$fixture_env_entry\"",
            "done",
        )
    )
    environment = os.environ.copy()
    environment.update(
        {
            "sessions_root": str(sessions_root),
            "case_home": str(case_home),
            "case_data": str(case_data),
        }
    )
    completed = subprocess.run(
        ["/bin/bash", "--noprofile", "--norc", "-s"],
        input=wrapper,
        check=True,
        capture_output=True,
        text=True,
        env=environment,
        timeout=5,
    )
    output_lines = completed.stdout.splitlines()
    path_line = next(
        (line for line in output_lines if line.startswith("__fixture_path__\t")),
        None,
    )
    require(path_line is not None, "write_fixture did not report its Session path")
    session_path = Path(path_line.partition("\t")[2])
    env_values: dict[str, str] = {}
    for line in output_lines:
        if not line.startswith("__fixture_env__\t"):
            continue
        key, separator, value = line.partition("\t")[2].partition("=")
        require(bool(separator), "write_fixture emitted malformed fake-server environment")
        env_values[key] = value

    rows = [json.loads(line) for line in session_path.read_text(encoding="utf-8").splitlines()]
    return session_path, rows, env_values


def check_session_fixture(
    session_path: Path, rows: list[dict[str, object]], env_values: dict[str, str]
) -> tuple[str, int]:
    require(len(rows) == 3, "baseline must keep one context and one token-count event")
    metadata = rows[0]
    payload = metadata.get("payload")
    require(metadata.get("type") == "session_meta", "Session must begin with session_meta")
    require(isinstance(payload, dict), "session_meta payload must be an object")
    require(payload.get("id") == FIXTURE_THREAD_ID, "session_meta thread id is incorrect")
    session_id = payload.get("session_id")
    require(
        isinstance(session_id, str)
        and bool(session_id.strip())
        and not any(ord(character) < 32 for character in session_id),
        "session_meta session id must be a non-empty valid string",
    )

    context, token_event = rows[1:]
    context_payload = context.get("payload")
    require(context.get("type") == "event_msg", "baseline context event is missing")
    require(isinstance(context_payload, dict), "baseline context payload must be an object")
    require(context_payload.get("type") == "turn_context", "baseline turn context is incorrect")
    require(context_payload.get("model") == "gpt-5.6-luna", "baseline model changed")
    token_payload = token_event.get("payload")
    require(token_event.get("type") == "event_msg", "baseline token event is missing")
    require(isinstance(token_payload, dict), "baseline token payload must be an object")
    require(token_payload.get("type") == "token_count", "baseline token event is incorrect")
    info = token_payload.get("info")
    require(isinstance(info, dict), "baseline token info must be an object")
    total = info.get("total_token_usage")
    require(
        total
        == {
            "total_tokens": 120,
            "input_tokens": 100,
            "cached_input_tokens": 80,
            "output_tokens": 20,
        },
        "baseline token values changed",
    )

    timestamp = context.get("timestamp")
    require(isinstance(timestamp, str), "baseline timestamp is missing")
    require(token_event.get("timestamp") == timestamp, "baseline events must retain one timestamp")
    parsed = datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    baseline_seconds = int(parsed.timestamp())
    require(baseline_seconds % 60 == 0, "baseline must remain aligned to a UTC minute")
    require(120 <= int(time.time()) - baseline_seconds <= 300, "baseline timing changed")

    expected_env = {
        "CODEX_INFO_FAKE_THREAD_ID": FIXTURE_THREAD_ID,
        "CODEX_INFO_FAKE_THREAD_PATH": str(session_path),
        "CODEX_INFO_FAKE_THREAD_TITLE": FIXTURE_THREAD_TITLE,
    }
    for key, value in expected_env.items():
        require(env_values.get(key) == value, f"fake app-server {key} does not match Session")

    reset_value = env_values.get("CODEX_INFO_FAKE_RESET_AT")
    require(
        isinstance(reset_value, str) and reset_value.isdecimal(),
        "write_fixture must pin CODEX_INFO_FAKE_RESET_AT as an integer",
    )
    reset_at = int(reset_value)
    current_seconds = int(time.time())
    require(
        reset_at - 604800 <= baseline_seconds <= current_seconds < reset_at,
        "fixture baseline and current time must both be inside the pinned week period",
    )
    return session_id, reset_at


def check_fake_app_server(
    session_path: Path,
    env_values: dict[str, str],
    metadata_session_id: str,
    reset_at: int,
) -> None:
    environment = os.environ.copy()
    environment.update(env_values)
    requests = (
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        {"jsonrpc": "2.0", "id": 2, "method": "account/read", "params": {}},
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "thread/read",
            "params": {"threadId": FIXTURE_THREAD_ID},
        },
        {"jsonrpc": "2.0", "id": 4, "method": "account/rateLimits/read", "params": {}},
    )
    input_data = "".join(json.dumps(request) + "\n" for request in requests)
    fixture_now = int(time.time())
    fake_times = (fixture_now - 17, fixture_now + 29)
    require(fake_times[0] != fake_times[1], "fake server launch times must be distinct")
    for fake_now in fake_times:
        environment.update(
            {
                "RECORD_DAEMON_FAKE_TIME": str(fake_now),
                "RECORD_DAEMON_FAKE_APP_SERVER": str(FAKE_APP_SERVER),
            }
        )
        completed = subprocess.run(
            [
                "/usr/bin/python3",
                "-c",
                "import os, runpy, time; time.time = lambda: float(os.environ['RECORD_DAEMON_FAKE_TIME']); runpy.run_path(os.environ['RECORD_DAEMON_FAKE_APP_SERVER'], run_name='__main__')",
            ],
            input=input_data,
            check=True,
            capture_output=True,
            text=True,
            env=environment,
            timeout=5,
        )
        responses = [json.loads(line) for line in completed.stdout.splitlines()]
        require(len(responses) == 4, "fake app-server did not answer each source/quota request")
        require(responses[0].get("result") == {}, "fake initialize response changed")
        require(
            responses[1].get("result", {}).get("account", {}).get("type") == "chatgpt",
            "fake account/read response is not a ChatGPT account",
        )
        thread = responses[2].get("result", {}).get("thread")
        require(isinstance(thread, dict), "fake thread/read did not return the matching thread")
        require(thread.get("id") == FIXTURE_THREAD_ID, "fake thread/read id does not match Session")
        require(Path(str(thread.get("path"))).resolve() == session_path.resolve(), "fake thread/read path does not match Session")
        require(thread.get("name") == FIXTURE_THREAD_TITLE, "fake thread/read title is empty or mismatched")
        require(
            thread.get("sessionId") == metadata_session_id,
            "fake thread/read sessionId does not match session_meta session_id",
        )
        quota = responses[3].get("result", {}).get("rateLimits", {}).get("primary")
        require(isinstance(quota, dict), "fake account/rateLimits/read response is missing")
        require(quota.get("resetsAt") == reset_at, "fake reset changed between app-server launches")
        require(quota.get("windowDurationMins") == 10080, "fake quota week window changed")


def run_rest_assertion(
    source: str,
    function_name: str,
    body_path: Path,
    headers_path: Path,
    reset_at: int,
    expected_total: int,
) -> subprocess.CompletedProcess[str]:
    assertion = extract_shell_function(source, function_name)
    wrapper = "\n".join(
        (
            "set -euo pipefail",
            assertion,
            f'{function_name} "$RECORD_DAEMON_BODY_PATH" "$RECORD_DAEMON_HEADERS_PATH" '
            '"$RECORD_DAEMON_RESET_AT" "$RECORD_DAEMON_EXPECTED_TOTAL"',
        )
    )
    environment = os.environ.copy()
    environment.update(
        {
            "RECORD_DAEMON_BODY_PATH": str(body_path),
            "RECORD_DAEMON_HEADERS_PATH": str(headers_path),
            "RECORD_DAEMON_RESET_AT": str(reset_at),
            "RECORD_DAEMON_EXPECTED_TOTAL": str(expected_total),
        }
    )
    return subprocess.run(
        ["/bin/bash", "--noprofile", "--norc", "-s"],
        input=wrapper,
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
        env=environment,
    )


def write_headers(path: Path, pair_suffix: str = "a" * 64) -> None:
    path.write_text(
        f"HTTP/1.1 200 OK\r\nCodex-Info-Published-Pair: v1:{pair_suffix}\r\n\r\n",
        encoding="ascii",
    )


def current_body(reset_at: int, total: int) -> dict[str, object]:
    components = {
        120: (100, 80, 20),
        240: (200, 160, 40),
        300: (250, 200, 50),
    }[total]
    input_tokens, cached_tokens, output_tokens = components
    return {
        "api_version": "v4",
        "state": "ready",
        "observed_at": reset_at - 1800,
        "authenticated": True,
        "plan_label": None,
        "quota": {
            "remaining_percent": 44.0,
            "reset_at": reset_at,
            "window_seconds": 604800,
            "monthly": False,
        },
        "models": [
            {
                "model": "gpt-5.6-luna",
                "total_tokens": total,
                "input_tokens": input_tokens,
                "cached_input_tokens": cached_tokens,
                "cache_write_input_tokens": None,
                "output_tokens": output_tokens,
                "estimated_cost": None,
            }
        ],
        "active_thread_count": 0,
        "open_session_thread_count": 0,
        "models_status": "partial",
    }


def history_body(reset_at: int, total: int) -> dict[str, object]:
    components = {
        120: (100, 80, 20),
        240: (200, 160, 40),
        300: (250, 200, 50),
    }[total]
    input_tokens, cached_tokens, output_tokens = components
    return {
        "api_version": "v3",
        "history_samples": [
            {
                "timestamp": reset_at - 3600,
                "reset_at": reset_at,
                "remaining_percent": 44.0,
                "task_active_since_previous": None,
                "models": None,
                "models_complete": False,
                "model_source": "unavailable",
            },
            {
                "timestamp": reset_at - 1800,
                "reset_at": reset_at,
                "remaining_percent": None,
                "task_active_since_previous": None,
                "models": [
                    {
                        "model": "gpt-5.6-luna",
                        "total_tokens": total,
                        "input_tokens": input_tokens,
                        "cached_input_tokens": cached_tokens,
                        "output_tokens": output_tokens,
                    }
                ],
                "models_complete": False,
                "model_source": "confirmed",
            },
        ],
        "history_gaps": [],
        "next_cursor": None,
        "resume_cursor": "opaque-resume-cursor",
    }


def run_history_snapshot_assertion(
    source: str,
    database_path: Path,
    body_path: Path,
    reset_at: int,
    expected_total: int,
) -> subprocess.CompletedProcess[str]:
    read_projection_snapshot = extract_shell_function(source, "read_projection_snapshot")
    assert_history_matches_sqlite = extract_shell_function(source, "assert_history_matches_sqlite")
    wrapper = "\n".join(
        (
            "set -euo pipefail",
            'case_db="$RECORD_DAEMON_SNAPSHOT_DB"',
            "read_sql() {",
            '    [[ -f "$case_db" ]] || return 1',
            '    sqlite3 -batch -readonly -bail -cmd \'.timeout 2000\' "$case_db" "$1"',
            "}",
            'is_uint() { [[ "$1" =~ ^[0-9]+$ ]]; }',
            read_projection_snapshot,
            assert_history_matches_sqlite,
            'snapshot="$(read_projection_snapshot "$RECORD_DAEMON_RESET_AT" '
            '"$RECORD_DAEMON_HISTORY_PATH" "$RECORD_DAEMON_EXPECTED_TOTAL")"',
            'assert_history_matches_sqlite "$RECORD_DAEMON_HISTORY_PATH" "$snapshot" '
            '"$RECORD_DAEMON_RESET_AT"',
        )
    )
    environment = os.environ.copy()
    environment.update(
        {
            "RECORD_DAEMON_SNAPSHOT_DB": str(database_path),
            "RECORD_DAEMON_HISTORY_PATH": str(body_path),
            "RECORD_DAEMON_RESET_AT": str(reset_at),
            "RECORD_DAEMON_EXPECTED_TOTAL": str(expected_total),
        }
    )
    return subprocess.run(
        ["/bin/bash", "--noprofile", "--norc", "-s"],
        input=wrapper,
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
        env=environment,
    )


def check_history_snapshot_race(root: Path) -> None:
    source = SCRIPT.read_text(encoding="utf-8")
    database_path = root / "snapshot-race.db"
    history_path = root / "snapshot-race-history.json"
    reset_at = 10_000
    timestamp = 1_000
    later_timestamp = timestamp + 60
    with sqlite3.connect(database_path) as database:
        database.executescript(
            """
            CREATE TABLE usage_history(timestamp INTEGER, reset_at INTEGER, remaining_percent REAL);
            CREATE TABLE collection_generation(singleton INTEGER, reset_at INTEGER, window_seconds INTEGER);
            CREATE TABLE session_model_totals(
                model TEXT, total_tokens INTEGER, input_tokens INTEGER,
                cached_input_tokens INTEGER, output_tokens INTEGER, cache_write_input_tokens INTEGER
            );
            CREATE TABLE usage_model_history(
                timestamp INTEGER, reset_at INTEGER, model TEXT, total_tokens INTEGER,
                input_tokens INTEGER, cached_input_tokens INTEGER, output_tokens INTEGER,
                cache_write_input_tokens INTEGER
            );
            """
        )
        database.execute("INSERT INTO collection_generation VALUES (1, ?, 604800)", (reset_at,))
        database.execute("INSERT INTO usage_history VALUES (?, ?, 44.0)", (timestamp, reset_at))
        database.execute(
            "INSERT INTO session_model_totals VALUES ('gpt-5.6-luna', 120, 100, 80, 20, NULL)"
        )
        database.executemany(
            "INSERT INTO usage_model_history VALUES (?, ?, 'gpt-5.6-luna', 120, 100, 80, 20, NULL)",
            ((timestamp, reset_at), (later_timestamp, reset_at)),
        )
    history_path.write_text(
        json.dumps(
            {
                "api_version": "v3",
                "history_samples": [
                    {
                        "timestamp": timestamp,
                        "reset_at": reset_at,
                        "remaining_percent": 44.0,
                        "models": [
                            {
                                "model": "gpt-5.6-luna",
                                "total_tokens": 120,
                                "input_tokens": 100,
                                "cached_input_tokens": 80,
                                "output_tokens": 20,
                            }
                        ],
                        "models_complete": False,
                        "model_source": "confirmed",
                    }
                ],
                "history_gaps": [],
                "next_cursor": None,
                "resume_cursor": "opaque-resume-cursor",
            }
        ),
        encoding="utf-8",
    )
    result = run_history_snapshot_assertion(source, database_path, history_path, reset_at, 120)
    require(
        result.returncode == 0,
        "an accepted history observation at T was mismatched against a later DB row at T+60: "
        + result.stderr.strip(),
    )


def check_rest_current_history_assertions(root: Path, reset_at: int) -> None:
    source = SCRIPT.read_text(encoding="utf-8")
    current_headers = root / "current.headers"
    history_headers = root / "history.headers"
    write_headers(current_headers)
    write_headers(history_headers, "b" * 64)

    for total in (120, 240):
        current_path = root / f"current-{total}.json"
        current_path.write_text(json.dumps(current_body(reset_at, total)), encoding="utf-8")
        current_result = run_rest_assertion(
            source, "assert_current_file", current_path, current_headers, reset_at, total
        )
        require(
            current_result.returncode == 0,
            f"v4 current rejected the exact {total}-token observation: "
            + current_result.stderr.strip(),
        )

        history_path = root / f"history-{total}.json"
        history_path.write_text(json.dumps(history_body(reset_at, total)), encoding="utf-8")
        history_result = run_rest_assertion(
            source, "assert_history_file", history_path, history_headers, reset_at, total
        )
        require(
            history_result.returncode == 0,
            f"v3 history rejected the exact {total}-token observation with a nullable gap: "
            + history_result.stderr.strip(),
        )

    pending_current = root / "current-pending.json"
    pending_document = current_body(reset_at, 120)
    pending_document["models"] = []
    pending_current.write_text(json.dumps(pending_document), encoding="utf-8")
    result = run_rest_assertion(
        source, "assert_current_file", pending_current, current_headers, reset_at, 120
    )
    require(result.returncode == 75, "an unpublished v4 model projection was not kept pending")

    error_current = root / "current-error.json"
    error_current.write_text(json.dumps({"api_version": "v1", "error": "details_unavailable"}))
    result = run_rest_assertion(
        source, "assert_current_file", error_current, current_headers, reset_at, 120
    )
    require(result.returncode == 1, "a legacy/error body was accepted as v4 current")

    wrong_reset_current = root / "current-wrong-reset.json"
    wrong_reset_current.write_text(
        json.dumps(current_body(reset_at + 60, 120)), encoding="utf-8"
    )
    result = run_rest_assertion(
        source, "assert_current_file", wrong_reset_current, current_headers, reset_at, 120
    )
    require(result.returncode == 1, "v4 current accepted a different reset boundary")

    overshoot_current = root / "current-overshoot.json"
    overshoot_current.write_text(json.dumps(current_body(reset_at, 300)), encoding="utf-8")
    result = run_rest_assertion(
        source, "assert_current_file", overshoot_current, current_headers, reset_at, 240
    )
    require(result.returncode == 1, "v4 current accepted model usage beyond the owned append")

    empty_history = root / "history-pending.json"
    empty_page = history_body(reset_at, 120)
    empty_page["history_samples"] = []
    empty_page["resume_cursor"] = None
    empty_history.write_text(json.dumps(empty_page), encoding="utf-8")
    result = run_rest_assertion(
        source, "assert_history_file", empty_history, history_headers, reset_at, 120
    )
    require(result.returncode == 75, "an unpublished v3 history projection was not kept pending")

    error_history = root / "history-error.json"
    error_history.write_text(json.dumps({"api_version": "v3", "error": "history_unavailable"}))
    result = run_rest_assertion(
        source, "assert_history_file", error_history, history_headers, reset_at, 120
    )
    require(result.returncode == 1, "a history error body was accepted as a v3 page")

    wrong_reset_history = root / "history-wrong-reset.json"
    wrong_page = history_body(reset_at + 60, 120)
    wrong_reset_history.write_text(json.dumps(wrong_page), encoding="utf-8")
    result = run_rest_assertion(
        source, "assert_history_file", wrong_reset_history, history_headers, reset_at, 120
    )
    require(result.returncode == 1, "v3 history accepted a different reset boundary")

    overshoot_history = root / "history-overshoot.json"
    overshoot_history.write_text(json.dumps(history_body(reset_at, 300)), encoding="utf-8")
    result = run_rest_assertion(
        source, "assert_history_file", overshoot_history, history_headers, reset_at, 240
    )
    require(result.returncode == 1, "v3 history accepted model usage beyond the owned append")
    check_history_snapshot_race(root)


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="record-daemon-fixture-") as temporary_root:
        session_path, rows, env_values = run_real_write_fixture(Path(temporary_root))
        metadata_session_id, reset_at = check_session_fixture(session_path, rows, env_values)
        check_fake_app_server(session_path, env_values, metadata_session_id, reset_at)
        check_rest_current_history_assertions(Path(temporary_root), reset_at)
    print("record-daemon-fixture: PASS session binding, fixed reset, and REST current/history contracts")


if __name__ == "__main__":
    main()

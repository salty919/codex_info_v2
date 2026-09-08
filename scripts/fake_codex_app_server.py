#!/usr/bin/env python3
"""Bounded Codex app-server fixture for isolated native acceptance tests."""

import json
import os
import sys
import time


reset_at = int(os.environ.get("CODEX_INFO_FAKE_RESET_AT", int(time.time()) + 604800 - 3600))
failure_file = os.environ.get("CODEX_INFO_FAKE_FAILURE_FILE")
thread_id = os.environ.get("CODEX_INFO_FAKE_THREAD_ID")
thread_path = os.environ.get("CODEX_INFO_FAKE_THREAD_PATH")
thread_title = os.environ.get("CODEX_INFO_FAKE_THREAD_TITLE")
thread_item = None
if thread_id and thread_path and thread_title:
    now = int(time.time())
    thread_item = {
        "cliVersion": "0.147.0",
        "createdAt": now - 120,
        "cwd": os.path.dirname(thread_path),
        "ephemeral": False,
        "id": thread_id,
        "modelProvider": "openai",
        "preview": thread_title,
        "sessionId": f"session-{thread_id}",
        "source": "cli",
        "status": {"type": "idle"},
        "turns": [],
        "updatedAt": now,
        "name": thread_title,
        "path": thread_path,
    }
account = {
    "requiresOpenaiAuth": False,
    "account": {
        "type": "chatgpt",
        "email": "fixture@example.com",
        "planType": "pro",
    },
}
quota = {
    "rateLimits": {
        "primary": {
            "usedPercent": 56,
            "resetsAt": reset_at,
            "windowDurationMins": 10080,
        }
    }
}

for line in sys.stdin:
    try:
        request = json.loads(line)
    except json.JSONDecodeError:
        continue
    request_id = request.get("id")
    if not isinstance(request_id, int):
        continue
    method = request.get("method")
    if method == "initialize":
        result = {}
    elif method == "account/read":
        result = account
    elif method == "account/rateLimits/read" and failure_file and os.path.exists(failure_file):
        print(
            json.dumps({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32000, "message": "injected bounded fixture failure"},
            }),
            flush=True,
        )
        continue
    elif method == "account/rateLimits/read":
        result = quota
    elif method == "thread/list":
        result = {"data": [thread_item] if thread_item is not None else []}
    elif (
        method == "thread/read"
        and thread_item is not None
        and request.get("params", {}).get("threadId") == thread_id
    ):
        result = {"thread": thread_item}
    else:
        result = {}
    print(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}),
        flush=True,
    )

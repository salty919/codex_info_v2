#!/usr/bin/env python3
"""Bounded Codex app-server fixture for isolated native acceptance tests."""

import json
import os
import sys
import time


reset_at = int(os.environ.get("CODEX_INFO_FAKE_RESET_AT", int(time.time()) + 604800 - 3600))
failure_file = os.environ.get("CODEX_INFO_FAKE_FAILURE_FILE")
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
        result = {"data": []}
    else:
        result = {}
    print(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}),
        flush=True,
    )

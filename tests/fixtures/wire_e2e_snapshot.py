#!/usr/bin/env python3
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
"""Resolve qualified UI rows for E2E oracles; never emulate a retired schema."""
import argparse
import json
import pathlib
import subprocess


def receive(args, target, conditions=()):
    command = [str(args.probe), "--port", str(args.port), "--cache", str(args.cache),
               "--target", target, "--wait-ms", str(args.wait_ms), *conditions]
    result = subprocess.run(command, capture_output=True, text=True, timeout=args.wait_ms / 1000 + 5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "qualified UI receiver failed")
    document = json.loads(result.stdout)
    if set(document) != {"views"} or not isinstance(document["views"], list):
        raise ValueError("unknown qualified receiver envelope")
    return document


def one_row(document, kind):
    rows = [(view, row) for view in document["views"] for row in view["rows"] if row["key"][0] == kind]
    if len(rows) != 1:
        raise ValueError(f"expected one {kind} row")
    return rows[0]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", required=True, type=pathlib.Path)
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--cache", required=True, type=pathlib.Path)
    parser.add_argument("--resource", required=True, choices=("current", "runtime", "periods", "history"))
    parser.add_argument("--output", required=True, type=pathlib.Path)
    parser.add_argument("--reset", type=int)
    parser.add_argument("--period")
    parser.add_argument("--min-total", type=int)
    parser.add_argument("--sample-source")
    parser.add_argument("--wait-ms", type=int, default=10000)
    args = parser.parse_args()
    if not args.probe.is_absolute() or not args.cache.is_absolute() or not args.output.is_absolute():
        raise ValueError("explicit absolute E2E paths required")
    if not 1 <= args.port <= 65535 or not 1 <= args.wait_ms <= 30000:
        raise ValueError("invalid finite operation boundary")

    if args.resource == "runtime":
        document = receive(args, "/state?parts=runtime")
        view, row = one_row(document, "runtime")
        value = row["value"]
    else:
        conditions = ["--ready", "true"]
        if args.resource == "current" and args.min_total is not None:
            conditions += ["--min-model-total", str(args.min_total)]
        current = receive(args, "/state?parts=main", conditions)
        main_view, main_row = one_row(current, "main")
        if args.resource == "current":
            document, view, value = current, main_view, main_row["value"]
        else:
            account = main_row["value"]["selection"]["account_id"]
            if not isinstance(account, str) or not account:
                raise ValueError("confirmed account selection missing")
            if args.resource == "periods":
                if args.reset is None:
                    raise ValueError("exact requested display period required")
                window = main_row["value"]["quota"]["window_seconds"]
                target = f"/history/periods?account={account}&purpose=window&from={max(1, args.reset-window)}&until={args.reset+1}"
                document = receive(args, target)
            else:
                if not args.period:
                    raise ValueError("server-issued or fixture-declared period selector required")
                target = f"/history?account={account}&period={args.period}"
                conditions = ["--samples", "true"]
                if args.min_total is not None:
                    conditions += ["--sample-total", str(args.min_total)]
                if args.sample_source:
                    conditions += ["--sample-source", args.sample_source]
                document = receive(args, target, conditions)
            if len(document["views"]) != 1:
                raise ValueError("resource view is not complete")
            view = document["views"][0]
            if view["view_key"][1]["account"] != account:
                raise ValueError("cross-account resource view")
            if args.resource == "periods":
                value = {"periods": []}
                for row in view["rows"]:
                    if row["key"][0] != "period" or row["value"]["id"] != row["key"][1]:
                        raise ValueError("period owner/key mismatch")
                    value["periods"].append(row["value"])
            else:
                value = {"period_id": args.period, "samples": [], "gaps": [], "local_usage": []}
                for row in view["rows"]:
                    key, item = row["key"], dict(row["value"])
                    if key[1] != args.period:
                        raise ValueError("cross-period row")
                    if key[0] == "sample":
                        item.update(reset_at=key[2], timestamp=key[3])
                        value["samples"].append(item)
                    elif key[0] == "gap":
                        item["gap_id"] = key[2]
                        value["gaps"].append(item)
                    elif key[0] == "local_usage":
                        value["local_usage"].append(item)
                    else:
                        raise ValueError("unexpected history row")
                value["samples"].sort(key=lambda item: (item["reset_at"], item["timestamp"]))
    if not isinstance(view["image"], str) or not view["image"]:
        raise ValueError("qualified resource image missing")
    metadata = {"view_key": view["view_key"], "image": view["image"], "complete": True}
    args.output.with_suffix(".snapshot.json").write_text(json.dumps(document), encoding="utf-8")
    args.output.with_suffix(".meta.json").write_text(json.dumps(metadata), encoding="utf-8")
    args.output.with_suffix(".json").write_text(json.dumps(value), encoding="utf-8")
    print(json.dumps(value))


if __name__ == "__main__":
    main()

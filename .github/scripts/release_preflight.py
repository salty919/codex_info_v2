#!/usr/bin/env python3
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
"""Plan/reconstruct a release-version tree without changing a branch ref.

This file and product_version.py must come from the caller's trusted workflow
SHA. The PR checkout is data until its reconstructed tree matches the plan.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
from product_version import VersionPaths, bump_versions, check_versions, next_version

VERSION_FILES = ("Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props")
SHA = re.compile(r"^[0-9a-f]{40}$")
FIELDS = {"schema", "source_sha", "source_tree", "main_base_sha", "workflow_sha",
          "base_version", "expected_version", "expected_tree", "main_included"}


def git(*args: str, data: bytes | None = None, env=None) -> bytes:
    return subprocess.check_output(["git", *args], input=data, env=env)


def oid(value: str) -> str:
    if not isinstance(value, str) or SHA.fullmatch(value) is None:
        raise ValueError("snapshot object ID is malformed")
    return value


def paths(root: Path) -> VersionPaths:
    return VersionPaths(*(root / name for name in VERSION_FILES))


def extract(revision: str, root: Path) -> None:
    for name in VERSION_FILES:
        # Never follow a symlink or executable supplied by the PR.
        entry = git("ls-tree", revision, "--", name).decode().split()
        if len(entry) != 4 or entry[0] != "100644" or entry[1] != "blob":
            raise ValueError(f"version input is not a regular file: {name}")
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(git("show", f"{revision}:{name}"))


def prepared_files(source: str, base_version: str, expected_version: str, root: Path) -> None:
    extract(source, root)
    version = check_versions(paths(root))
    if version == expected_version:
        return
    if version != base_version or expected_version != next_version(base_version):
        raise ValueError("source version is neither the pinned main version nor its next patch")
    bump_versions(paths(root), expected=base_version)


def version_tree(source: str, root: Path, temporary: Path) -> str:
    index = temporary / "planned.index"
    environment = {**os.environ, "GIT_INDEX_FILE": str(index)}
    git("read-tree", f"{source}^{{tree}}", env=environment)
    for name in VERSION_FILES:
        blob = git("hash-object", "-w", "--stdin", data=(root / name).read_bytes()).decode().strip()
        git("update-index", "--add", "--cacheinfo", f"100644,{blob},{name}", env=environment)
    return git("write-tree", env=environment).decode().strip()


def plan(source: str, main: str, workflow: str) -> dict:
    oid(source); oid(main); oid(workflow)
    with tempfile.TemporaryDirectory(prefix="codex-release-plan-") as tmp:
        temporary = Path(tmp)
        extract(main, temporary / "main")
        base_version = check_versions(paths(temporary / "main"))
        expected_version = next_version(base_version)
        prepared_files(source, base_version, expected_version, temporary / "next")
        tree = version_tree(source, temporary / "next", temporary)
    included = subprocess.run(["git", "merge-base", "--is-ancestor", main, source],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if included.returncode not in (0, 1):
        raise ValueError("could not establish the pinned main ancestry")
    return {"schema": 1, "source_sha": source,
            "source_tree": git("rev-parse", f"{source}^{{tree}}").decode().strip(),
            "main_base_sha": main, "workflow_sha": workflow,
            "base_version": base_version, "expected_version": expected_version,
            "expected_tree": tree, "main_included": included.returncode == 0}


def apply(snapshot: dict, source: str, workflow: str) -> None:
    if not isinstance(snapshot, dict) or set(snapshot) != FIELDS or type(snapshot["schema"]) is not int or snapshot["schema"] != 1:
        raise ValueError("planned snapshot schema differs")
    for key in ("source_sha", "source_tree", "main_base_sha", "workflow_sha", "expected_tree"):
        oid(snapshot[key])
    if snapshot["source_sha"] != oid(source) or snapshot["workflow_sha"] != oid(workflow):
        raise ValueError("planned source/workflow identity differs from the caller")
    if type(snapshot["main_included"]) is not bool:
        raise ValueError("planned ancestry is malformed")
    if snapshot["expected_version"] != next_version(snapshot["base_version"]):
        raise ValueError("planned version differs from the pinned next patch")
    if git("rev-parse", "HEAD").decode().strip() != source:
        raise ValueError("checkout is not the exact source commit")
    if git("rev-parse", "HEAD^{tree}").decode().strip() != snapshot["source_tree"]:
        raise ValueError("checkout tree differs from the planned source")
    git("diff", "--exit-code", "HEAD", "--")
    # Validate every output before touching the product checkout.
    with tempfile.TemporaryDirectory(prefix="codex-release-apply-") as tmp:
        temporary = Path(tmp)
        prepared_files(source, snapshot["base_version"], snapshot["expected_version"], temporary / "next")
        if version_tree(source, temporary / "next", temporary) != snapshot["expected_tree"]:
            raise ValueError("reconstructed release tree differs from the trusted plan")
        for name in VERSION_FILES:
            target = Path(name)
            if target.is_symlink() or not target.is_file():
                raise ValueError(f"checkout version input is not a regular file: {name}")
        for name in VERSION_FILES:
            Path(name).write_bytes((temporary / "next" / name).read_bytes())
    changed = set(git("diff", "--name-only", "HEAD", "--").decode().splitlines())
    if not changed.issubset(VERSION_FILES):
        raise ValueError("reconstruction changed a non-version tracked input")
    print("planned-release-tree: verified", snapshot["expected_tree"], snapshot["expected_version"])
    if not snapshot["main_included"]:
        print("planned-release-tree: main is not an ancestor; promotion requires fresh checks")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("plan", "apply"))
    parser.add_argument("--source", required=True)
    parser.add_argument("--workflow", required=True)
    parser.add_argument("--main")
    parser.add_argument("--plan")
    args = parser.parse_args()
    try:
        if args.mode == "plan":
            print(json.dumps(plan(args.source, args.main, args.workflow), separators=(",", ":")))
        else:
            apply(json.loads(args.plan), args.source, args.workflow)
    except (ValueError, RuntimeError, subprocess.CalledProcessError, TypeError) as error:
        print(f"planned-release-tree: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

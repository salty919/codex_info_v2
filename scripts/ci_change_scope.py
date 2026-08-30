#!/usr/bin/env python3
"""Classify one complete Git diff into the quality owners that must run."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import sys
from typing import Sequence


OWNER_ORDER = ("DOCS", "GOVERNANCE", "LINUX_BACKEND", "LINUX_UI", "WINDOWS")
PRODUCT_OWNERS = frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"})
GIT_STATUSES = frozenset({"A", "C", "D", "M", "R", "T"})

DOC_EXACT = frozenset({"README.md", "README.en.md", "DESIGN.md", "SECURITY.md"})
WINDOWS_SCRIPT_EXACT = frozenset(
    {
        "scripts/capture_windows_window.ps1",
        "scripts/windows_window_move_message_smoke.ps1",
        "scripts/windows_window_move_smoke.ps1",
    }
)
LINUX_BACKEND_EXACT = frozenset(
    {
        "run.sh",
        "scripts/cli_contract_e2e.sh",
        "scripts/data_protection_gate.sh",
        "scripts/db_protection_e2e.sh",
        "scripts/install_systemd_recorder.sh",
        "scripts/record_daemon_e2e.sh",
    }
)
LINUX_UI_EXACT = frozenset(
    {"scripts/x11_graph_visual_gate.sh", "scripts/x11_startup_visual_gate.sh"}
)
LINUX_SHARED_EXACT = frozenset(
    {"Cargo.toml", "Cargo.lock", "build.rs", "deny.toml", "src/main.rs"}
)
LEGAL_SHARED_EXACT = frozenset(
    {"COPYRIGHT", "LICENSE", "LICENSE.ja.md", "THIRD_PARTY_NOTICES.md"}
)


class ScopeError(ValueError):
    """The diff cannot be mapped completely to the finite owner set."""


@dataclass(frozen=True)
class Selection:
    owners: tuple[str, ...]
    codeql_languages: tuple[str, ...]

    @property
    def binary_impact(self) -> bool:
        return bool(PRODUCT_OWNERS.intersection(self.owners))

    def as_json(self) -> str:
        return json.dumps(
            {
                "binary_impact": self.binary_impact,
                "owners": list(self.owners),
                "codeql_languages": list(self.codeql_languages),
            },
            separators=(",", ":"),
            sort_keys=True,
        )


def _path(value: str) -> str:
    if not value or "\x00" in value or value.startswith("/"):
        raise ScopeError("changed file path is malformed")
    if any(part in {"", ".", ".."} for part in value.split("/")):
        raise ScopeError("changed file path is not normalized")
    return value


def owners_for_path(path: str) -> frozenset[str]:
    """Return the owner set for one repository path, or reject an unknown area."""
    path = _path(path)
    if path in DOC_EXACT or path.startswith(("docs/", "wiki/")):
        return frozenset({"DOCS"})
    if path.startswith((".github/", ".vscode/", ".codex-tasks/")) or path in {
        ".gitignore",
        "AGENTS.md",
    }:
        return frozenset({"GOVERNANCE"})
    if path in WINDOWS_SCRIPT_EXACT or path.startswith("windows-client/"):
        return frozenset({"WINDOWS"})
    if path in LINUX_BACKEND_EXACT or path.startswith(("tests/", "packaging/")):
        return frozenset({"LINUX_BACKEND"})
    if path.startswith("src/") and path != "src/main.rs":
        return frozenset({"LINUX_BACKEND"})
    if path in LINUX_UI_EXACT or path.startswith(("ui/", "assets/")):
        return frozenset({"LINUX_UI"})
    if path in LINUX_SHARED_EXACT or path.startswith(".cargo/"):
        return frozenset({"LINUX_BACKEND", "LINUX_UI"})
    if path.startswith("protocol/"):
        return frozenset({"LINUX_BACKEND", "WINDOWS"})
    if path in LEGAL_SHARED_EXACT or path.startswith("LICENSES/"):
        return frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"})
    if path.startswith("scripts/"):
        return frozenset({"GOVERNANCE"})
    raise ScopeError(f"changed path has no CI owner: {path}")


def selection_for_paths(paths: Sequence[str]) -> Selection:
    owners: set[str] = set()
    for path in paths:
        owners.update(owners_for_path(path))
    if not owners:
        raise ScopeError("pull request contains no changed paths")

    languages: set[str] = set()
    if "GOVERNANCE" in owners:
        languages.update(("actions", "python"))
    if "WINDOWS" in owners:
        languages.add("csharp")
    if {"LINUX_BACKEND", "LINUX_UI"}.intersection(owners):
        languages.add("rust")
    return Selection(
        owners=tuple(owner for owner in OWNER_ORDER if owner in owners),
        codeql_languages=tuple(
            language
            for language in ("actions", "csharp", "python", "rust")
            if language in languages
        ),
    )


def paths_from_name_status(raw: bytes) -> tuple[str, ...]:
    """Parse `git diff --name-status -z`, retaining both ends of renames/copies."""
    if not raw or not raw.endswith(b"\0"):
        raise ScopeError("git name-status diff is empty or truncated")
    try:
        fields = [field.decode("utf-8") for field in raw[:-1].split(b"\0")]
    except UnicodeDecodeError as exc:
        raise ScopeError("git name-status diff is not UTF-8") from exc

    paths: list[str] = []
    index = 0
    while index < len(fields):
        status = fields[index]
        index += 1
        kind = status[:1]
        if kind not in GIT_STATUSES:
            raise ScopeError(f"unsupported git diff status: {status!r}")
        path_count = 2 if kind in {"C", "R"} else 1
        if index + path_count > len(fields):
            raise ScopeError("git name-status record is truncated")
        paths.extend(_path(value) for value in fields[index : index + path_count])
        index += path_count
    return tuple(paths)


def selection_from_name_status(raw: bytes) -> Selection:
    return selection_for_paths(paths_from_name_status(raw))


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name-status", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        result = selection_from_name_status(args.name_status.read_bytes())
    except (OSError, ScopeError) as exc:
        print(f"ci-change-scope: FAIL {exc}", file=sys.stderr)
        return 1
    print(result.as_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

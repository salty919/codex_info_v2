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
GIT_STATUSES = frozenset({"A", "C", "D", "M", "R", "T"})

DOC_EXACT = frozenset(
    {"README.md", "README.en.md", "DESIGN.md", "SECURITY.md"}
)
LINUX_PRODUCT_SCRIPT_PREFIXES = ("build_linux_", "install_systemd_", "linux_")
LINUX_TEST_SCRIPT_PREFIXES = (
    "cli_",
    "data_",
    "db_",
    "fake_codex_",
    "record_daemon_",
    "regression_guard",
    "test_linux_",
    "test_run_",
)
LINUX_SHARED_EXACT = frozenset(
    {"Cargo.toml", "Cargo.lock", "build.rs", "src/main.rs"}
)
WINDOWS_PRODUCT_ROOT_EXACT = frozenset(
    {
        "windows-client/CodexInfo.WindowsClient.sln",
        "windows-client/Directory.Build.props",
        "windows-client/THIRD_PARTY_NOTICES.md",
    }
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
    powershell_paths: tuple[str, ...]
    binary_impact: bool
    distribution_required: bool

    def as_json(self) -> str:
        return json.dumps(
            {
                "binary_impact": self.binary_impact,
                "distribution_required": self.distribution_required,
                "owners": list(self.owners),
                "codeql_languages": list(self.codeql_languages),
                "powershell_paths": list(self.powershell_paths),
            },
            separators=(",", ":"),
            sort_keys=True,
        )


@dataclass(frozen=True)
class PathSelection:
    """Quality, publication, and CodeQL effects for one changed path."""

    owners: frozenset[str]
    binary_impact: bool
    codeql_languages: frozenset[str] = frozenset()


def _path(value: str) -> str:
    if (
        not value
        or any(character in value for character in "\x00\r\n")
        or value.startswith("/")
    ):
        raise ScopeError("changed file path is malformed")
    if any(part in {"", ".", ".."} for part in value.split("/")):
        raise ScopeError("changed file path is not normalized")
    return value


def _selection_for_path(path: str) -> PathSelection:
    path = _path(path)
    if path in DOC_EXACT or path.startswith(("docs/", "wiki/")):
        return PathSelection(frozenset({"DOCS"}), False)
    if path.startswith((".github/", ".vscode/", ".codex-tasks/")) or path in {
        ".gitignore",
        "AGENTS.md",
        "deny.toml",
    }:
        languages = (
            frozenset({"actions"})
            if path.startswith(".github/workflows/")
            else frozenset()
        )
        return PathSelection(frozenset({"GOVERNANCE"}), False, languages)
    if path.startswith("scripts/"):
        name = path.rsplit("/", 1)[-1]
        if path.endswith(".ps1") or name.startswith("windows_"):
            return PathSelection(frozenset({"WINDOWS"}), False)
        if name.startswith("x11_"):
            return PathSelection(frozenset({"LINUX_UI"}), False)
        if name.startswith(LINUX_PRODUCT_SCRIPT_PREFIXES):
            return PathSelection(frozenset({"LINUX_BACKEND"}), True)
        if name.startswith(LINUX_TEST_SCRIPT_PREFIXES):
            return PathSelection(frozenset({"LINUX_BACKEND"}), False)
        languages = (
            frozenset({"python"})
            if path.endswith(".py") and not name.startswith("test_")
            else frozenset()
        )
        return PathSelection(frozenset({"GOVERNANCE"}), False, languages)
    if path == "run.sh" or path.startswith("packaging/"):
        return PathSelection(frozenset({"LINUX_BACKEND"}), True)
    if path.startswith("tests/fixtures/graph_"):
        return PathSelection(
            frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"}), False
        )
    if path.startswith("tests/"):
        return PathSelection(frozenset({"LINUX_BACKEND"}), False)
    if path.startswith("src/") and path != "src/main.rs":
        languages = frozenset({"rust"}) if path.endswith(".rs") else frozenset()
        return PathSelection(frozenset({"LINUX_BACKEND"}), True, languages)
    if path.startswith(("ui/", "assets/")):
        return PathSelection(frozenset({"LINUX_UI"}), True)
    if path in LINUX_SHARED_EXACT or path.startswith(".cargo/"):
        languages = (
            frozenset({"rust"})
            if path in {"build.rs", "src/main.rs"}
            else frozenset()
        )
        return PathSelection(
            frozenset({"LINUX_BACKEND", "LINUX_UI"}), True, languages
        )
    if path.startswith("protocol/"):
        return PathSelection(frozenset({"LINUX_BACKEND", "WINDOWS"}), True)
    if path in LEGAL_SHARED_EXACT or path.startswith("LICENSES/"):
        return PathSelection(
            frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"}), True
        )
    if (
        path.startswith("windows-client/tests/")
        or path == "windows-client/CodeCoverage.runsettings"
    ):
        return PathSelection(frozenset({"WINDOWS"}), False)
    if path.startswith("windows-client/tools/"):
        name = path.rsplit("/", 1)[-1]
        binary_impact = name.startswith(("Build-", "Collect-", "Install-", "New-"))
        return PathSelection(frozenset({"WINDOWS"}), binary_impact)
    if (
        path in WINDOWS_PRODUCT_ROOT_EXACT
        or path.startswith(("windows-client/src/", "windows-client/installer/"))
    ):
        languages = frozenset({"csharp"}) if path.endswith(".cs") else frozenset()
        return PathSelection(frozenset({"WINDOWS"}), True, languages)
    raise ScopeError(f"changed path has no CI owner: {path}")


def selection_for_paths(
    paths: Sequence[str],
    *,
    release_candidate: bool = False,
) -> Selection:
    owners: set[str] = set()
    languages: set[str] = set()
    powershell_paths: set[str] = set()
    binary_impact = False
    for path in paths:
        path_selection = _selection_for_path(path)
        owners.update(path_selection.owners)
        languages.update(path_selection.codeql_languages)
        if path.endswith(".ps1"):
            powershell_paths.add(path)
        binary_impact = binary_impact or path_selection.binary_impact
    if not owners:
        raise ScopeError("pull request contains no changed paths")
    if release_candidate and binary_impact:
        owners.add("WINDOWS")

    return Selection(
        owners=tuple(owner for owner in OWNER_ORDER if owner in owners),
        codeql_languages=tuple(
            language
            for language in ("actions", "csharp", "python", "rust")
            if language in languages
        ),
        powershell_paths=tuple(sorted(powershell_paths)),
        binary_impact=binary_impact,
        distribution_required=release_candidate and binary_impact,
    )


def paths_from_name_status(raw: bytes) -> tuple[str, ...]:
    """Parse name-status records; retain both rename ends and only a copy target."""
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
        record_paths = tuple(_path(value) for value in fields[index : index + path_count])
        paths.extend(record_paths if kind == "R" else record_paths[-1:])
        index += path_count
    return tuple(paths)


def selection_from_name_status(
    raw: bytes,
    *,
    release_candidate: bool = False,
) -> Selection:
    return selection_for_paths(
        paths_from_name_status(raw),
        release_candidate=release_candidate,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name-status", required=True, type=Path)
    parser.add_argument("--release-candidate", action="store_true")
    args = parser.parse_args(argv)
    try:
        result = selection_from_name_status(
            args.name_status.read_bytes(),
            release_candidate=args.release_candidate,
        )
    except (OSError, ScopeError) as exc:
        print(f"ci-change-scope: FAIL {exc}", file=sys.stderr)
        return 1
    print(result.as_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

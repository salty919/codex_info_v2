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

DOC_EXACT = frozenset({"README.md", "README.en.md", "DESIGN.md", "SECURITY.md"})
WINDOWS_TEST_SCRIPT_EXACT = frozenset(
    {
        "scripts/capture_windows_window.ps1",
        "scripts/windows_window_move_message_smoke.ps1",
        "scripts/windows_window_move_smoke.ps1",
    }
)
LINUX_PRODUCT_EXACT = frozenset(
    {
        "run.sh",
        "scripts/install_systemd_recorder.sh",
        "scripts/build_linux_bundle.sh",
    }
)
LINUX_TEST_EXACT = frozenset(
    {
        "scripts/cli_contract_e2e.sh",
        "scripts/data_protection_gate.sh",
        "scripts/db_protection_e2e.sh",
        "scripts/record_daemon_e2e.sh",
        "scripts/test_linux_bundle.sh",
        "scripts/test_linux_update_convergence.sh",
        "scripts/test_run_launcher_version_sync.sh",
    }
)
LINUX_UI_EXACT = frozenset(
    {
        "scripts/x11_graph_visual_gate.sh",
        "scripts/x11_service_recovery_visual_gate.sh",
        "scripts/x11_startup_visual_gate.sh",
    }
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
WINDOWS_PRODUCT_TOOL_EXACT = frozenset(
    {
        "windows-client/tools/Build-WindowsInstaller.ps1",
        "windows-client/tools/Collect-ThirdPartyNotices.ps1",
        "windows-client/tools/New-WindowsUpdateManifest.ps1",
    }
)
WINDOWS_TEST_TOOL_EXACT = frozenset(
    {
        "windows-client/tools/Measure-WindowsGraphLatency.ps1",
        "windows-client/tools/Run-WindowsClientE2E.ps1",
        "windows-client/tools/Test-WindowsClientFixtureContract.ps1",
    }
)
WINDOWS_GOVERNANCE_TOOL_EXACT = frozenset(
    {
        "windows-client/tools/Get-WindowsReleaseDecision.ps1",
        "windows-client/tools/Test-WindowsReleaseDecision.ps1",
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
    binary_impact: bool

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


@dataclass(frozen=True)
class PathSelection:
    """Quality, publication, and CodeQL effects for one changed path."""

    owners: frozenset[str]
    binary_impact: bool
    codeql_languages: frozenset[str] = frozenset()


def _path(value: str) -> str:
    if not value or "\x00" in value or value.startswith("/"):
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
    if path in WINDOWS_TEST_SCRIPT_EXACT:
        return PathSelection(frozenset({"WINDOWS"}), False)
    if path in LINUX_PRODUCT_EXACT or path.startswith("packaging/"):
        return PathSelection(frozenset({"LINUX_BACKEND"}), True)
    if path in LINUX_TEST_EXACT:
        return PathSelection(frozenset({"LINUX_BACKEND"}), False)
    if path.startswith("tests/fixtures/graph_"):
        return PathSelection(
            frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"}), False
        )
    if path.startswith("tests/"):
        return PathSelection(frozenset({"LINUX_BACKEND"}), False)
    if path.startswith("src/") and path != "src/main.rs":
        languages = frozenset({"rust"}) if path.endswith(".rs") else frozenset()
        return PathSelection(frozenset({"LINUX_BACKEND"}), True, languages)
    if path in LINUX_UI_EXACT:
        return PathSelection(frozenset({"LINUX_UI"}), False)
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
    if path in WINDOWS_TEST_TOOL_EXACT:
        return PathSelection(frozenset({"WINDOWS"}), False)
    if path in WINDOWS_GOVERNANCE_TOOL_EXACT:
        return PathSelection(frozenset({"GOVERNANCE"}), False)
    if (
        path in WINDOWS_PRODUCT_ROOT_EXACT
        or path in WINDOWS_PRODUCT_TOOL_EXACT
        or path.startswith(("windows-client/src/", "windows-client/installer/"))
    ):
        languages = frozenset({"csharp"}) if path.endswith(".cs") else frozenset()
        return PathSelection(frozenset({"WINDOWS"}), True, languages)
    if path.startswith("scripts/"):
        languages = frozenset({"python"}) if path.endswith(".py") else frozenset()
        return PathSelection(frozenset({"GOVERNANCE"}), False, languages)
    raise ScopeError(f"changed path has no CI owner: {path}")


def owners_for_path(path: str) -> frozenset[str]:
    """Return the quality owners for one repository path."""

    return _selection_for_path(path).owners


def selection_for_paths(
    paths: Sequence[str], *, release_candidate: bool = False
) -> Selection:
    owners: set[str] = set()
    languages: set[str] = set()
    binary_impact = False
    for path in paths:
        path_selection = _selection_for_path(path)
        owners.update(path_selection.owners)
        languages.update(path_selection.codeql_languages)
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
        binary_impact=binary_impact,
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


def selection_from_name_status(
    raw: bytes, *, release_candidate: bool = False
) -> Selection:
    return selection_for_paths(
        paths_from_name_status(raw), release_candidate=release_candidate
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name-status", required=True, type=Path)
    parser.add_argument("--release-candidate", action="store_true")
    args = parser.parse_args(argv)
    try:
        result = selection_from_name_status(
            args.name_status.read_bytes(), release_candidate=args.release_candidate
        )
    except (OSError, ScopeError) as exc:
        print(f"ci-change-scope: FAIL {exc}", file=sys.stderr)
        return 1
    print(result.as_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

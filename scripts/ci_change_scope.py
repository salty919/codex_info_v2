#!/usr/bin/env python3
"""Classify one complete Git diff into the quality owners that must run."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
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
PRODUCT_OWNERS = frozenset({"LINUX_BACKEND", "LINUX_UI", "WINDOWS"})
HISTORY_GRAPH_PROFILE = "history-graph"
MODEL_HISTORY_PROFILE = "model-history"
WORKFLOW_SELECTION_PROFILE = "workflow-selection"
HISTORY_GRAPH_PATHS = frozenset(
    {
        "docs/REST_API_V1.md",
        "src/main.rs",
        "src/usage_store.rs",
        "tests/fixtures/graph_delayed_quota.json",
        "windows-client/src/CodexInfo.WindowsClient/Controls/GraphPlotControl.cs",
        "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphPlotProjection.cs",
        "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphScene.cs",
        "windows-client/src/CodexInfo.WindowsClient/ViewModels/DetailsWindowViewModels.cs",
        "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/DetailsWindowViewModelTests.cs",
        "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs",
        "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/WindowDragGeometryTests.cs",
    }
)
WORKFLOW_SELECTION_PATHS = frozenset(
    {
        ".github/workflows/feat-integration.yml",
        ".github/workflows/linux-ui-quality.yml",
        ".github/workflows/rust.yml",
        ".github/workflows/selective-quality.yml",
        ".github/workflows/windows-client.yml",
        "docs/PRODUCT_REQUIREMENTS.md",
        "docs/REQUIREMENTS_LEDGER.md",
        "docs/WINDOWS_CLIENT_REQUIREMENTS.md",
        "docs/WINDOWS_UX_SPEC.md",
        "scripts/ci_change_scope.py",
        "scripts/pre_pr_gate.sh",
        "scripts/quality_plan.py",
        "scripts/regression_guard.sh",
        "scripts/requirements_authority.py",
        "scripts/selected_quality_gate.py",
        "scripts/test_ci_change_scope.py",
        "scripts/test_quality_plan.py",
        "scripts/test_requirements_authority.py",
        "scripts/test_selected_quality_gate.py",
        "scripts/windows_client_contract_gate.sh",
        "scripts/workflow_quality_gate.py",
    }
)
MODEL_HISTORY_PATHS = frozenset(
    {
        "docs/DATA_PROTECTION_POLICY.md",
        "docs/PRODUCT_REQUIREMENTS.md",
        "docs/REQUIREMENTS_LEDGER.md",
        "docs/REST_API_V1.md",
        "docs/WINDOWS_CLIENT.md",
        "docs/WINDOWS_CLIENT_REQUIREMENTS.md",
        "docs/WINDOWS_UX_SPEC.md",
        "src/daemon.rs",
        "src/main.rs",
        "src/server.rs",
        "src/usage_store.rs",
        "ui/app.slint",
        "ui/components.slint",
        "ui/theme.slint",
        "windows-client/src/CodexInfo.WindowsClient.Core/DetailsContracts.cs",
        "windows-client/src/CodexInfo.WindowsClient.Core/LoopbackStatusClient.cs",
        "windows-client/src/CodexInfo.WindowsClient/Controls/GraphPlotControl.cs",
        "windows-client/src/CodexInfo.WindowsClient/GraphWindow.axaml",
        "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphPlotProjection.cs",
        "windows-client/src/CodexInfo.WindowsClient/Graphing/GraphScene.cs",
        "windows-client/src/CodexInfo.WindowsClient/MainWindow.axaml",
        "windows-client/src/CodexInfo.WindowsClient/ViewModels/DetailsWindowViewModels.cs",
        "windows-client/src/CodexInfo.WindowsClient/ViewModels/MainWindowViewModel.cs",
        "windows-client/src/CodexInfo.WindowsClient/ViewModels/ModelUsageViewModel.cs",
        "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/LoopbackBoundaryCoverageTests.cs",
        "windows-client/tests/CodexInfo.WindowsClient.Core.Tests/LoopbackStatusClientTests.cs",
        "windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs",
    }
)
PROFILE_PATHS = {
    HISTORY_GRAPH_PROFILE: HISTORY_GRAPH_PATHS,
    MODEL_HISTORY_PROFILE: MODEL_HISTORY_PATHS,
    WORKFLOW_SELECTION_PROFILE: WORKFLOW_SELECTION_PATHS,
}
PROFILE_LINE_RE = re.compile(r"^Quality-Profile:[ \t]*([a-z0-9]+(?:-[a-z0-9]+)*)[ \t]*$")


class ScopeError(ValueError):
    """The diff cannot be mapped completely to the finite owner set."""


@dataclass(frozen=True)
class Selection:
    owners: tuple[str, ...]
    codeql_languages: tuple[str, ...]
    binary_impact: bool
    distribution_required: bool
    quality_profile: str

    def as_json(self) -> str:
        return json.dumps(
            {
                "binary_impact": self.binary_impact,
                "distribution_required": self.distribution_required,
                "owners": list(self.owners),
                "codeql_languages": list(self.codeql_languages),
                "quality_profile": self.quality_profile,
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


def quality_profile_from_document(text: str) -> str | None:
    """Read one exact finite profile declaration from PR prose."""

    declarations: list[str] = []
    for line in text.splitlines():
        if line.startswith("Quality-Profile:"):
            match = PROFILE_LINE_RE.fullmatch(line)
            if match is None:
                raise ScopeError("Quality-Profile declaration is malformed")
            declarations.append(match.group(1))
    if len(declarations) > 1:
        raise ScopeError("Quality-Profile declaration is duplicated")
    if not declarations:
        return None
    profile = declarations[0]
    if profile not in PROFILE_PATHS:
        raise ScopeError(f"Quality-Profile is unknown: {profile}")
    return profile


def _resolve_quality_profile(
    paths: Sequence[str],
    owners: set[str],
    *,
    release_candidate: bool,
    quality_profile: str | None,
) -> tuple[str, bool]:
    """Return the finite execution profile and distribution decision.

    A feat product diff without a registered profile stops here instead of
    expanding to every owner suite.  Release candidates deliberately ignore
    feat profiles and retain the complete distribution path.
    """

    product_change = bool(PRODUCT_OWNERS.intersection(owners))
    governance_change = "GOVERNANCE" in owners
    if release_candidate:
        if quality_profile is not None:
            raise ScopeError("feat Quality-Profile cannot narrow a release candidate")
        return "release", product_change
    if not product_change and not governance_change:
        if quality_profile is not None:
            raise ScopeError("Quality-Profile is unnecessary for a non-product diff")
        return "authority-only", False
    if quality_profile is None:
        raise ScopeError("feat product diff requires one finite Quality-Profile")
    if quality_profile not in PROFILE_PATHS:
        raise ScopeError(f"Quality-Profile is unknown: {quality_profile}")
    expected_paths = PROFILE_PATHS[quality_profile]
    if quality_profile == HISTORY_GRAPH_PROFILE and not product_change:
        raise ScopeError("history-graph profile has no product path")
    if quality_profile == MODEL_HISTORY_PROFILE and not product_change:
        raise ScopeError("model-history profile has no product path")
    if quality_profile == MODEL_HISTORY_PROFILE and owners != {
        "DOCS",
        "LINUX_BACKEND",
        "LINUX_UI",
        "WINDOWS",
    }:
        raise ScopeError(
            "model-history profile requires DOCS, LINUX_BACKEND, LINUX_UI, and WINDOWS"
        )
    if quality_profile == WORKFLOW_SELECTION_PROFILE and product_change:
        raise ScopeError("workflow-selection profile cannot own product code")
    outside = sorted(set(paths) - expected_paths)
    if outside:
        raise ScopeError(
            f"{quality_profile} profile does not own changed path: {outside[0]}"
        )
    return quality_profile, False


def selection_for_paths(
    paths: Sequence[str],
    *,
    release_candidate: bool = False,
    quality_profile: str | None = None,
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
    resolved_profile, distribution_required = _resolve_quality_profile(
        paths,
        owners,
        release_candidate=release_candidate,
        quality_profile=quality_profile,
    )
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
        distribution_required=distribution_required and binary_impact,
        quality_profile=resolved_profile,
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
    raw: bytes,
    *,
    release_candidate: bool = False,
    quality_profile: str | None = None,
) -> Selection:
    return selection_for_paths(
        paths_from_name_status(raw),
        release_candidate=release_candidate,
        quality_profile=quality_profile,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name-status", required=True, type=Path)
    parser.add_argument("--release-candidate", action="store_true")
    parser.add_argument(
        "--profile-document",
        type=Path,
        help="PR body containing one exact Quality-Profile declaration",
    )
    args = parser.parse_args(argv)
    try:
        quality_profile = (
            quality_profile_from_document(args.profile_document.read_text(encoding="utf-8"))
            if args.profile_document is not None
            else None
        )
        result = selection_from_name_status(
            args.name_status.read_bytes(),
            release_candidate=args.release_candidate,
            quality_profile=quality_profile,
        )
    except (OSError, ScopeError) as exc:
        print(f"ci-change-scope: FAIL {exc}", file=sys.stderr)
        return 1
    print(result.as_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

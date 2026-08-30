#!/usr/bin/env python3
"""Finite trust-boundary fixtures for versioning and final-head checks."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "version-prepare.yml"
QUALITY_WORKFLOW = ROOT / ".github" / "workflows" / "windows-client.yml"
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
REPORTER = ROOT / "scripts" / "final_head_check_reporter.py"
FEAT_WORKFLOW = ROOT / ".github" / "workflows" / "feat-integration.yml"
SELECTIVE_WORKFLOW = ROOT / ".github" / "workflows" / "selective-quality.yml"
FEAT_REPORTER = ROOT / "scripts" / "feat_integration_check_reporter.py"


def section(source: str, start: str, end: str | None) -> str:
    if source.count(start) != 1:
        raise AssertionError(f"non-unique section start: {start!r}")
    start_index = source.index(start)
    if end is None:
        return source[start_index:]
    if source.count(end) != 1:
        raise AssertionError(f"non-unique section end: {end!r}")
    return source[start_index : source.index(end, start_index + len(start))]


def validate(
    workflow: str, quality_workflow: str, release_workflow: str, reporter: str
) -> list[str]:
    errors: list[str] = []

    def exact(source: str, marker: str, expected: int = 1) -> None:
        actual = source.count(marker)
        if actual != expected:
            errors.append(f"count {marker!r}: expected {expected}, found {actual}")

    prepare = section(workflow, "  prepare:\n", "  register-final-head-checks:\n")
    register = section(workflow, "  register-final-head-checks:\n", "  quality:\n")
    quality = section(workflow, "  quality:\n", "  finalize-final-head-checks:\n")
    finalize = section(workflow, "  finalize-final-head-checks:\n", None)

    exact(workflow, "  pull_request_target:\n")
    exact(workflow, "  workflow_dispatch:\n", 0)
    exact(workflow, "  pull_request:\n", 0)
    exact(workflow, "permissions: {}\n")
    exact(
        workflow, "  group: version-prepare-${{ github.event.pull_request.number }}\n"
    )
    exact(workflow, "  cancel-in-progress: false\n")

    exact(prepare, "      contents: write\n")
    exact(prepare, "      pull-requests: read\n")
    exact(prepare, "      checks: write\n")
    exact(prepare, "          ref: refs/heads/main\n")
    exact(prepare, "          persist-credentials: false\n")
    exact(prepare, '"repos/$REPOSITORY/check-runs"')
    exact(prepare, '{name:"feat-acceptance",head_sha:$head_sha,status:"completed",')
    exact(prepare, '"$(jq -r \'.app.id\' <<<"$check_info")" == 15368')
    exact(prepare, '"repos/$REPOSITORY/git/refs/heads/$HEAD_REF"\n')
    exact(prepare, "            -F force=false \\\n")
    if prepare.index('"repos/$REPOSITORY/check-runs"') > prepare.index(
        '"$update_ref_endpoint"'
    ):
        errors.append("generated version check is created after the protected ref update")
    for marker in (
        '[[ "$HEAD_REPOSITORY" != "$REPOSITORY" ]]',
        '[[ "$observed_head" == "$HEAD_SHA" ]]',
        "\"repos/$REPOSITORY/git/commits/$HEAD_SHA\" --jq '.tree.sha'",
        "{message:$message,tree:$tree,parents:[$parent]}",
        '"$(jq -r \'.parents[0].sha\' <<<"$commit_info")" == "$HEAD_SHA"',
        'python3 scripts/product_version.py bump --expected "$base_version"',
        '[[ "$prepared_version" == "$next_version" ]]',
    ):
        exact(prepare, marker)
    exact(prepare, 'fetch_version_file "$REPOSITORY" "$BASE_SHA"', 3)
    exact(prepare, 'fetch_version_file "$HEAD_REPOSITORY" "$HEAD_SHA"', 3)
    exact(prepare, "git push", 0)
    exact(prepare, "actions/checkout@v4", 1)

    exact(register, "      checks: write\n")
    exact(register, "      contents: read\n")
    exact(register, "      pull-requests: read\n")
    exact(register, "          ref: refs/heads/main\n")
    exact(register, "scripts/final_head_check_reporter.py register")
    exact(register, "actions/checkout@v4", 1)
    exact(register, "      contents: write\n", 0)

    exact(quality, "uses: ./.github/workflows/windows-client.yml")
    exact(quality, "      premerge: true\n")
    exact(quality, "      head_sha: ${{ needs.prepare.outputs.final_head_sha }}\n")
    exact(quality, "      contents: read\n")
    exact(quality, "      pull-requests: read\n")
    exact(quality, "      security-events: write\n")
    exact(quality, "      checks: write\n", 0)

    exact(finalize, "      actions: read\n")
    exact(finalize, "      checks: write\n")
    exact(finalize, "      contents: read\n")
    exact(finalize, "      pull-requests: read\n")
    exact(finalize, "          ref: refs/heads/main\n")
    exact(finalize, "scripts/final_head_check_reporter.py finalize")
    exact(finalize, "actions/checkout@v4", 1)
    exact(finalize, "      contents: write\n", 0)

    exact(workflow, "/dispatches", 0)
    exact(workflow, "event=workflow_dispatch", 0)
    exact(quality_workflow, "  workflow_call:\n")
    exact(quality_workflow, "  pull_request_target:\n", 0)
    exact(quality_workflow, "  release:\n", 0)
    exact(quality_workflow, "contents: write\n", 0)
    exact(release_workflow, "  pull_request_target:\n")
    exact(release_workflow, "    types: [closed]\n")
    exact(release_workflow, "  workflow_call:\n", 0)
    exact(release_workflow, "  release:\n")
    exact(release_workflow, "      contents: write\n")
    exact(reporter, "GITHUB_ACTIONS_APP_ID = 15368")
    exact(reporter, 'CHECK_NAMES = ("version-prepared", "acceptance")')
    exact(reporter, "codex-quality-v1:pr=", 2)
    exact(reporter, "multiple GitHub Actions {name} checks exist on the final head")
    exact(reporter, "another active run owns a final-head required check")
    exact(reporter, "live pull-request identity does not match the final head")
    return errors


def validate_feat(feat: str, selective: str, reporter: str) -> list[str]:
    errors: list[str] = []

    def exact(source: str, marker: str, expected: int = 1) -> None:
        actual = source.count(marker)
        if actual != expected:
            errors.append(f"feat count {marker!r}: expected {expected}, found {actual}")

    register = section(feat, "  register-selection:\n", "  selective-quality:\n")
    quality = section(feat, "  selective-quality:\n", "  finalize-selection:\n")
    finalize = section(feat, "  finalize-selection:\n", None)
    permissions_index = feat.find("\npermissions:")
    trigger = feat[:permissions_index] if permissions_index >= 0 else feat

    exact(trigger, "  pull_request_target:\n")
    exact(trigger, '    branches: ["feat/next"]\n')
    exact(trigger, "  pull_request:\n", 0)
    exact(trigger, "  workflow_dispatch:\n", 0)
    exact(feat, "permissions: {}\n")
    exact(feat, "  cancel-in-progress: false\n")
    exact(feat, "contents: write\n", 0)
    exact(feat, "pull-requests: write\n", 0)

    exact(register, "      checks: write\n")
    exact(register, "      contents: read\n")
    exact(register, "      pull-requests: read\n")
    exact(register, "          ref: refs/heads/main\n")
    exact(register, "          persist-credentials: false\n")
    exact(register, '"repos/$REPOSITORY/pulls/$PR_NUMBER" > "$scope_root/before.json"\n')
    exact(register, '"repos/$REPOSITORY/pulls/$PR_NUMBER" > "$scope_root/after.json"\n')
    exact(register, "--paginate --slurp")
    exact(register, "--expected-base-ref feat/next")
    exact(register, '--expected-head-ref "$EVENT_HEAD_REF"')
    exact(register, '"$EVENT_HEAD_REPOSITORY" == "$REPOSITORY"')
    exact(register, "scripts/feat_integration_check_reporter.py register")

    exact(quality, "uses: ./.github/workflows/selective-quality.yml")
    exact(quality, "      contents: read\n")
    exact(quality, "      security-events: write\n")
    exact(quality, "      checks: write\n", 0)
    exact(quality, "      contents: write\n", 0)

    exact(finalize, "      checks: write\n")
    exact(finalize, "      contents: read\n")
    exact(finalize, "      pull-requests: read\n")
    exact(finalize, "          ref: refs/heads/main\n")
    exact(finalize, "scripts/feat_integration_check_reporter.py finalize")
    exact(finalize, "      contents: write\n", 0)

    exact(selective, "  workflow_call:\n")
    exact(selective, "  pull_request_target:\n", 0)
    exact(selective, "contents: write\n", 0)
    exact(selective, "checks: write\n", 0)
    exact(selective, "secrets:", 0)
    exact(selective, "          ref: refs/heads/main\n")
    exact(selective, "          ref: ${{ inputs.source_sha }}\n", 2)
    for owner, job in (
        ("DOCS", "docs-quality"),
        ("GOVERNANCE", "governance-quality"),
        ("LINUX_BACKEND", "linux-backend-quality"),
        ("LINUX_UI", "linux-ui-quality"),
        ("WINDOWS", "windows-quality"),
    ):
        exact(selective, f"  {job}:\n")
        exact(selective, f"    if: contains(fromJSON(inputs.selection_json).owners, '{owner}')\n")
    exact(selective, "    uses: ./.github/workflows/rust.yml\n")
    exact(selective, "    uses: ./.github/workflows/linux-ui-quality.yml\n")
    exact(selective, "    uses: ./.github/workflows/windows-client.yml\n")
    exact(selective, "      selective_mode: true\n")
    exact(selective, "    if: inputs.codeql_languages_json != '[]'\n")
    exact(selective, "      languages_json: ${{ inputs.codeql_languages_json }}\n")
    exact(selective, "  selected-quality:\n")
    exact(selective, "python3 scripts/selected_quality_gate.py")

    exact(reporter, 'CHECK_NAME = "feat-acceptance"')
    exact(reporter, "APP_ID = 15368")
    exact(reporter, "codex-feat-v1:pr=", 1)
    exact(reporter, 'pr["base"]["ref"] == "feat/next"')
    exact(reporter, "foreign, malformed, or duplicate-name feat check exists")
    exact(reporter, "live pull request identity moved or is malformed")
    return errors


def main() -> int:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    quality_workflow = QUALITY_WORKFLOW.read_text(encoding="utf-8")
    release_workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    reporter = REPORTER.read_text(encoding="utf-8")
    feat = FEAT_WORKFLOW.read_text(encoding="utf-8")
    selective = SELECTIVE_WORKFLOW.read_text(encoding="utf-8")
    feat_reporter = FEAT_REPORTER.read_text(encoding="utf-8")
    baseline = validate(workflow, quality_workflow, release_workflow, reporter)
    if baseline:
        raise AssertionError(
            "production CI trust contract failed: " + "; ".join(baseline)
        )
    feat_baseline = validate_feat(feat, selective, feat_reporter)
    if feat_baseline:
        raise AssertionError(
            "production feat CI trust contract failed: " + "; ".join(feat_baseline)
        )
    mutations = (
        ("workflow", "permissions: {}\n", "permissions:\n  contents: write\n"),
        ("workflow", "          ref: refs/heads/main\n", "          ref: feat/next\n"),
        (
            "workflow",
            "            -F force=false \\\n",
            "            -F force=true \\\n",
        ),
        (
            "workflow",
            '{name:"feat-acceptance",head_sha:$head_sha,status:"completed",',
            '{name:"acceptance",head_sha:$head_sha,status:"completed",',
        ),
        ("workflow", "      checks: write\n", "      checks: read\n"),
        ("workflow", "scripts/final_head_check_reporter.py register", "true"),
        (
            "workflow",
            "uses: ./.github/workflows/windows-client.yml",
            "uses: ./other.yml",
        ),
        ("workflow", "scripts/final_head_check_reporter.py finalize", "true"),
        (
            "workflow",
            "{message:$message,tree:$tree,parents:[$parent]}",
            "{message:$message,tree:$tree,parents:[]}",
        ),
        (
            "workflow",
            '[[ "$observed_head" == "$HEAD_SHA" ]]',
            "true",
        ),
        (
            "workflow",
            'python3 scripts/product_version.py bump --expected "$base_version"',
            "python3 scripts/product_version.py bump",
        ),
        (
            "quality_workflow",
            "permissions:\n  contents: read\n",
            "permissions:\n  contents: write\n",
        ),
        (
            "release_workflow",
            "    types: [closed]\n",
            "    types: [opened]\n",
        ),
        ("reporter", "GITHUB_ACTIONS_APP_ID = 15368", "GITHUB_ACTIONS_APP_ID = 1"),
        (
            "reporter",
            "multiple GitHub Actions {name} checks exist on the final head",
            "duplicate accepted",
        ),
    )
    cases = 1
    for target, needle, replacement in mutations:
        candidate_workflow = workflow
        candidate_quality_workflow = quality_workflow
        candidate_release_workflow = release_workflow
        candidate_reporter = reporter
        if target == "workflow":
            candidate_workflow = candidate_workflow.replace(needle, replacement, 1)
        elif target == "quality_workflow":
            candidate_quality_workflow = candidate_quality_workflow.replace(
                needle, replacement, 1
            )
        elif target == "release_workflow":
            candidate_release_workflow = candidate_release_workflow.replace(
                needle, replacement, 1
            )
        else:
            candidate_reporter = candidate_reporter.replace(needle, replacement, 1)
        if not validate(
            candidate_workflow,
            candidate_quality_workflow,
            candidate_release_workflow,
            candidate_reporter,
        ):
            raise AssertionError(f"CI trust mutation was accepted: {needle!r}")
        cases += 1
    feat_mutations = (
        ("feat", "permissions: {}\n", "permissions:\n  contents: write\n"),
        ("feat", "          ref: refs/heads/main\n", "          ref: feat/next\n"),
        ("feat", '"$EVENT_HEAD_REPOSITORY" == "$REPOSITORY"', "true"),
        ("feat", "--paginate --slurp", "--paginate"),
        ("feat", "scripts/feat_integration_check_reporter.py register", "true"),
        ("selective", "permissions:\n  contents: read\n", "permissions:\n  contents: write\n"),
        ("selective", "          ref: refs/heads/main\n", "          ref: ${{ inputs.source_sha }}\n"),
        ("selective", "    if: contains(fromJSON(inputs.selection_json).owners, 'WINDOWS')\n", "    if: always()\n"),
        ("reporter", "APP_ID = 15368", "APP_ID = 1"),
    )
    for target, needle, replacement in feat_mutations:
        candidate_feat = feat
        candidate_selective = selective
        candidate_reporter = feat_reporter
        if target == "feat":
            candidate_feat = candidate_feat.replace(needle, replacement, 1)
        elif target == "selective":
            candidate_selective = candidate_selective.replace(needle, replacement, 1)
        else:
            candidate_reporter = candidate_reporter.replace(needle, replacement, 1)
        if not validate_feat(candidate_feat, candidate_selective, candidate_reporter):
            raise AssertionError(f"feat CI trust mutation was accepted: {needle!r}")
        cases += 1
    print(f"ci-trust-fixture: PASS cases={cases}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

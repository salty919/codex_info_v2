#!/usr/bin/env python3
"""Local-only causal contract for the selective workflow graph."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import textwrap
from typing import Mapping, Sequence

import yaml


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_NAMES = (
    "codeql.yml",
    "feat-integration.yml",
    "linux-ui-quality.yml",
    "linux-distribution.yml",
    "release.yml",
    "rust.yml",
    "selective-quality.yml",
    "version-prepare.yml",
    "windows-client.yml",
)


def sources() -> dict[str, str]:
    return {
        name: (ROOT / ".github" / "workflows" / name).read_text(encoding="utf-8")
        for name in WORKFLOW_NAMES
    }


def _workflow_document(source: str) -> dict[str, object]:
    document = yaml.safe_load(source)
    if not isinstance(document, dict) or not isinstance(document.get("jobs"), dict):
        raise AssertionError("workflow document has no jobs mapping")
    return document


def _job(document: Mapping[str, object], job_id: str) -> dict[str, object]:
    jobs = document.get("jobs")
    if not isinstance(jobs, dict) or not isinstance(jobs.get(job_id), dict):
        raise AssertionError(f"workflow job was not found: {job_id}")
    return jobs[job_id]


def _step(
    job: Mapping[str, object],
    *,
    step_id: str | None = None,
    name: str | None = None,
    uses: str | None = None,
) -> dict[str, object]:
    steps = job.get("steps")
    if not isinstance(steps, list):
        raise AssertionError("workflow job has no steps")
    matches = [
        value
        for value in steps
        if isinstance(value, dict)
        and (step_id is None or value.get("id") == step_id)
        and (name is None or value.get("name") == name)
        and (uses is None or value.get("uses") == uses)
    ]
    if len(matches) != 1:
        identity = step_id or name or uses or "unspecified"
        raise AssertionError(f"workflow step is not unique: {identity}")
    return matches[0]


def _semantic_workflow_errors(workflows: Mapping[str, str]) -> list[str]:
    """Validate GitHub-interpreted handoffs bypassed by local fixtures."""

    errors: list[str] = []

    def expect(label: str, actual: object, expected: object) -> None:
        if actual != expected:
            errors.append(
                f"workflow wiring {label}: expected {expected!r}, found {actual!r}"
            )

    def mapping(label: str, actual: object, expected: Mapping[str, object]) -> None:
        if not isinstance(actual, dict):
            raise AssertionError(f"{label} mapping is missing")
        for key, value in expected.items():
            expect(f"{label}.{key}", actual.get(key), value)

    try:
        docs = {name: _workflow_document(text) for name, text in workflows.items()}
        feat = docs["feat-integration.yml"]
        version = docs["version-prepare.yml"]
        selective = docs["selective-quality.yml"]
        windows = docs["windows-client.yml"]
        linux_distribution = docs["linux-distribution.yml"]
        release = docs["release.yml"]
        feat_jobs = feat.get("jobs")
        if not isinstance(feat_jobs, dict):
            raise AssertionError("feat workflow jobs mapping is missing")
        if "feat-acceptance" in feat_jobs:
            errors.append("workflow wiring feat: redundant feat-acceptance job remains")
        feat_classify = _job(feat, "classify")
        feat_quality = _job(feat, "selective-quality")
        prepared = _job(version, "version-prepared")
        version_quality = _job(version, "selective-quality")
        acceptance = _job(version, "acceptance")
        selective_windows = _job(selective, "windows-quality")
        selective_codeql = _job(selective, "codeql-quality")
        selected = _job(selective, "selected-quality")
        windows_job = _job(windows, "windows-quality")
        linux_distribution_job = _job(linux_distribution, "linux-distribution")
        resolve = _job(release, "resolve")
        publish = _job(release, "publish")
        revalidate = _step(publish, step_id="revalidate")
        download = _step(publish, uses="actions/download-artifact@v4")
        release_token = _step(
            publish,
            uses="actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1",
        )
        publication = _step(publish, name="Publish or verify the exact release state")

        # Observer output -> owner execution and the complete, push-capable checkout.
        condition = "steps.current.outputs.observer != 'true'"
        checkout = _step(prepared, uses="actions/checkout@v5")
        expect("version.checkout.if", checkout.get("if"), condition)
        expect("version.prepare.if", _step(prepared, step_id="prepare").get("if"), condition)
        mapping("version.checkout", checkout.get("with"), {
            "ref": "${{ github.workflow_sha }}",
            "fetch-depth": 0,
            "persist-credentials": True,
        })
        version_prepare_script = _step(prepared, step_id="prepare").get("run")
        if (
            not isinstance(version_prepare_script, str)
            or 'git fetch --no-tags origin "$BASE_SHA" "$HEAD_SHA"'
            not in version_prepare_script
        ):
            errors.append(
                "workflow wiring version.prepare: exact base and head objects are not fetched"
            )

        # Producer metadata -> H1 and Release identity protocol.
        expect(
            "version.run-name",
            version.get("run-name"),
            "codex-main-quality-v1:pr=${{ github.event.pull_request.number }}:"
            "event_head=${{ github.event.pull_request.head.sha }}:"
            "action=${{ github.event.action }}:draft=${{ github.event.pull_request.draft }}",
        )

        # Version step outputs -> owner selection -> acceptance.
        for key, source in {
            "ready": "steps.prepare.outputs.ready",
            "selection_json": "steps.prepare.outputs.selection_json",
            "quality_sha": "steps.prepare.outputs.quality_sha",
            "generated_head": "steps.prepare.outputs.generated_head",
            "generated_observer": "steps.prepare.outputs.generated_observer",
            "event_observer": "steps.current.outputs.observer",
            "observer_reason": "steps.current.outputs.reason",
        }.items():
            mapping("version.outputs", prepared.get("outputs"), {key: f"${{{{ {source} }}}}"})
        acceptance_step = _step(acceptance, name="Resolve release quality result")
        acceptance_env = acceptance_step.get("env")
        expect("acceptance.permissions", acceptance.get("permissions"), None)
        expect("acceptance.env", acceptance_env, {
            "GENERATED_OBSERVER": "${{ needs.version-prepared.outputs.generated_observer }}",
            "EVENT_OBSERVER": "${{ needs.version-prepared.outputs.event_observer }}",
            "PREPARE_RESULT": "${{ needs.version-prepared.result }}",
            "QUALITY_RESULT": "${{ needs.selective-quality.result }}",
        })

        # Complete PR identity -> reusable owner calls -> each leaf checkout.
        mapping(
            "feat.classify.checkout",
            _step(feat_classify, uses="actions/checkout@v5").get("with"),
            {
                "ref": "${{ github.workflow_sha }}",
                "fetch-depth": 0,
                "persist-credentials": False,
            },
        )
        mapping("feat.classify.outputs", feat_classify.get("outputs"), {
            "selection_json": "${{ steps.classify.outputs.selection_json }}",
        })
        feat_classify_script = _step(feat_classify, step_id="classify").get("run")
        if (
            not isinstance(feat_classify_script, str)
            or 'git fetch --no-tags origin "$BASE_SHA" "$HEAD_SHA"'
            not in feat_classify_script
        ):
            errors.append(
                "workflow wiring feat.classify: exact base and head objects are not fetched"
            )
        expect("feat.quality.needs", feat_quality.get("needs"), ["classify"])
        expect("feat.quality.uses", feat_quality.get("uses"), "./.github/workflows/selective-quality.yml")
        expect("feat.classify.continue-on-error", feat_classify.get("continue-on-error"), None)
        expect("feat.quality.continue-on-error", feat_quality.get("continue-on-error"), None)
        mapping("feat.quality", feat_quality.get("with"), {
            "source_sha": "${{ github.event.pull_request.head.sha }}",
            "base_sha": "${{ github.event.pull_request.base.sha }}",
            "head_ref": "${{ github.event.pull_request.head.ref }}",
            "pr_number": "${{ github.event.pull_request.number }}",
            "selection_json": "${{ needs.classify.outputs.selection_json }}",
            "release_candidate": False,
        })
        expect("version.quality.needs", version_quality.get("needs"), ["version-prepared"])
        expect("version.quality.if", version_quality.get("if"), "needs.version-prepared.outputs.ready == 'true'")
        expect("version.quality.uses", version_quality.get("uses"), "./.github/workflows/selective-quality.yml")
        expect("version.quality.continue-on-error", version_quality.get("continue-on-error"), None)
        mapping("version.quality", version_quality.get("with"), {
            "source_sha": "${{ needs.version-prepared.outputs.quality_sha }}",
            "base_sha": "${{ github.event.pull_request.base.sha }}",
            "head_ref": "${{ github.event.pull_request.head.ref }}",
            "pr_number": "${{ github.event.pull_request.number }}",
            "selection_json": "${{ needs.version-prepared.outputs.selection_json }}",
            "release_candidate": True,
        })
        expect("version.acceptance.needs", acceptance.get("needs"), ["version-prepared", "selective-quality"])
        expect("version.acceptance.continue-on-error", acceptance.get("continue-on-error"), None)
        child_workflows = {
            "linux-backend-quality": "./.github/workflows/rust.yml",
            "linux-ui-quality": "./.github/workflows/linux-ui-quality.yml",
            "windows-quality": "./.github/workflows/windows-client.yml",
            "codeql-quality": "./.github/workflows/codeql.yml",
        }
        for job_id, uses in child_workflows.items():
            child = _job(selective, job_id)
            expect(f"selective.{job_id}.uses", child.get("uses"), uses)
            expect(
                f"selective.{job_id}.continue-on-error",
                child.get("continue-on-error"),
                None,
            )
            mapping(f"selective.{job_id}", child.get("with"), {
                "source_sha": "${{ inputs.source_sha }}"
            })
        distribution = _job(selective, "linux-distribution")
        expect(
            "selective.linux-distribution.uses",
            distribution.get("uses"),
            "./.github/workflows/linux-distribution.yml",
        )
        expect(
            "selective.linux-distribution.if",
            distribution.get("if"),
            "fromJSON(inputs.selection_json).binary_impact == true",
        )
        expect(
            "selective.linux-distribution.continue-on-error",
            distribution.get("continue-on-error"),
            None,
        )
        mapping("selective.linux-distribution", distribution.get("with"), {
            "source_sha": "${{ inputs.source_sha }}",
            "pr_number": "${{ inputs.pr_number }}",
            "release_candidate": "${{ inputs.release_candidate }}",
        })
        mapping("selective.windows", selective_windows.get("with"), {
            "pr_number": "${{ inputs.pr_number }}",
            "release_candidate": "${{ inputs.release_candidate }}",
        })
        mapping("selective.codeql", selective_codeql.get("with"), {
            "head_ref": "${{ inputs.head_ref }}",
            "languages_json": "${{ toJSON(fromJSON(inputs.selection_json).codeql_languages) }}",
        })
        for document, job_id in (
            (selective, "docs-quality"),
            (selective, "governance-quality"),
            (docs["rust.yml"], "native-quality"),
            (docs["linux-ui-quality.yml"], "linux-ui-quality"),
            (windows, "windows-quality"),
            (linux_distribution, "linux-distribution"),
            (docs["codeql.yml"], "analyze"),
        ):
            leaf_job = _job(document, job_id)
            expect(f"{job_id}.continue-on-error", leaf_job.get("continue-on-error"), None)
            leaf_checkout = _step(leaf_job, uses="actions/checkout@v5")
            mapping(f"{job_id}.checkout", leaf_checkout.get("with"), {
                "ref": "${{ inputs.source_sha }}"
            })
        expect("selected.needs", selected.get("needs"), [
            "docs-quality", "governance-quality", "linux-backend-quality",
            "linux-ui-quality", "windows-quality", "codeql-quality",
            "linux-distribution",
        ])
        expect("selected.if", selected.get("if"), "always() && inputs.release_candidate")
        expect("selected.continue-on-error", selected.get("continue-on-error"), None)
        mapping(
            "selected.checkout",
            _step(selected, uses="actions/checkout@v5").get("with"),
            {
                "ref": "${{ github.workflow_sha }}",
                "persist-credentials": False,
            },
        )
        mapping(
            "selected.results",
            _step(selected, name="Require selected success and non-selected skip").get("env"),
            {
                "SELECTION": "${{ inputs.selection_json }}",
                "RELEASE_CANDIDATE": "${{ inputs.release_candidate }}",
                "RESULTS": "${{ toJSON(needs) }}",
            },
        )
        selected_script = _step(
            selected, name="Require selected success and non-selected skip"
        ).get("run")
        if not isinstance(selected_script, str) or "--release-candidate \"$RELEASE_CANDIDATE\"" not in selected_script:
            errors.append(
                "workflow wiring selected.run: release-candidate context is not passed to the gate"
            )
        if isinstance(selected_script, str) and "selected_quality_gate.py --help" in selected_script:
            errors.append(
                "workflow wiring selected.run: cross-generation compatibility probe remains"
            )

        # Windows selected/skipped signal and artifact identity.
        expect("windows.job-set", set(windows["jobs"]), {"windows-quality"})
        expect("version.windows-name", version_quality.get("name"), "Run selected quality owners")
        expect("selective.windows-name", selective_windows.get("name"), "windows-quality")
        expect("windows.leaf-name", windows_job.get("name"), "windows-quality")
        e2e = _step(windows_job, name="Run installed Windows UI Automation E2E")
        mapping("windows.e2e.env", e2e.get("env"), {
            "SOURCE_SHA": "${{ inputs.source_sha }}",
        })
        if "-SourceSha $env:SOURCE_SHA" not in str(e2e.get("run", "")):
            errors.append("workflow wiring windows.e2e.run: source SHA is not passed to E2E")
        manifest = _step(windows_job, step_id="manifest")
        mapping("windows.manifest.env", manifest.get("env"), {
            "REPOSITORY": "${{ github.repository }}",
        })
        upload = _step(windows_job, uses="actions/upload-artifact@v4")
        expect("windows.upload.if", upload.get("if"), "inputs.release_candidate")
        mapping("windows.upload", upload.get("with"), {
            "name": "release-candidate-v1-pr-${{ inputs.pr_number }}-head-${{ inputs.source_sha }}-"
            "run-${{ github.run_id }}-attempt-${{ github.run_attempt }}-"
            "version-${{ steps.manifest.outputs.version }}",
            "if-no-files-found": "error",
        })

        # Resolver outputs -> lock holder; revalidation controls both side effects.
        output_keys = (
            "publish", "fingerprint", "tag", "version", "pr_number", "final_head",
            "merge_sha", "run_id", "run_number", "run_attempt", "artifact_id",
            "artifact_name", "artifact_digest", "artifact_ids", "linux_present",
            "windows_present",
        )
        for key in output_keys:
            mapping("release.outputs", resolve.get("outputs"), {
                key: f"${{{{ steps.resolve.outputs.{key} }}}}"
            })
        expect("release.publish.needs", publish.get("needs"), ["resolve"])
        expect("release.publish.if", publish.get("if"), "needs.resolve.outputs.publish == 'true'")
        for env_key, output_key in {
            "ARTIFACT_DIGEST": "artifact_digest", "ARTIFACT_ID": "artifact_id",
            "ARTIFACT_IDS": "artifact_ids",
            "ARTIFACT_NAME": "artifact_name", "FINAL_HEAD": "final_head",
            "FINGERPRINT": "fingerprint", "MERGE_SHA": "merge_sha",
            "LINUX_PRESENT": "linux_present",
            "PR_NUMBER": "pr_number", "RUN_ATTEMPT": "run_attempt",
            "RUN_ID": "run_id", "RUN_NUMBER": "run_number", "VERSION": "version",
            "WINDOWS_PRESENT": "windows_present",
        }.items():
            mapping("release.revalidate", revalidate.get("env"), {
                env_key: f"${{{{ needs.resolve.outputs.{output_key} }}}}"
            })
        mapping("release.revalidate", revalidate.get("env"), {
            "EVENT_NAME": "${{ github.event_name }}",
            "GH_TOKEN": "${{ github.token }}",
            "REPOSITORY": "${{ github.repository }}",
        })
        mapping("release.download", download.get("with"), {
            "artifact-ids": "${{ needs.resolve.outputs.artifact_ids }}",
            "run-id": "${{ needs.resolve.outputs.run_id }}",
            "github-token": "${{ github.token }}",
            "repository": "${{ github.repository }}",
            "path": "release-candidate",
        })
        mapping("release.publish.permissions", publish.get("permissions"), {
            "contents": "read",
        })
        expect("release.token.id", release_token.get("id"), "release-token")
        expect("release.token.inputs", release_token.get("with"), {
            "client-id": "${{ vars.RELEASE_APP_CLIENT_ID }}",
            "private-key": "${{ secrets.RELEASE_APP_PRIVATE_KEY }}",
            "permission-contents": "write",
            "permission-workflows": "write",
        })
        expect("release.publication.env", publication.get("env"), {
            "GH_TOKEN": "${{ steps.release-token.outputs.token }}",
            "FINAL_HEAD": "${{ needs.resolve.outputs.final_head }}",
            "LINUX_PRESENT": "${{ needs.resolve.outputs.linux_present }}",
            "MERGE_SHA": "${{ needs.resolve.outputs.merge_sha }}",
            "REPOSITORY": "${{ github.repository }}",
            "TAG": "${{ needs.resolve.outputs.tag }}",
            "VERSION": "${{ needs.resolve.outputs.version }}",
            "WINDOWS_PRESENT": "${{ needs.resolve.outputs.windows_present }}",
        })
        proceed = "steps.revalidate.outputs.proceed == 'true'"
        expect("release.download.if", download.get("if"), proceed)
        expect("release.token.if", release_token.get("if"), proceed)
        expect("release.publication.if", publication.get("if"), proceed)
        token_output = "${{ steps.release-token.outputs.token }}"

        def occurrences(value: object) -> int:
            if isinstance(value, dict):
                return sum(occurrences(item) for item in value.values())
            if isinstance(value, list):
                return sum(occurrences(item) for item in value)
            return int(value == token_output)

        expect("release.token.output-consumers", occurrences(release), 1)

        # Jobs API names are derived from the actual producer names, not a second fixture.
        resolver_script = _step(resolve, step_id="resolve").get("run")
        revalidate_script = revalidate.get("run")
        if not isinstance(resolver_script, str) or not isinstance(revalidate_script, str):
            raise AssertionError("release authority scripts are missing")
        version_script = _step_script(
            workflows["version-prepare.yml"],
            "Select owners and prepare the binary version",
        )
        if '--name-status "$changes" --release-candidate' not in version_script:
            errors.append(
                "workflow wiring version selection: main candidate context is missing"
            )
        feat_script = _step_script(
            workflows["feat-integration.yml"],
            "Classify the event's exact base and head",
        )
        if "--release-candidate" in feat_script:
            errors.append(
                "workflow wiring feat selection: release candidate context leaked into feat integration"
            )
        wrapper = f"{version_quality['name']} / {selective_windows['name']}"
        leaf = f"{wrapper} / {windows_job['name']}"
        for script, assignments in (
            (
                resolver_script,
                (
                    f"windows_wrapper='{wrapper}'",
                    f"windows_leaf='{leaf}'",
                    "linux_wrapper='Run selected quality owners / linux-distribution'",
                    "linux_leaf='Run selected quality owners / linux-distribution / linux-distribution'",
                ),
            ),
            (
                revalidate_script,
                (
                    f"windows_leaf='{leaf}'",
                    "linux_wrapper='Run selected quality owners / linux-distribution'",
                    "linux_leaf='Run selected quality owners / linux-distribution / linux-distribution'",
                ),
            ),
        ):
            for assignment in assignments:
                if assignment not in script:
                    errors.append(f"workflow wiring release job identity: missing {assignment}")
    except (AssertionError, KeyError, TypeError) as error:
        errors.append(f"workflow semantic graph is incomplete: {error}")
    return errors


def validate(workflows: Mapping[str, str]) -> list[str]:
    errors: list[str] = []

    def count(name: str, marker: str, expected: int) -> None:
        actual = workflows[name].count(marker)
        if actual != expected:
            errors.append(f"{name}: {marker!r}: expected {expected}, found {actual}")

    if set(workflows) != set(WORKFLOW_NAMES):
        errors.append("workflow set differs from the declared workflow graph")
        return errors

    joined = "\n".join(workflows.values())
    for forbidden in (
        "feat_integration_check_reporter.py",
        "final_head_check_reporter.py",
        "acceptance-verdict",
        "final_acceptance_gate.sh",
        "release_state_gate.py",
        "ci_trust_fixture.py",
        "scripts/regression_guard.sh",
        "scripts/data_protection_gate.sh",
        "scripts/windows_client_contract_gate.sh",
        "workflow_quality_gate.py --self-test",
        "test_ci_change_scope.py",
        "test_selected_quality_gate.py",
    ):
        if forbidden in joined:
            errors.append(f"obsolete or local-only mechanism remains in Actions: {forbidden}")

    count("feat-integration.yml", "scripts/ci_change_scope.py", 1)
    count("version-prepare.yml", "scripts/ci_change_scope.py", 1)
    for name in set(WORKFLOW_NAMES) - {"feat-integration.yml", "version-prepare.yml"}:
        count(name, "scripts/ci_change_scope.py", 0)

    feat = workflows["feat-integration.yml"]
    for marker in (
        'branches: ["feat/next"]',
        "release_candidate: false",
        '--name-status -z "$BASE_SHA...$HEAD_SHA"',
    ):
        if marker not in feat:
            errors.append(f"feat-integration.yml: missing {marker}")
    for marker in ('"$HEAD_REF" !=', "workflow_dispatch:"):
        if marker in feat:
            errors.append(f"feat-integration.yml: branch/dispatch overconstraint {marker}")
    count("feat-integration.yml", "feat-acceptance", 0)
    count("feat-integration.yml", "--find-copies-harder", 1)

    version = workflows["version-prepare.yml"]
    for marker in (
        "name: version-prepared",
        "run-name: codex-main-quality-v1:",
        "'Observe generated H1' || 'acceptance'",
        "ref: ${{ github.workflow_sha }}",
        "git log --first-parent --format=%H",
        "expected_version_transition=true",
        "generated_observer=true",
        "':(exclude)Cargo.toml'",
        "cmp -s",
        'git push origin "$commit_sha:refs/heads/$HEAD_REF"',
        "Codex-Version-Prepare-Run-Attempt: $GITHUB_RUN_ATTEMPT",
        "expected_generated_message=",
        '"$commit_message" == "$expected_generated_message"',
        "actions/runs/$generator_run_id/attempts/$generator_run_attempt",
        "release_candidate: true",
    ):
        if marker not in version:
            errors.append(f"version-prepare.yml: missing {marker}")
    for marker in (
        "--force",
        "actions/checkout@v5\n        with:\n          ref: ${{ github.event.pull_request.head.sha }}",
    ):
        if marker in version:
            errors.append(f"version-prepare.yml: write path overconstraint {marker}")
    count("version-prepare.yml", "checks: write", 0)
    count("version-prepare.yml", "cancel-in-progress: false", 1)
    count("version-prepare.yml", "repos/$REPOSITORY/check-runs", 0)
    count("version-prepare.yml", "--find-copies-harder", 2)

    selective = workflows["selective-quality.yml"]
    for owner in (
        "docs-quality",
        "governance-quality",
        "linux-backend-quality",
        "linux-ui-quality",
        "windows-quality",
        "codeql-quality",
    ):
        if f"  {owner}:\n" not in selective:
            errors.append(f"selective-quality.yml: missing {owner}")
    if "selected_quality_gate.py" not in selective:
        errors.append("selective-quality.yml: selected result aggregation is missing")
    count(
        "selective-quality.yml",
        "if: always() && inputs.release_candidate",
        1,
    )
    count("selective-quality.yml", "ref: ${{ inputs.base_sha }}", 0)
    count("selective-quality.yml", "ref: ${{ github.workflow_sha }}", 1)

    linux_distribution = workflows["linux-distribution.yml"]
    for marker in (
        "runs-on: ubuntu-22.04",
        "LINUX_BUNDLE_TARGET: x86_64-unknown-linux-gnu",
        "cargo build --release --locked --target",
        "scripts/build_linux_bundle.sh",
        "scripts/test_linux_bundle.sh",
        "uses: actions/upload-artifact@v4",
        "release-candidate-linux-v1-pr-${{ inputs.pr_number }}",
    ):
        if marker not in linux_distribution:
            errors.append(f"linux-distribution.yml: missing {marker}")
    count("linux-distribution.yml", "uses: actions/upload-artifact@v4", 1)

    windows = workflows["windows-client.yml"]
    for marker in (
        "dotnet test windows-client/CodexInfo.WindowsClient.sln",
        "Build-WindowsInstaller.ps1",
        "Run-WindowsClientE2E.ps1",
        "windows_window_move_smoke.ps1",
        "$moveSmokeOutput = @(& ./scripts/windows_window_move_smoke.ps1",
        "[string]$moveSmokeOutput[-1] -ne 'window-move-smoke: PASS'",
        "New-WindowsUpdateManifest.ps1",
        "name: release-candidate-v1-pr-${{ inputs.pr_number }}",
    ):
        if marker not in windows:
            errors.append(f"windows-client.yml: missing {marker}")
    for forbidden in (
        "Measure-WindowsGraphLatency.ps1",
        "if ($LASTEXITCODE -ne 0) { throw 'Physical window move smoke failed.' }",
    ):
        if forbidden in windows:
            errors.append(f"windows-client.yml: unrelated or stale gate remains: {forbidden}")
    count("windows-client.yml", "uses: actions/upload-artifact@v4", 1)

    rust = workflows["rust.yml"]
    for marker in (
        "cargo test --locked --all-targets -- --nocapture",
        "cargo build --release --locked",
        "scripts/cli_contract_e2e.sh",
        "scripts/record_daemon_e2e.sh",
        "xvfb-run --auto-servernum",
    ):
        if marker not in rust:
            errors.append(f"rust.yml: missing {marker}")
    if "upload-artifact" in rust:
        errors.append("rust.yml: evidence-only artifact remains")

    codeql = workflows["codeql.yml"]
    if "  schedule:\n" in codeql:
        errors.append("codeql.yml: undeclared weekly schedule remains")
    for marker in (
        "fromJSON(inputs.languages_json)",
        "sha: ${{ inputs.source_sha }}",
        "ref: refs/heads/${{ inputs.head_ref }}",
    ):
        if marker not in codeql:
            errors.append(f"codeql.yml: missing selected-source contract {marker}")

    release = workflows["release.yml"]
    for marker in (
        'types: [closed]',
        'workflows: ["Main PR quality"]',
        "name: Resolve the immutable publication snapshot",
        "--paginate --slurp",
        "a same-final-head attempt is pending or concluded non-success",
        "total_count",
        "branch=$head_ref_encoded&per_page=100",
        "union_signal_run",
        "release-candidate-v1-pr-",
        "release-candidate-linux-v1-pr-",
        "linux-distribution",
        "name: Revalidate authority after acquiring the tag lock",
        "group: release-windows-client-${{ needs.resolve.outputs.tag }}",
        "final-head run set or latest attempt changed",
        "name: Publish or verify the exact release state",
        'api --method POST "repos/$REPOSITORY/releases"',
        "curl -L --fail-with-body --silent --show-error",
        '"$upload_base?name=$filename"',
        ".upload_url | select(type == \"string\")",
        'api --method PATCH "repos/$REPOSITORY/releases/$release_id"',
        "existing release is draft or partial; automatic repair is forbidden",
        "orphan tag or release-without-tag state; automatic repair is forbidden",
    ):
        if marker not in release:
            errors.append(f"release.yml: missing {marker}")
    if "actions/checkout" in release:
        errors.append("release.yml: write job must not checkout source")
    if "status=success" in release:
        errors.append("release.yml: latest failure must not fall back to an older success")
    count("release.yml", "--paginate --slurp", 3)

    errors.extend(_semantic_workflow_errors(workflows))

    return errors


def _step_script(workflow: str, step_name: str) -> str:
    document = _workflow_document(workflow)
    for job in document["jobs"].values():
        for step in job.get("steps", []):
            if step.get("name") == step_name:
                script = step.get("run")
                if isinstance(script, str) and script:
                    return script
    raise AssertionError(f"workflow step was not found: {step_name}")


def _selected_quality_release_candidate_tests(selective_workflow: str) -> int:
    """Exercise both classifiers and the reachable main Release aggregation.

    The workflow and helper are checked out from one immutable workflow SHA.
    The feat caller does not run the aggregate shell, so only main Release
    candidate inputs are exercised through that shell.
    """
    script = _step_script(
        selective_workflow, "Require selected success and non-selected skip"
    )
    owner_jobs = {
        "DOCS": "docs-quality",
        "GOVERNANCE": "governance-quality",
        "LINUX_BACKEND": "linux-backend-quality",
        "LINUX_UI": "linux-ui-quality",
        "WINDOWS": "windows-quality",
    }
    changes = ROOT / "scripts" / ".selected-quality-release-candidate-changes.z"
    cases = 0

    def classify(path: str, *, release_candidate: bool) -> str:
        changes.write_bytes(f"M\0{path}\0".encode())
        command = (
            "python3",
            "scripts/ci_change_scope.py",
            "--name-status",
            str(changes),
        )
        if release_candidate:
            command += ("--release-candidate",)
        return _command(command, cwd=ROOT).stdout.strip()

    def results(selection_raw: str) -> str:
        selection = json.loads(selection_raw)
        owners = set(selection["owners"])
        selected_jobs = {owner_jobs[owner] for owner in owners}
        return json.dumps(
            {
                job: {
                    "result": (
                        "success"
                        if job in selected_jobs
                        or (job == "codeql-quality" and bool(selection["codeql_languages"]))
                        or (job == "linux-distribution" and selection["binary_impact"])
                        else "skipped"
                    )
                }
                for job in (
                    "docs-quality",
                    "governance-quality",
                    "linux-backend-quality",
                    "linux-ui-quality",
                    "windows-quality",
                    "codeql-quality",
                    "linux-distribution",
                )
            },
            separators=(",", ":"),
        )

    try:
        environment = os.environ.copy()
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
        for path in ("src/lib.rs", "ui/app.slint"):
            non_candidate = classify(path, release_candidate=False)
            fixed = classify(path, release_candidate=True)
            if "WINDOWS" in json.loads(non_candidate)["owners"]:
                raise AssertionError(f"non-candidate classifier selected WINDOWS for {path}")
            if "WINDOWS" not in json.loads(fixed)["owners"]:
                raise AssertionError(
                    f"release-candidate classifier omitted WINDOWS for {path}"
                )

            environment.update(
                {
                    "SELECTION": non_candidate,
                    "RELEASE_CANDIDATE": "true",
                    "RESULTS": results(non_candidate),
                }
            )
            rejected = _command(
                ("bash", "-c", script), cwd=ROOT, env=environment, check=False
            )
            if rejected.returncode == 0 or "must select WINDOWS" not in rejected.stderr:
                raise AssertionError(
                    f"Linux-only candidate was not rejected for {path}"
                )
            cases += 1

            environment.update({"SELECTION": fixed, "RESULTS": results(fixed)})
            accepted = _command(
                ("bash", "-c", script), cwd=ROOT, env=environment, check=False
            )
            if accepted.returncode != 0:
                raise AssertionError(
                    f"fixed main classifier output was rejected for {path}: "
                    f"{accepted.stderr}"
                )
            cases += 1

        docs = json.dumps(
            {
                "owners": ["DOCS"],
                "codeql_languages": [],
                "binary_impact": False,
            },
            separators=(",", ":"),
        )
        environment.update(
            {
                "SELECTION": docs,
                "RELEASE_CANDIDATE": "true",
                "RESULTS": results(docs),
            }
        )
        accepted_docs = _command(("bash", "-c", script), cwd=ROOT, env=environment, check=False)
        if accepted_docs.returncode != 0:
            raise AssertionError(
                f"non-binary release candidate was rejected: {accepted_docs.stderr}"
            )
        cases += 1
    finally:
        changes.unlink(missing_ok=True)
    return cases


def _command(
    args: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        list(args),
        cwd=cwd,
        env=None if env is None else dict(env),
        text=True,
        capture_output=True,
    )
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed ({result.returncode}): {' '.join(args)}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def _git(cwd: Path, *args: str) -> str:
    return _command(("git", *args), cwd=cwd).stdout.strip()


def _output(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            key, separator, value = line.partition("=")
            if separator:
                values[key] = value
    return values


def _git_copy_detection_test() -> int:
    """Prove the production diff flags emit both ends of an unchanged-source copy."""
    with tempfile.TemporaryDirectory(prefix="codex-info-copy-diff-") as raw_root:
        root = Path(raw_root)
        _git(root, "init", "--quiet")
        _git(root, "config", "user.name", "fixture")
        _git(root, "config", "user.email", "fixture@example.invalid")
        (root / "README.md").write_text("copy source\n", encoding="utf-8")
        _git(root, "add", "README.md")
        _git(root, "commit", "--quiet", "-m", "base")
        (root / ".github").mkdir()
        shutil.copy2(root / "README.md", root / ".github/copied.md")
        _git(root, "add", "-N", ".github/copied.md")
        result = subprocess.run(
            (
                "git",
                "diff",
                "--find-renames=50%",
                "--find-copies=50%",
                "--find-copies-harder",
                "--name-status",
                "-z",
                "HEAD",
            ),
            cwd=root,
            capture_output=True,
            check=True,
        )
        expected = b"C100\0README.md\0.github/copied.md\0"
        if result.stdout != expected:
            raise AssertionError(f"Git copy record is wrong: {result.stdout!r}")
        changes = root / "changes.z"
        changes.write_bytes(result.stdout)
        selection = json.loads(
            _command(
                (
                    "python3",
                    str(ROOT / "scripts/ci_change_scope.py"),
                    "--name-status",
                    str(changes),
                ),
                cwd=root,
            ).stdout
        )
        if selection["owners"] != ["DOCS", "GOVERNANCE"]:
            raise AssertionError(f"copy endpoints selected {selection['owners']}")
    return 1


def _checked_version(cwd: Path) -> str:
    output = _command(("python3", "scripts/product_version.py", "check"), cwd=cwd).stdout
    versions = [line.removeprefix("version=") for line in output.splitlines() if line.startswith("version=")]
    if len(versions) != 1:
        raise AssertionError(f"version fixture returned {len(versions)} version rows")
    return versions[0]


def _new_version_fixture(root: Path, name: str) -> dict[str, Path | str | int]:
    case_root = root / name
    remote = case_root / "remote.git"
    seed = case_root / "seed"
    remote.mkdir(parents=True)
    seed.mkdir()
    _git(remote, "init", "--bare", "--quiet")
    _git(seed, "init", "--quiet")
    _git(seed, "config", "user.name", "fixture")
    _git(seed, "config", "user.email", "fixture@example.invalid")

    for relative in (
        "Cargo.toml",
        "Cargo.lock",
        "windows-client/Directory.Build.props",
        "scripts/product_version.py",
        "scripts/ci_change_scope.py",
    ):
        destination = seed / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, destination)
    for relative, content in (
        ("README.md", "base\n"),
        ("src/lib.rs", "pub fn base() {}\n"),
        ("windows-client/src/App.cs", "class App {}\n"),
    ):
        destination = seed / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content, encoding="utf-8")

    _git(seed, "checkout", "-b", "main")
    _git(seed, "add", ".")
    _git(seed, "commit", "--quiet", "-m", "base")
    base = _git(seed, "rev-parse", "HEAD")
    _git(seed, "remote", "add", "origin", str(remote))
    _git(seed, "push", "--quiet", "-u", "origin", "main")
    _git(remote, "symbolic-ref", "HEAD", "refs/heads/main")
    _git(seed, "checkout", "-b", "case")
    version = _checked_version(seed)
    return {
        "base": base,
        "counter": 0,
        "remote": remote,
        "seed": seed,
        "version": version,
    }


def _commit(fixture: dict[str, Path | str | int], message: str) -> str:
    seed = fixture["seed"]
    assert isinstance(seed, Path)
    _git(seed, "add", ".")
    _git(seed, "commit", "--quiet", "-m", message)
    head = _git(seed, "rev-parse", "HEAD")
    _git(seed, "push", "--quiet", "origin", "case")
    return head


def _bump(fixture: dict[str, Path | str | int], expected: str) -> None:
    seed = fixture["seed"]
    assert isinstance(seed, Path)
    _command(
        (
            "python3",
            "scripts/product_version.py",
            "bump",
            "--expected",
            expected,
        ),
        cwd=seed,
    )


def _remote_head(fixture: dict[str, Path | str | int]) -> str:
    remote = fixture["remote"]
    assert isinstance(remote, Path)
    return _git(remote, "rev-parse", "refs/heads/case")


def _write_readonly_gh(directory: Path) -> Path:
    binary = directory / "gh"
    binary.write_text(
        textwrap.dedent(
            """\
            #!/usr/bin/env python3
            import json
            import os
            import sys

            args = sys.argv[1:]
            log = os.environ.get("MOCK_GH_LOG")
            if log:
                with open(log, "a", encoding="utf-8") as stream:
                    stream.write(json.dumps(args, separators=(",", ":")) + "\\n")
            if not args or args[0] != "api":
                raise SystemExit(3)
            endpoint = next((arg for arg in args if arg.startswith("repos/")), None)
            if endpoint is None:
                raise SystemExit(4)
            with open(os.environ["MOCK_GH_DATABASE"], encoding="utf-8") as stream:
                responses = json.load(stream)["responses"]
            if endpoint not in responses:
                print(f"unexpected gh endpoint: {endpoint}", file=sys.stderr)
                raise SystemExit(5)
            response = responses[endpoint]
            if isinstance(response, dict) and "__returncode__" in response:
                raise SystemExit(int(response["__returncode__"]))
            json.dump(response, sys.stdout, separators=(",", ":"))
            sys.stdout.write("\\n")
            """
        ),
        encoding="utf-8",
    )
    binary.chmod(0o755)
    return binary


def _current_observer_tests(version_workflow: str) -> int:
    script = _step_script(version_workflow, "Read the current pull request once")
    head = "a" * 40
    stale = "b" * 40
    cases = (
        ("open-owner", "open", False, head, False, "false", "owner"),
        ("closed-observer", "closed", False, head, False, "true", "closed-pr"),
        ("event-draft", "open", False, head, True, "true", "event-draft"),
        ("current-draft", "open", True, head, False, "true", "current-draft"),
        ("stale-event", "open", False, stale, False, "true", "stale-event-head"),
    )
    with tempfile.TemporaryDirectory(prefix="codex-info-current-observer-") as raw_root:
        root = Path(raw_root)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        _write_readonly_gh(bin_dir)
        for (
            name,
            state,
            current_draft,
            current_head,
            event_draft,
            expected_observer,
            expected_reason,
        ) in cases:
            case_root = root / name
            case_root.mkdir()
            output = case_root / "output"
            output.write_text("", encoding="utf-8")
            database = case_root / "database.json"
            database.write_text(
                json.dumps(
                    {
                        "responses": {
                            "repos/example/project/pulls/44": {
                                "number": 44,
                                "state": state,
                                "draft": current_draft,
                                "base": {"repo": {"full_name": "example/project"}},
                                "head": {"sha": current_head},
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            environment = os.environ.copy()
            environment.update(
                {
                    "EVENT_DRAFT": str(event_draft).lower(),
                    "EVENT_HEAD_SHA": head,
                    "GH_TOKEN": "fixture",
                    "GITHUB_OUTPUT": str(output),
                    "MOCK_GH_DATABASE": str(database),
                    "PATH": f"{bin_dir}:{environment['PATH']}",
                    "PR_NUMBER": "44",
                    "REPOSITORY": "example/project",
                }
            )
            result = _command(
                ("bash", "-c", script), cwd=case_root, env=environment, check=False
            )
            values = _output(output)
            if result.returncode != 0 or values != {
                "observer": expected_observer,
                "reason": expected_reason,
            }:
                raise AssertionError(f"current observer case {name} is wrong: {values}")
    return len(cases)


def _run_version_step(
    fixture: dict[str, Path | str | int],
    script: str,
    head: str,
    *,
    event_action: str = "opened",
    producer_run: dict[str, object] | None = None,
    run_attempt: int = 7,
    run_id: int = 12345,
) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
    remote = fixture["remote"]
    base = fixture["base"]
    counter = fixture["counter"]
    assert isinstance(remote, Path) and isinstance(base, str) and isinstance(counter, int)
    counter += 1
    fixture["counter"] = counter
    runner = remote.parent / f"runner-{counter}"
    _command(("git", "clone", "--quiet", str(remote), str(runner)), cwd=remote.parent)
    _git(runner, "checkout", "--quiet", "--detach", base)
    output = runner / "github-output"
    output.write_text("", encoding="utf-8")
    runner_temp = runner / "runner-temp"
    runner_temp.mkdir()
    bin_dir = runner_temp / "bin"
    bin_dir.mkdir()
    _write_readonly_gh(bin_dir)
    database = runner_temp / "gh-database.json"
    responses: dict[str, object] = {}
    if producer_run is not None:
        producer_id = producer_run["id"]
        producer_attempt = producer_run["run_attempt"]
        responses[
            f"repos/example/project/actions/runs/{producer_id}/attempts/{producer_attempt}"
        ] = producer_run
    database.write_text(
        json.dumps({"responses": responses}),
        encoding="utf-8",
    )
    environment = os.environ.copy()
    environment.update(
        {
            "BASE_SHA": base,
            "EVENT_ACTION": event_action,
            "GH_TOKEN": "fixture",
            "GITHUB_OUTPUT": str(output),
            "GITHUB_RUN_ATTEMPT": str(run_attempt),
            "GITHUB_RUN_ID": str(run_id),
            "HEAD_REF": "case",
            "HEAD_REPOSITORY": "example/project",
            "HEAD_SHA": head,
            "MOCK_GH_DATABASE": str(database),
            "PATH": f"{bin_dir}:{environment['PATH']}",
            "PR_NUMBER": "44",
            "REPOSITORY": "example/project",
            "RUNNER_TEMP": str(runner_temp),
        }
    )
    result = _command(
        ("bash", "-c", script), cwd=runner, env=environment, check=False
    )
    return result, _output(output)


def _version_state_tests(version_workflow: str) -> int:
    script = _step_script(version_workflow, "Select owners and prepare the binary version")
    cases = 0
    with tempfile.TemporaryDirectory(prefix="codex-info-version-workflow-") as raw_root:
        root = Path(raw_root)

        fixture = _new_version_fixture(root, "docs")
        seed = fixture["seed"]
        assert isinstance(seed, Path)
        (seed / "README.md").write_text("docs\n", encoding="utf-8")
        head = _commit(fixture, "docs")
        result, values = _run_version_step(fixture, script, head)
        if result.returncode != 0 or values.get("ready") != "true":
            raise AssertionError("non-binary H0 did not become ready")
        if values.get("quality_sha") != head or values.get("generated_head") != "false":
            raise AssertionError("non-binary H0 changed the quality head")
        if json.loads(values["selection_json"])["owners"] != ["DOCS"]:
            raise AssertionError("non-binary H0 selected unrelated owners")
        cases += 1

        fixture = _new_version_fixture(root, "windows-causal-chain")
        seed = fixture["seed"]
        base_version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(base_version, str)
        (seed / "windows-client/src/App.cs").write_text(
            "windows H0\n", encoding="utf-8"
        )
        h0 = _commit(fixture, "windows H0")

        # Cancellation before the prepare step leaves the event H0 untouched.
        if _remote_head(fixture) != h0:
            raise AssertionError("pre-push cancellation fixture mutated the PR head")
        cases += 1

        first, first_values = _run_version_step(fixture, script, h0)
        h1 = _remote_head(fixture)
        if (
            first.returncode != 0
            or first_values.get("ready") != "true"
            or first_values.get("generated_head") != "true"
            or first_values.get("quality_sha") != h1
            or h1 == h0
            or json.loads(first_values["selection_json"])["owners"] != ["WINDOWS"]
        ):
            raise AssertionError("Windows H0 did not continue on its generated H1")
        next_version = _command(
            (
                "python3",
                "scripts/product_version.py",
                "next",
                "--version",
                base_version,
            ),
            cwd=seed,
        ).stdout.strip().removeprefix("version=")
        expected_message = (
            f"chore: prepare version {next_version}\n\n"
            "Codex-Version-Prepare-Schema: v1\n"
            "Codex-Version-Prepare-PR: 44\n"
            f"Codex-Version-Prepare-Event-Head: {h0}\n"
            "Codex-Version-Prepare-Run-ID: 12345\n"
            "Codex-Version-Prepare-Run-Attempt: 7"
        )
        message = _git(fixture["remote"], "show", "-s", "--format=%B", h1)
        if message != expected_message or _git(fixture["remote"], "rev-parse", f"{h1}^") != h0:
            raise AssertionError("generated H1 did not atomically retain its H0 run/attempt identity")
        cases += 1

        # No final-check side effect is needed to recover the producer after a
        # post-push cancellation: the immutable H1 trailer is the authority.
        producer_run = {
            "id": 12345,
            "run_number": 90,
            "run_attempt": 7,
            "path": ".github/workflows/version-prepare.yml@refs/heads/main",
            "event": "pull_request_target",
            "repository": {"full_name": "example/project"},
            "display_title": (
                f"codex-main-quality-v1:pr=44:event_head={h0}:"
                "action=opened:draft=false"
            ),
            "status": "completed",
            "conclusion": "success",
        }
        second, second_values = _run_version_step(
            fixture,
            script,
            h1,
            event_action="synchronize",
            producer_run=producer_run,
        )
        if (
            second.returncode != 0
            or second_values.get("ready") != "false"
            or second_values.get("generated_head") != "false"
            or second_values.get("generated_observer") != "true"
            or second_values.get("quality_sha") != h1
            or json.loads(second_values["selection_json"])["owners"] != ["WINDOWS"]
        ):
            raise AssertionError("generated H1 was not a zero-owner observer")
        cases += 1

        _git(seed, "pull", "--quiet", "--ff-only", "origin", "case")
        (seed / "windows-client/src/Later.cs").write_text(
            "class Later {}\n", encoding="utf-8"
        )
        h2 = _commit(fixture, "later Windows change")
        third, third_values = _run_version_step(
            fixture, script, h2, event_action="synchronize"
        )
        if (
            third.returncode != 0
            or third_values.get("quality_sha") != h2
            or third_values.get("generated_head") != "false"
            or third_values.get("generated_observer") != "false"
            or json.loads(third_values["selection_json"])["owners"] != ["WINDOWS"]
        ):
            raise AssertionError("H2 retained generated version files or became an observer")
        cases += 1

        fixture = _new_version_fixture(root, "linux")
        seed = fixture["seed"]
        assert isinstance(seed, Path)
        (seed / "src/lib.rs").write_text("pub fn changed() {}\n", encoding="utf-8")
        h0 = _commit(fixture, "linux")
        result, values = _run_version_step(fixture, script, h0)
        if (
            result.returncode != 0
            or values.get("generated_head") != "true"
            or json.loads(values["selection_json"])["owners"]
            != ["LINUX_BACKEND", "WINDOWS"]
        ):
            raise AssertionError(
                "Linux H0 did not add the Windows release owner on H1"
            )
        cases += 1

        fixture = _new_version_fixture(root, "manual-next")
        seed = fixture["seed"]
        version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(version, str)
        (seed / "windows-client/src/App.cs").write_text("manual next\n", encoding="utf-8")
        parent = _commit(fixture, "manual source")
        _bump(fixture, version)
        next_version = _checked_version(seed)
        invalid_identity = (
            f"chore: prepare version {next_version}\n\n"
            "Codex-Version-Prepare-Schema: v1\n"
            "Codex-Version-Prepare-PR: 45\n"
            f"Codex-Version-Prepare-Event-Head: {parent}\n"
            "Codex-Version-Prepare-Run-ID: 12345\n"
            "Codex-Version-Prepare-Run-Attempt: 7"
        )
        head = _commit(fixture, invalid_identity)
        result, values = _run_version_step(
            fixture, script, head, event_action="synchronize"
        )
        if result.returncode != 0 or values.get("ready") != "true":
            raise AssertionError("invalid canonical identity did not fall back to manual owner")
        if values.get("quality_sha") != head or values.get("generated_head") != "false":
            raise AssertionError("manual exact-next head was treated as generated")
        cases += 1

        fixture = _new_version_fixture(root, "schema-like-manual-next")
        seed = fixture["seed"]
        version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(version, str)
        (seed / "windows-client/src/App.cs").write_text(
            "schema-like manual next\n", encoding="utf-8"
        )
        parent = _commit(fixture, "schema-like manual source")
        _bump(fixture, version)
        next_version = _checked_version(seed)
        schema_like_message = (
            f"chore: prepare version {next_version}\n\n"
            "Codex-Version-Prepare-Schema: v1\n"
            "Codex-Version-Prepare-PR: 44\n"
            f"Codex-Version-Prepare-Event-Head: {parent}\n"
            "Codex-Version-Prepare-Run-ID: 12345\n"
            "Codex-Version-Prepare-Run-Attempt: 7\n"
            "Unexpected-Text: manual"
        )
        head = _commit(fixture, schema_like_message)
        result, values = _run_version_step(
            fixture, script, head, event_action="synchronize"
        )
        if (
            result.returncode != 0
            or values.get("ready") != "true"
            or values.get("generated_observer") != "false"
            or json.loads(values["selection_json"])["owners"] != ["WINDOWS"]
        ):
            raise AssertionError("non-exact schema-like message became a generated observer")
        cases += 1

        fixture = _new_version_fixture(root, "nonbinary-version")
        seed = fixture["seed"]
        version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(version, str)
        (seed / "README.md").write_text("docs then version\n", encoding="utf-8")
        _commit(fixture, "docs")
        _bump(fixture, version)
        head = _commit(fixture, "version only")
        result, _ = _run_version_step(fixture, script, head)
        if result.returncode == 0:
            raise AssertionError("non-binary version transition was accepted")
        cases += 1

        fixture = _new_version_fixture(root, "wrong-version")
        seed = fixture["seed"]
        version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(version, str)
        (seed / "windows-client/src/App.cs").write_text("wrong version\n", encoding="utf-8")
        _bump(fixture, version)
        next_version = _checked_version(seed)
        _bump(fixture, next_version)
        head = _commit(fixture, "wrong version")
        result, _ = _run_version_step(fixture, script, head)
        if result.returncode == 0:
            raise AssertionError("non-next product version was accepted")
        cases += 1

        fixture = _new_version_fixture(root, "unknown")
        seed = fixture["seed"]
        assert isinstance(seed, Path)
        (seed / "future").mkdir()
        (seed / "future/unknown.bin").write_bytes(b"unknown")
        head = _commit(fixture, "unknown")
        result, _ = _run_version_step(fixture, script, head)
        if result.returncode == 0:
            raise AssertionError("unknown path was classified")
        cases += 1

        fixture = _new_version_fixture(root, "empty")
        seed = fixture["seed"]
        remote = fixture["remote"]
        base = fixture["base"]
        assert isinstance(seed, Path) and isinstance(remote, Path) and isinstance(base, str)
        _git(seed, "push", "--quiet", "origin", "main:case")
        result, _ = _run_version_step(fixture, script, base)
        if result.returncode == 0:
            raise AssertionError("empty diff was classified")
        cases += 1

        fixture = _new_version_fixture(root, "head-race")
        seed = fixture["seed"]
        assert isinstance(seed, Path)
        (seed / "windows-client/src/App.cs").write_text("event head\n", encoding="utf-8")
        event_head = _commit(fixture, "event head")
        (seed / "README.md").write_text("concurrent head\n", encoding="utf-8")
        _commit(fixture, "concurrent head")
        concurrent_head = _remote_head(fixture)
        result, _ = _run_version_step(fixture, script, event_head)
        if result.returncode == 0 or _remote_head(fixture) != concurrent_head:
            raise AssertionError("non-fast-forward version push did not fail atomically")
        cases += 1
    return cases


def _acceptance_result_tests(version_workflow: str) -> int:
    """Exercise only the reachable Release acceptance result paths."""

    script = _step_script(version_workflow, "Resolve release quality result")
    cases = 0

    def execute(
        *,
        generated_observer: bool = False,
        event_observer: bool = False,
        prepare: str = "success",
        quality: str = "success",
    ) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment.update(
            {
                "EVENT_OBSERVER": str(event_observer).lower(),
                "GENERATED_OBSERVER": str(generated_observer).lower(),
                "PREPARE_RESULT": prepare,
                "QUALITY_RESULT": quality,
            }
        )
        return _command(
            ("bash", "-c", script), cwd=ROOT, env=environment, check=False
        )

    if execute().returncode != 0:
        raise AssertionError("successful Release quality was rejected")
    cases += 1

    if execute(quality="failure").returncode == 0:
        raise AssertionError("failed Release quality was accepted")
    cases += 1

    if execute(quality="cancelled").returncode == 0:
        raise AssertionError("cancelled Release quality was accepted")
    cases += 1

    if execute(generated_observer=True, quality="skipped").returncode != 0:
        raise AssertionError("generated-H1 observer was rejected")
    cases += 1

    if execute(event_observer=True, quality="skipped").returncode != 0:
        raise AssertionError("non-authoritative event observer was rejected")
    cases += 1

    if execute(prepare="failure", quality="skipped").returncode == 0:
        raise AssertionError("failed version preparation was accepted")
    cases += 1

    return cases


_REPOSITORY = "example/project"
_PR_NUMBER = 44
_FINAL_HEAD = "a" * 40
_MERGE_SHA = "b" * 40
_VERSION = "1.2.3"
_ARTIFACT_DIGEST = "sha256:" + "c" * 64
_HEAD_REF = "issue-44-order-independent-release"


def _quality_title(
    head: str, *, action: str = "opened", draft: bool = False
) -> str:
    return (
        f"codex-main-quality-v1:pr={_PR_NUMBER}:event_head={head}:"
        f"action={action}:draft={str(draft).lower()}"
    )


def _quality_run(
    run_id: int,
    run_number: int,
    run_attempt: int,
    *,
    head: str = _FINAL_HEAD,
    action: str = "opened",
    status: str = "completed",
    conclusion: str | None = "success",
) -> dict[str, object]:
    return {
        "id": run_id,
        "run_number": run_number,
        "run_attempt": run_attempt,
        "path": ".github/workflows/version-prepare.yml@refs/heads/main",
        "event": "pull_request_target",
        "head_sha": head,
        "repository": {"full_name": _REPOSITORY},
        "display_title": _quality_title(head, action=action),
        "status": status,
        "conclusion": conclusion,
    }


def _pull_request(*, merged: bool) -> dict[str, object]:
    return {
        "number": _PR_NUMBER,
        "state": "closed" if merged else "open",
        "merged": merged,
        "draft": False,
        "merged_at": "2026-08-31T00:00:00Z" if merged else None,
        "merge_commit_sha": _MERGE_SHA if merged else None,
        "base": {"ref": "main", "repo": {"full_name": _REPOSITORY}},
        "head": {
            "ref": _HEAD_REF,
            "sha": _FINAL_HEAD,
            "repo": {"full_name": _REPOSITORY},
        },
    }


def _commit_object(
    *,
    head: str = _FINAL_HEAD,
    message: str = "manual final head",
    parent: str = "d" * 40,
) -> dict[str, object]:
    return {
        "sha": head,
        "commit": {"message": message},
        "parents": [{"sha": parent}],
    }


def _invalid_generated_message(parent: str) -> str:
    return (
        f"chore: prepare version {_VERSION}\n\n"
        "Codex-Version-Prepare-Schema: v1\n"
        "Codex-Version-Prepare-PR: 45\n"
        f"Codex-Version-Prepare-Event-Head: {parent}\n"
        "Codex-Version-Prepare-Run-ID: 999\n"
        "Codex-Version-Prepare-Run-Attempt: 1"
    )


def _release_candidate(
    run_id: int,
    attempt: int,
    *,
    artifact_id: int | None = None,
    expired: bool = False,
    head: str = _FINAL_HEAD,
    malformed: bool = False,
    platform: str = "windows",
) -> dict[str, object]:
    prefix = "release-candidate-v1-"
    if platform == "linux":
        prefix = "release-candidate-linux-v1-"
    name = (
        f"{prefix}malformed"
        if malformed
        else (
            f"{prefix}pr-{_PR_NUMBER}-head-{head}-run-{run_id}-"
            f"attempt-{attempt}-version-{_VERSION}"
        )
    )
    return {
        "id": artifact_id if artifact_id is not None else run_id * 100 + attempt,
        "name": name,
        "digest": _ARTIFACT_DIGEST,
        "expired": expired,
    }


def _candidate_set(
    run_id: int, attempt: int, mode: str, *, platform: str = "windows"
) -> list[dict[str, object]]:
    if mode == "missing":
        return []
    if mode == "exact":
        return [_release_candidate(run_id, attempt, platform=platform)]
    if mode == "expired":
        return [_release_candidate(run_id, attempt, expired=True, platform=platform)]
    if mode == "multiple":
        return [
            _release_candidate(run_id, attempt, artifact_id=run_id * 100 + attempt, platform=platform),
            _release_candidate(run_id, attempt, artifact_id=run_id * 100 + attempt + 50, platform=platform),
        ]
    if mode == "malformed":
        return [_release_candidate(run_id, attempt, malformed=True, platform=platform)]
    raise AssertionError(f"unknown candidate fixture mode: {mode}")


def _object_pages(
    member: str, items: Sequence[Mapping[str, object]], *, paginated: bool = False
) -> list[dict[str, object]]:
    values = [dict(item) for item in items]
    total = len(values)
    if paginated:
        return [
            {member: [], "total_count": total},
            {member: values, "total_count": total},
        ]
    return [{member: values, "total_count": total}]


def _runs_endpoint() -> str:
    return (
        f"repos/{_REPOSITORY}/actions/workflows/version-prepare.yml/"
        f"runs?event=pull_request_target&branch={_HEAD_REF}&per_page=100"
    )


def _manual_release_responses(
    run_specs: Sequence[Mapping[str, object]],
    *,
    merged: bool = True,
    paginated: bool = False,
) -> tuple[dict[str, object], list[dict[str, object]]]:
    responses: dict[str, object] = {
        f"repos/{_REPOSITORY}/pulls/{_PR_NUMBER}": _pull_request(merged=merged),
    }
    summaries: list[dict[str, object]] = []
    for spec in run_specs:
        run_id = int(spec["id"])
        run_number = int(spec["number"])
        action = str(spec.get("action", "opened"))
        attempts = list(spec["attempts"])
        if not attempts:
            raise AssertionError("release run fixture must contain at least one attempt")
        latest = len(attempts)
        latest_row = attempts[-1]
        summary = _quality_run(
            run_id,
            run_number,
            latest,
            action=action,
            status=str(latest_row["status"]),
            conclusion=latest_row.get("conclusion"),
        )
        summaries.append(summary)
        artifacts: list[dict[str, object]] = []
        for attempt, row in enumerate(attempts, start=1):
            attempt_run = _quality_run(
                run_id,
                run_number,
                attempt,
                action=action,
                status=str(row["status"]),
                conclusion=row.get("conclusion"),
            )
            responses[
                f"repos/{_REPOSITORY}/actions/runs/{run_id}/attempts/{attempt}"
            ] = attempt_run
            windows = row.get("windows")
            jobs: list[dict[str, object]] = []
            if windows == "success":
                jobs.append(
                    {
                        "name": "Run selected quality owners / windows-quality / windows-quality",
                        "status": "completed",
                        "conclusion": "success",
                    }
                )
            elif windows == "skipped":
                jobs.append(
                    {
                        "name": "Run selected quality owners / windows-quality",
                        "status": "completed",
                        "conclusion": "skipped",
                    }
                )
            elif windows == "observer":
                jobs.append(
                    {
                        "name": "Observe non-authoritative PR event",
                        "status": "completed",
                        "conclusion": "success",
                    }
                )
            elif windows is not None:
                raise AssertionError(f"unknown Windows fixture result: {windows}")
            linux = row.get("linux")
            if linux == "success":
                jobs.append(
                    {
                        "name": "Run selected quality owners / linux-distribution / linux-distribution",
                        "status": "completed",
                        "conclusion": "success",
                    }
                )
            elif linux == "skipped":
                jobs.append(
                    {
                        "name": "Run selected quality owners / linux-distribution",
                        "status": "completed",
                        "conclusion": "skipped",
                    }
                )
            elif linux == "observer":
                # The draft observer is represented by the shared observer job;
                # no product authority job is added for it.
                pass
            elif linux is not None:
                raise AssertionError(f"unknown Linux fixture result: {linux}")
            product_quality = row.get("product_quality")
            if product_quality == "linux-backend":
                jobs.append(
                    {
                        "name": "Run selected quality owners / linux-backend-quality / native-quality",
                        "status": "completed",
                        "conclusion": "success",
                    }
                )
            elif product_quality == "linux-ui":
                jobs.append(
                    {
                        "name": "Run selected quality owners / linux-ui-quality / linux-ui-quality",
                        "status": "completed",
                        "conclusion": "success",
                    }
                )
            elif product_quality is not None:
                raise AssertionError(f"unknown product quality fixture: {product_quality}")
            responses[
                f"repos/{_REPOSITORY}/actions/runs/{run_id}/attempts/{attempt}/jobs?filter=all&per_page=100"
            ] = _object_pages("jobs", jobs, paginated=paginated)
            mode = row.get("candidate")
            if mode is not None:
                artifacts.extend(_candidate_set(run_id, attempt, str(mode)))
            linux_mode = row.get("linux_candidate")
            if linux_mode is not None:
                artifacts.extend(
                    _candidate_set(run_id, attempt, str(linux_mode), platform="linux")
                )
        responses[
            f"repos/{_REPOSITORY}/actions/runs/{run_id}/artifacts?per_page=100"
        ] = _object_pages("artifacts", artifacts, paginated=paginated)
    responses[f"repos/{_REPOSITORY}/commits/{_FINAL_HEAD}"] = _commit_object()
    responses[_runs_endpoint()] = _object_pages(
        "workflow_runs", summaries, paginated=paginated
    )
    return responses, summaries


def _closed_event(*, merged: bool = True) -> dict[str, object]:
    return {
        "action": "closed",
        "pull_request": {
            "number": _PR_NUMBER,
            "base": {"ref": "main"},
            "merged": merged,
        },
    }


def _workflow_event(run: Mapping[str, object]) -> dict[str, object]:
    return {"workflow_run": dict(run)}


def _execute_release_shell(
    script: str,
    responses: Mapping[str, object],
    *,
    event_name: str,
    event: Mapping[str, object],
) -> tuple[subprocess.CompletedProcess[str], dict[str, str], list[list[str]]]:
    with tempfile.TemporaryDirectory(prefix="codex-info-release-shell-") as raw_root:
        root = Path(raw_root)
        bin_dir = root / "bin"
        runner_temp = root / "runner-temp"
        bin_dir.mkdir()
        runner_temp.mkdir()
        _write_readonly_gh(bin_dir)
        database = root / "database.json"
        database.write_text(
            json.dumps({"responses": dict(responses)}), encoding="utf-8"
        )
        event_path = root / "event.json"
        event_path.write_text(json.dumps(event), encoding="utf-8")
        output = root / "output"
        output.write_text("", encoding="utf-8")
        log = root / "gh.jsonl"
        log.write_text("", encoding="utf-8")
        environment = os.environ.copy()
        environment.update(
            {
                "EVENT_NAME": event_name,
                "GH_TOKEN": "fixture",
                "GITHUB_EVENT_PATH": str(event_path),
                "GITHUB_OUTPUT": str(output),
                "MOCK_GH_DATABASE": str(database),
                "MOCK_GH_LOG": str(log),
                "PATH": f"{bin_dir}:{environment['PATH']}",
                "REPOSITORY": _REPOSITORY,
                "RUNNER_TEMP": str(runner_temp),
            }
        )
        result = _command(
            ("bash", "-c", script), cwd=root, env=environment, check=False
        )
        calls = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
        return result, _output(output), calls


def _generated_release_responses() -> tuple[dict[str, object], dict[str, object]]:
    generator_head = "e" * 40
    generator = _quality_run(301, 30, 7, head=generator_head, action="opened")
    observer = _quality_run(302, 31, 1, head=_FINAL_HEAD, action="synchronize")
    message = (
        f"chore: prepare version {_VERSION}\n\n"
        "Codex-Version-Prepare-Schema: v1\n"
        f"Codex-Version-Prepare-PR: {_PR_NUMBER}\n"
        f"Codex-Version-Prepare-Event-Head: {generator_head}\n"
        "Codex-Version-Prepare-Run-ID: 301\n"
        "Codex-Version-Prepare-Run-Attempt: 7"
    )
    candidate = _release_candidate(301, 7)
    responses: dict[str, object] = {
        f"repos/{_REPOSITORY}/pulls/{_PR_NUMBER}": _pull_request(merged=True),
        f"repos/{_REPOSITORY}/commits/{_FINAL_HEAD}": _commit_object(
            message=message, parent=generator_head
        ),
        _runs_endpoint(): _object_pages("workflow_runs", [generator, observer]),
        f"repos/{_REPOSITORY}/actions/runs/301": generator,
        f"repos/{_REPOSITORY}/actions/runs/301/attempts/7": generator,
        f"repos/{_REPOSITORY}/actions/runs/301/attempts/7/jobs?filter=all&per_page=100": _object_pages(
            "jobs",
            [
                {
                    "name": "Run selected quality owners / windows-quality / windows-quality",
                    "status": "completed",
                    "conclusion": "success",
                },
                {
                    "name": "Run selected quality owners / linux-distribution / linux-distribution",
                    "status": "completed",
                    "conclusion": "success",
                }
            ],
        ),
        f"repos/{_REPOSITORY}/actions/runs/301/artifacts?per_page=100": _object_pages(
            "artifacts", [candidate, _release_candidate(301, 7, platform="linux")]
        ),
        f"repos/{_REPOSITORY}/actions/runs/302/artifacts?per_page=100": _object_pages(
            "artifacts", []
        ),
    }
    return responses, generator


def _release_resolution_tests(release_workflow: str) -> int:
    script = _step_script(release_workflow, "Resolve the immutable publication snapshot")
    cases = 0

    successful_spec = [
        {
            "id": 101,
            "number": 11,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "success",
                    "candidate": "exact",
                    "linux": "success",
                    "linux_candidate": "exact",
                }
            ],
        }
    ]

    # Quality-first: the early completed signal is a green no-op until merge,
    # then the closed signal converges on the exact same immutable snapshot.
    responses, summaries = _manual_release_responses(successful_spec, merged=False)
    result, values, _ = _execute_release_shell(
        script,
        responses,
        event_name="workflow_run",
        event=_workflow_event(summaries[0]),
    )
    if result.returncode != 0 or values.get("publish") != "false":
        raise AssertionError("quality-first signal did not wait for merge")
    cases += 1
    responses, _ = _manual_release_responses(successful_spec, merged=True)
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode != 0 or values.get("publish") != "true":
        raise AssertionError("quality-first closed signal did not publish")
    cases += 1

    # Merge-first: closed observes a pending attempt without becoming red; the
    # later successful workflow signal performs the same release resolution.
    pending_spec = [
        {
            "id": 102,
            "number": 12,
            "attempts": [{"status": "in_progress", "conclusion": None}],
        }
    ]
    responses, _ = _manual_release_responses(pending_spec, merged=True)
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode != 0 or values.get("publish") != "false":
        raise AssertionError("merge-first closed signal did not hold pending quality")
    cases += 1

    no_runs, _ = _manual_release_responses([], merged=True)
    result, values, _ = _execute_release_shell(
        script, no_runs, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode != 0 or values.get("publish") != "false":
        raise AssertionError("closed signal with no visible run was not a green HOLD")
    cases += 1

    responses, summaries = _manual_release_responses(successful_spec, merged=True)
    responses[_runs_endpoint()] = _object_pages("workflow_runs", [])
    result, values, _ = _execute_release_shell(
        script,
        responses,
        event_name="workflow_run",
        event=_workflow_event(summaries[0]),
    )
    if result.returncode != 0 or values.get("publish") != "true":
        raise AssertionError("merge-first completion did not publish")
    cases += 1

    nonsuccess_signal = _quality_run(
        103, 13, 1, status="completed", conclusion="failure"
    )
    result, values, calls = _execute_release_shell(
        script,
        {},
        event_name="workflow_run",
        event=_workflow_event(nonsuccess_signal),
    )
    if result.returncode != 0 or values.get("publish") != "false" or calls:
        raise AssertionError("non-success workflow signal was not a green mutation-free no-op")
    cases += 1

    barriers: tuple[tuple[str, Sequence[Mapping[str, object]]], ...] = (
        (
            "success-then-failure",
            [
                {
                    "id": 110,
                    "number": 20,
                    "attempts": [
                        {"status": "completed", "conclusion": "success"},
                        {"status": "completed", "conclusion": "failure"},
                    ],
                }
            ],
        ),
        (
            "failure-then-success",
            [
                {
                    "id": 111,
                    "number": 21,
                    "attempts": [
                        {"status": "completed", "conclusion": "failure"},
                        {"status": "completed", "conclusion": "success"},
                    ],
                }
            ],
        ),
        (
            "failure-then-reopen-success",
            [
                {
                    "id": 112,
                    "number": 22,
                    "action": "opened",
                    "attempts": [{"status": "completed", "conclusion": "failure"}],
                },
                {
                    "id": 113,
                    "number": 23,
                    "action": "reopened",
                    "attempts": [{"status": "completed", "conclusion": "success"}],
                },
            ],
        ),
    )
    for name, specs in barriers:
        responses, _ = _manual_release_responses(specs)
        result, values, _ = _execute_release_shell(
            script, responses, event_name="pull_request_target", event=_closed_event()
        )
        if result.returncode != 0 or values.get("publish") != "false":
            raise AssertionError(f"all-attempt failure barrier failed: {name}")
        cases += 1

    skipped_spec = [
        {
            "id": 120,
            "number": 30,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "skipped",
                    "candidate": "missing",
                }
            ],
        }
    ]
    responses, _ = _manual_release_responses(skipped_spec)
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode != 0 or values.get("publish") != "false":
        raise AssertionError("skipped Windows authority with zero candidates was not a no-op")
    cases += 1

    linux_only_spec = [
        {
            "id": 122,
            "number": 32,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "skipped",
                    "linux": "success",
                    "linux_candidate": "exact",
                }
            ],
        }
    ]
    responses, _ = _manual_release_responses(linux_only_spec)
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode == 0:
        raise AssertionError(
            "Linux-only distribution candidate bypassed the Windows release authority"
        )
    print("workflow-quality-gate: case=linux-only-without-windows PASS")
    cases += 1

    co_located_spec = [
        {
            "id": 125,
            "number": 35,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "success",
                    "candidate": "exact",
                    "linux": "success",
                    "linux_candidate": "exact",
                }
            ],
        }
    ]
    responses, _ = _manual_release_responses(co_located_spec)
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if (
        result.returncode != 0
        or values.get("publish") != "true"
        or values.get("linux_present") != "true"
        or values.get("windows_present") != "true"
        or not values.get("artifact_ids")
    ):
        raise AssertionError("co-located Windows/Linux candidates were not selected")
    cases += 1

    partial_linux_spec = [
        {
            "id": 123,
            "number": 33,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "success",
                    "candidate": "exact",
                    "linux": "skipped",
                    "linux_candidate": "missing",
                }
            ],
        }
    ]
    responses, _ = _manual_release_responses(partial_linux_spec)
    result, _, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode == 0:
        raise AssertionError("Windows-only candidate bypassed a skipped Linux authority")
    cases += 1

    missing_linux_authority_spec = [
        {
            "id": 124,
            "number": 34,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "skipped",
                    "product_quality": "linux-backend",
                }
            ],
        }
    ]
    responses, _ = _manual_release_responses(missing_linux_authority_spec)
    result, _, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode == 0:
        raise AssertionError(
            "product-impact run without a Linux authority job was accepted"
        )
    print("workflow-quality-gate: case=missing-linux-authority PASS")
    cases += 1

    draft_observer_spec = [
        {
            "id": 121,
            "number": 31,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "observer",
                    "candidate": "missing",
                }
            ],
        }
    ]
    responses, _ = _manual_release_responses(draft_observer_spec)
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode != 0 or values.get("publish") != "false":
        raise AssertionError("current-draft successful observer was treated as Windows authority")
    cases += 1

    # Empty first pages prove that runs, jobs, and artifacts are all flattened
    # across pagination before the exact-one decision.
    responses, _ = _manual_release_responses(successful_spec, paginated=True)
    invalid_parent = "d" * 40
    responses[f"repos/{_REPOSITORY}/commits/{_FINAL_HEAD}"] = _commit_object(
        message=_invalid_generated_message(invalid_parent),
        parent=invalid_parent,
    )
    result, values, _ = _execute_release_shell(
        script, responses, event_name="pull_request_target", event=_closed_event()
    )
    if (
        result.returncode != 0
        or values.get("publish") != "true"
        or values.get("run_id") != "101"
        or values.get("run_attempt") != "1"
        or values.get("artifact_name")
        != _release_candidate(101, 1)["name"]
    ):
        raise AssertionError("paginated exact-one Windows candidate was not selected")
    cases += 1

    incomplete = json.loads(json.dumps(responses))
    for page in incomplete[_runs_endpoint()]:
        page["total_count"] = 2
    result, _, _ = _execute_release_shell(
        script, incomplete, event_name="pull_request_target", event=_closed_event()
    )
    if result.returncode == 0:
        raise AssertionError("incomplete paginated run history was accepted")
    cases += 1

    for mode in ("missing", "expired", "multiple", "malformed"):
        bad_spec = [
            {
                "id": 130,
                "number": 40,
                "attempts": [
                    {
                        "status": "completed",
                        "conclusion": "success",
                        "windows": "success",
                        "candidate": mode,
                        "linux": "success",
                        "linux_candidate": "exact",
                    }
                ],
            }
        ]
        responses, _ = _manual_release_responses(bad_spec)
        result, _, _ = _execute_release_shell(
            script, responses, event_name="pull_request_target", event=_closed_event()
        )
        if result.returncode == 0:
            raise AssertionError(f"invalid Windows candidate was accepted: {mode}")
        cases += 1

    for mode in ("missing", "expired", "multiple", "malformed"):
        bad_linux_spec = [
            {
                "id": 131,
                "number": 41,
                "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "success",
                    "candidate": "exact",
                    "linux": "success",
                    "linux_candidate": mode,
                }
                ],
            }
        ]
        responses, _ = _manual_release_responses(bad_linux_spec)
        result, _, _ = _execute_release_shell(
            script, responses, event_name="pull_request_target", event=_closed_event()
        )
        if result.returncode == 0:
            raise AssertionError(f"invalid Linux candidate was accepted: {mode}")
        cases += 1

    responses, generator = _generated_release_responses()
    result, values, _ = _execute_release_shell(
        script,
        responses,
        event_name="workflow_run",
        event=_workflow_event(generator),
    )
    if (
        result.returncode != 0
        or values.get("publish") != "true"
        or values.get("run_id") != "301"
        or values.get("run_attempt") != "7"
    ):
        raise AssertionError("generated H1 observer replaced or duplicated its H0 producer")
    cases += 1
    return cases


def _execute_revalidation(
    script: str,
    responses: Mapping[str, object],
    authority: Mapping[str, str],
    *,
    event_name: str,
    event: Mapping[str, object],
) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
    with tempfile.TemporaryDirectory(prefix="codex-info-release-lock-") as raw_root:
        root = Path(raw_root)
        bin_dir = root / "bin"
        runner_temp = root / "runner-temp"
        bin_dir.mkdir()
        runner_temp.mkdir()
        _write_readonly_gh(bin_dir)
        database = root / "database.json"
        database.write_text(
            json.dumps({"responses": dict(responses)}), encoding="utf-8"
        )
        event_path = root / "event.json"
        event_path.write_text(json.dumps(event), encoding="utf-8")
        output = root / "output"
        output.write_text("", encoding="utf-8")
        environment = os.environ.copy()
        environment.update(
            {
                "ARTIFACT_DIGEST": authority["artifact_digest"],
                "ARTIFACT_ID": authority["artifact_id"],
                "ARTIFACT_IDS": authority.get("artifact_ids", authority["artifact_id"]),
                "ARTIFACT_NAME": authority["artifact_name"],
                "EVENT_NAME": event_name,
                "FINAL_HEAD": authority["final_head"],
                "FINGERPRINT": authority["fingerprint"],
                "GH_TOKEN": "fixture",
                "GITHUB_EVENT_PATH": str(event_path),
                "GITHUB_OUTPUT": str(output),
                "MERGE_SHA": authority["merge_sha"],
                "MOCK_GH_DATABASE": str(database),
                "PATH": f"{bin_dir}:{environment['PATH']}",
                "PR_NUMBER": authority["pr_number"],
                "REPOSITORY": _REPOSITORY,
                "RUN_ATTEMPT": authority["run_attempt"],
                "RUN_ID": authority["run_id"],
                "RUN_NUMBER": authority["run_number"],
                "RUNNER_TEMP": str(runner_temp),
                "LINUX_PRESENT": authority.get("linux_present", "false"),
                "WINDOWS_PRESENT": authority.get("windows_present", "true"),
                "VERSION": authority["version"],
            }
        )
        result = _command(
            ("bash", "-c", script), cwd=root, env=environment, check=False
        )
        return result, _output(output)


def _release_lock_tests(release_workflow: str) -> int:
    resolve_script = _step_script(
        release_workflow, "Resolve the immutable publication snapshot"
    )
    revalidate_script = _step_script(
        release_workflow, "Revalidate authority after acquiring the tag lock"
    )
    spec = [
        {
            "id": 201,
            "number": 51,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "success",
                    "candidate": "exact",
                    "linux": "success",
                    "linux_candidate": "exact",
                }
            ],
        }
    ]
    responses, summaries = _manual_release_responses(spec)
    signal = summaries[0]
    invalid_parent = "d" * 40
    responses[f"repos/{_REPOSITORY}/commits/{_FINAL_HEAD}"] = _commit_object(
        message=_invalid_generated_message(invalid_parent),
        parent=invalid_parent,
    )
    responses[_runs_endpoint()] = _object_pages("workflow_runs", [])
    resolved, authority, _ = _execute_release_shell(
        resolve_script,
        responses,
        event_name="workflow_run",
        event=_workflow_event(signal),
    )
    if resolved.returncode != 0 or authority.get("publish") != "true":
        raise AssertionError("lock fixture did not resolve an initial authority")

    result, values = _execute_revalidation(
        revalidate_script,
        responses,
        authority,
        event_name="workflow_run",
        event=_workflow_event(signal),
    )
    if result.returncode != 0 or values.get("proceed") != "true":
        raise AssertionError("unchanged authority failed after acquiring the tag lock")
    cases = 1

    co_located_spec = [
        {
            "id": 203,
            "number": 53,
            "attempts": [
                {
                    "status": "completed",
                    "conclusion": "success",
                    "windows": "success",
                    "candidate": "exact",
                    "linux": "success",
                    "linux_candidate": "exact",
                }
            ],
        }
    ]
    linux_responses, linux_summaries = _manual_release_responses(co_located_spec)
    linux_responses[_runs_endpoint()] = _object_pages("workflow_runs", [])
    linux_resolved, linux_authority, _ = _execute_release_shell(
        resolve_script,
        linux_responses,
        event_name="workflow_run",
        event=_workflow_event(linux_summaries[0]),
    )
    if linux_resolved.returncode != 0 or linux_authority.get("publish") != "true":
        raise AssertionError("co-located authority did not resolve before lock revalidation")
    result, values = _execute_revalidation(
        revalidate_script,
        linux_responses,
        linux_authority,
        event_name="workflow_run",
        event=_workflow_event(linux_summaries[0]),
    )
    if result.returncode != 0 or values.get("proceed") != "true":
        raise AssertionError("co-located authority failed after acquiring the tag lock")
    cases += 1

    missing_windows_authority = dict(linux_authority)
    missing_windows_authority["windows_present"] = "false"
    result, values = _execute_revalidation(
        revalidate_script,
        linux_responses,
        missing_windows_authority,
        event_name="workflow_run",
        event=_workflow_event(linux_summaries[0]),
    )
    if result.returncode == 0 or values.get("proceed") == "true":
        raise AssertionError("Linux-only authority passed lock revalidation")
    cases += 1

    changed = json.loads(json.dumps(responses))
    changed[_runs_endpoint()] = _object_pages(
        "workflow_runs", [_quality_run(202, 52, 1, action="reopened")]
    )
    result, values = _execute_revalidation(
        revalidate_script,
        changed,
        authority,
        event_name="workflow_run",
        event=_workflow_event(signal),
    )
    if result.returncode != 0 or values.get("proceed") != "false":
        raise AssertionError("changed latest-run fingerprint passed the tag lock")
    cases += 1
    return cases


def _write_publish_gh(directory: Path) -> Path:
    binary = directory / "gh"
    binary.write_text(
        textwrap.dedent(
            """\
            #!/usr/bin/env python3
            import hashlib
            import json
            import os
            from pathlib import Path
            import re
            import sys

            state_path = Path(os.environ["MOCK_GH_STATE"])
            state = json.loads(state_path.read_text(encoding="utf-8"))
            args = sys.argv[1:]
            payload = None
            if "--input" in args:
                payload = json.load(sys.stdin)
            with open(os.environ["MOCK_GH_LOG"], "a", encoding="utf-8") as stream:
                stream.write(json.dumps({"tool": "gh", "args": args, "input": payload}, separators=(",", ":")) + "\\n")

            def save():
                state_path.write_text(json.dumps(state, separators=(",", ":")), encoding="utf-8")

            def emit(value):
                json.dump(value, sys.stdout, separators=(",", ":"))
                sys.stdout.write("\\n")

            if not args or args[0] != "api":
                raise SystemExit(3)
            endpoint = next((arg for arg in args if arg.startswith("repos/")), None)
            if endpoint is None:
                raise SystemExit(4)
            method = "GET"
            if "--method" in args:
                method = args[args.index("--method") + 1]

            if method == "GET" and "/git/matching-refs/tags/" in endpoint:
                emit([state["tags"]])
                raise SystemExit(0)
            if method == "GET" and endpoint.endswith("/releases?per_page=100"):
                emit([state["releases"]])
                raise SystemExit(0)
            match = re.search(r"/releases/([1-9][0-9]*)/assets\\?per_page=100$", endpoint)
            if method == "GET" and match:
                emit([state["assets"].get(match.group(1), [])])
                raise SystemExit(0)
            match = re.search(r"/releases/([1-9][0-9]*)$", endpoint)
            if method == "GET" and match:
                emit(state["details"][match.group(1)])
                raise SystemExit(0)

            if method == "POST" and endpoint.endswith("/releases"):
                release_id = int(state["next_id"])
                state["next_id"] = release_id + 1
                detail = {
                    "id": release_id,
                    "tag_name": payload["tag_name"],
                    "target_commitish": payload["target_commitish"],
                    "name": payload["name"],
                    "body": payload["body"],
                    "draft": True,
                    "prerelease": False,
                    "published_at": None,
                    "upload_url": (
                        f"https://uploads.github.com/repos/example/project/releases/"
                        f"{release_id}/assets{{?name,label}}"
                    ),
                }
                state["tags"].append(
                    {
                        "ref": "refs/tags/" + payload["tag_name"],
                        "object": {"type": "commit", "sha": payload["target_commitish"]},
                    }
                )
                state["releases"].append(
                    {"id": release_id, "tag_name": payload["tag_name"], "draft": True}
                )
                state["details"][str(release_id)] = detail
                state["assets"][str(release_id)] = []
                save()
                emit(detail)
                raise SystemExit(0)

            if method == "PATCH" and match:
                release_id = match.group(1)
                detail = state["details"][release_id]
                detail["draft"] = bool(payload["draft"])
                detail["published_at"] = None if detail["draft"] else "2026-08-31T00:01:00Z"
                for release in state["releases"]:
                    if str(release["id"]) == release_id:
                        release["draft"] = detail["draft"]
                save()
                response = dict(detail)
                response["assets"] = state["assets"].get(release_id, [])
                emit(response)
                raise SystemExit(0)

            print(f"unexpected gh operation: {args}", file=sys.stderr)
            raise SystemExit(5)
            """
        ),
        encoding="utf-8",
    )
    binary.chmod(0o755)
    return binary


def _write_publish_curl(directory: Path) -> Path:
    binary = directory / "curl"
    binary.write_text(
        textwrap.dedent(
            """\
            #!/usr/bin/env python3
            import hashlib
            import json
            import os
            from pathlib import Path
            import re
            import sys

            args = sys.argv[1:]
            with open(os.environ["MOCK_GH_LOG"], "a", encoding="utf-8") as stream:
                stream.write(json.dumps({"tool": "curl", "args": args}, separators=(",", ":")) + "\\n")
            state_path = Path(os.environ["MOCK_GH_STATE"])
            state = json.loads(state_path.read_text(encoding="utf-8"))

            def option(name):
                return args[args.index(name) + 1]

            if option("--request") != "POST":
                raise SystemExit(3)
            data = option("--data-binary")
            if not data.startswith("@"):
                raise SystemExit(4)
            source = Path(data[1:])
            output = Path(option("--output"))
            url = args[-1]
            match = re.fullmatch(
                r"https://uploads\\.github\\.com/repos/example/project/releases/([1-9][0-9]*)/assets\\?name=([^&]+)",
                url,
            )
            if match is None or not source.is_file():
                raise SystemExit(5)
            release_id = match.group(1)
            if release_id not in state["details"]:
                raise SystemExit(6)
            content = source.read_bytes()
            asset = {
                "id": int(release_id) * 10 + len(state["assets"].get(release_id, [])) + 1,
                "name": match.group(2),
                "state": "uploaded",
                "size": len(content),
                "digest": "sha256:" + hashlib.sha256(content).hexdigest(),
            }
            state["assets"].setdefault(release_id, []).append(asset)
            state_path.write_text(json.dumps(state, separators=(",", ":")), encoding="utf-8")
            output.write_text(json.dumps(asset, separators=(",", ":")), encoding="utf-8")
            sys.stdout.write("201")
            """
        ),
        encoding="utf-8",
    )
    binary.chmod(0o755)
    return binary


def _release_assets(*paths: Path) -> list[dict[str, object]]:
    assets: list[dict[str, object]] = []
    for path in paths:
        content = path.read_bytes()
        assets.append(
            {
                "name": path.name,
                "state": "uploaded",
                "size": len(content),
                "digest": "sha256:" + hashlib.sha256(content).hexdigest(),
            }
        )
    return assets


def _publication_state(kind: str, *asset_paths: Path) -> dict[str, object]:
    state: dict[str, object] = {
        "next_id": 901,
        "tags": [],
        "releases": [],
        "details": {},
        "assets": {},
    }
    if kind == "absent":
        return state
    release_id = 900
    tag = {
        "ref": f"refs/tags/windows-v{_VERSION}",
        "object": {"type": "commit", "sha": _MERGE_SHA},
    }
    detail = {
        "id": release_id,
        "tag_name": f"windows-v{_VERSION}",
        "target_commitish": _MERGE_SHA,
        "name": f"Codex Info Monitor {_VERSION}",
        "body": f"Windows client {_VERSION}",
        "draft": False,
        "prerelease": False,
        "published_at": "2026-08-31T00:00:00Z",
        "upload_url": (
            f"https://uploads.github.com/repos/{_REPOSITORY}/releases/"
            f"{release_id}/assets{{?name,label}}"
        ),
    }
    assets = _release_assets(*asset_paths)
    if kind != "release-only":
        state["tags"] = [tag]
    if kind != "orphan-tag":
        state["releases"] = [
            {"id": release_id, "tag_name": f"windows-v{_VERSION}", "draft": False}
        ]
        state["details"] = {str(release_id): detail}
        state["assets"] = {str(release_id): assets}
    if kind == "draft":
        state["releases"][0]["draft"] = True
        state["details"][str(release_id)]["draft"] = True
        state["details"][str(release_id)]["published_at"] = None
    elif kind == "partial":
        state["assets"][str(release_id)] = assets[:1]
    elif kind == "target-mismatch":
        state["details"][str(release_id)]["target_commitish"] = "f" * 40
    elif kind == "asset-mismatch":
        state["assets"][str(release_id)][0]["digest"] = "sha256:" + "0" * 64
    elif kind not in {"orphan-tag", "release-only", "published"}:
        raise AssertionError(f"unknown publication fixture: {kind}")
    return state


def _publish_mutations(calls: Sequence[Mapping[str, object]]) -> list[Mapping[str, object]]:
    mutations: list[Mapping[str, object]] = []
    for call in calls:
        args = call["args"]
        if call.get("tool") == "curl" or (
            call.get("tool") == "gh"
            and "--method" in args
            and args[args.index("--method") + 1] in {"POST", "PATCH"}
        ):
            mutations.append(call)
    return mutations


def _materialize_candidate_handoff(
    windows_workflow: str,
    release_workflow: str,
    case_root: Path,
) -> tuple[Path, Path]:
    """Model only the v4 upload/download filesystem contract used in production."""

    windows = _workflow_document(windows_workflow)
    upload = _step(
        _job(windows, "windows-quality"), uses="actions/upload-artifact@v4"
    )
    release = _workflow_document(release_workflow)
    download = _step(_job(release, "publish"), uses="actions/download-artifact@v4")
    upload_with = upload.get("with")
    download_with = download.get("with")
    if not isinstance(upload_with, dict) or not isinstance(download_with, dict):
        raise AssertionError("artifact Action inputs are missing")

    raw_paths = upload_with.get("path")
    if not isinstance(raw_paths, str):
        raise AssertionError("upload-artifact path is not a literal list")
    source_paths = [Path(line.strip()) for line in raw_paths.splitlines() if line.strip()]
    if not source_paths:
        raise AssertionError("upload-artifact has no payload paths")
    for path in source_paths:
        if path.is_absolute() or ".." in path.parts or any(
            token in str(path) for token in ("${{", "*", "?", "[")
        ):
            raise AssertionError(f"upload-artifact path is outside the bounded model: {path}")

    archive_root = Path(
        os.path.commonpath([str(path.parent) for path in source_paths])
    )
    raw_download_path = download_with.get("path")
    if not isinstance(raw_download_path, str):
        raise AssertionError("download-artifact path is not literal")
    download_path = Path(raw_download_path)
    if download_path.is_absolute() or ".." in download_path.parts:
        raise AssertionError("download-artifact path escapes the fixture")

    raw_merge = download_with.get("merge-multiple", False)
    if raw_merge in (True, "true"):
        merge_multiple = True
    elif raw_merge in (False, "false"):
        merge_multiple = False
    else:
        raise AssertionError("download-artifact merge-multiple is not boolean")
    extraction_root = case_root / download_path
    if not merge_multiple:
        extraction_root /= str(_release_candidate(101, 1)["name"])

    materialized: dict[str, Path] = {}
    for source in source_paths:
        relative = source.relative_to(archive_root)
        destination = extraction_root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        if source.name == "CodexInfo.WindowsClient.Setup.exe":
            destination.write_bytes(b"fixture installer")
        elif source.name == "CodexInfo.WindowsClient.update.json":
            destination.write_text(
                json.dumps({"version": _VERSION}), encoding="utf-8"
            )
        else:
            destination.write_bytes(b"unconsumed fixture payload")
        materialized[source.name] = destination

    try:
        return (
            materialized["CodexInfo.WindowsClient.Setup.exe"],
            materialized["CodexInfo.WindowsClient.update.json"],
        )
    except KeyError as error:
        raise AssertionError(f"upload payload omits a consumed file: {error}") from error


def _execute_publication(
    script: str,
    case_root: Path,
    *,
    initial_state: Mapping[str, object] | None,
    environment_overrides: Mapping[str, str] | None = None,
) -> tuple[subprocess.CompletedProcess[str], list[dict[str, object]], dict[str, object]]:
    bin_dir = case_root / "bin"
    bin_dir.mkdir(exist_ok=True)
    _write_publish_gh(bin_dir)
    _write_publish_curl(bin_dir)
    state_path = case_root / "state.json"
    if initial_state is not None:
        state_path.write_text(json.dumps(initial_state), encoding="utf-8")
    log = case_root / "gh.jsonl"
    log.write_text("", encoding="utf-8")
    runner_temp = case_root / "runner-temp"
    runner_temp.mkdir(exist_ok=True)
    environment = os.environ.copy()
    environment.update(
        {
            "GH_TOKEN": "fixture",
            "MERGE_SHA": _MERGE_SHA,
            "MOCK_GH_LOG": str(log),
            "MOCK_GH_STATE": str(state_path),
            "PATH": f"{bin_dir}:{environment['PATH']}",
            "REPOSITORY": _REPOSITORY,
            "RUNNER_TEMP": str(runner_temp),
            "TAG": f"windows-v{_VERSION}",
            "VERSION": _VERSION,
        }
    )
    if environment_overrides:
        environment.update(environment_overrides)
    result = _command(
        ("bash", "-c", script), cwd=case_root, env=environment, check=False
    )
    calls = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
    final_state = json.loads(state_path.read_text(encoding="utf-8"))
    return result, calls, final_state


def _release_publish_tests(windows_workflow: str, release_workflow: str) -> int:
    script = _step_script(release_workflow, "Publish or verify the exact release state")
    cases = 0
    with tempfile.TemporaryDirectory(prefix="codex-info-release-publish-") as raw_root:
        root = Path(raw_root)

        linux_output = root / "bundle"
        linux_output.mkdir()
        subprocess.run(
            [
                "bash",
                str(ROOT / "scripts" / "build_linux_bundle.sh"),
                "--binary",
                "/usr/bin/true",
                "--version",
                _VERSION,
                "--source-sha",
                _FINAL_HEAD,
                "--run-id",
                "99",
                "--run-attempt",
                "1",
                "--output-dir",
                str(linux_output),
            ],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )

        def copy_linux_assets(candidate: Path) -> tuple[Path, ...]:
            copied = []
            for path in linux_output.iterdir():
                destination = candidate / path.name
                shutil.copy2(path, destination)
                copied.append(destination)
            return tuple(sorted(copied))

        absent_root = root / "absent"
        absent_root.mkdir()
        setup, manifest = _materialize_candidate_handoff(
            windows_workflow, release_workflow, absent_root
        )
        linux_assets = copy_linux_assets(setup.parent)
        state = _publication_state("absent", setup, manifest, *linux_assets)
        result, calls, final_state = _execute_publication(
            script,
            absent_root,
            initial_state=state,
            environment_overrides={
                "FINAL_HEAD": _FINAL_HEAD,
                "LINUX_PRESENT": "true",
                "WINDOWS_PRESENT": "true",
            },
        )
        if result.returncode != 0:
            raise AssertionError(
                "artifact handoff did not materialize the publisher input paths"
            )
        mutations = _publish_mutations(calls)
        if (
            len([call for call in mutations if call.get("tool") == "curl"]) != 5
            or len(
                [
                    call
                    for call in mutations
                    if call.get("tool") == "gh" and "POST" in call["args"]
                ]
            )
            != 1
            or len(
                [
                    call
                    for call in mutations
                    if call.get("tool") == "gh" and "PATCH" in call["args"]
                ]
            )
            != 1
            or final_state["details"]["901"]["draft"] is not False
            or len(final_state["assets"]["901"]) != 5
        ):
            raise AssertionError("fully absent release did not create-upload-publish exactly once")
        cases += 1

        # A second same-tag holder acquires the lock after the first publication
        # and must observe an exact published no-op without another mutation.
        result, calls, _ = _execute_publication(
            script,
            absent_root,
            initial_state=None,
            environment_overrides={
                "FINAL_HEAD": _FINAL_HEAD,
                "LINUX_PRESENT": "true",
                "WINDOWS_PRESENT": "true",
            },
        )
        if result.returncode != 0 or _publish_mutations(calls):
            raise AssertionError("concurrent holder did not reduce to exact published no-op")
        cases += 1

        for kind in (
            "orphan-tag",
            "release-only",
            "draft",
            "partial",
            "target-mismatch",
            "asset-mismatch",
        ):
            case_root = root / kind
            case_root.mkdir()
            setup, manifest = _materialize_candidate_handoff(
                windows_workflow, release_workflow, case_root
            )
            linux_assets = copy_linux_assets(setup.parent)
            state = _publication_state(kind, setup, manifest, *linux_assets)
            result, calls, _ = _execute_publication(
                script,
                case_root,
                initial_state=state,
                environment_overrides={
                    "FINAL_HEAD": _FINAL_HEAD,
                    "LINUX_PRESENT": "true",
                    "WINDOWS_PRESENT": "true",
                },
            )
            if result.returncode == 0 or _publish_mutations(calls):
                raise AssertionError(f"invalid existing release state was repaired: {kind}")
            cases += 1

        linux_root = root / "linux"
        linux_root.mkdir()
        linux_candidate = linux_root / "release-candidate"
        linux_candidate.mkdir()
        copy_linux_assets(linux_candidate)
        linux_state = _publication_state("absent", linux_candidate, linux_candidate)
        result, calls, final_state = _execute_publication(
            script,
            linux_root,
            initial_state=linux_state,
            environment_overrides={
                "FINAL_HEAD": _FINAL_HEAD,
                "LINUX_PRESENT": "true",
                "WINDOWS_PRESENT": "false",
            },
        )
        if result.returncode == 0 or _publish_mutations(calls):
            raise AssertionError(
                "Linux-only publication bypassed the Windows asset requirement"
            )
        cases += 1

        windows_only_root = root / "windows-only"
        windows_only_root.mkdir()
        setup, manifest = _materialize_candidate_handoff(
            windows_workflow, release_workflow, windows_only_root
        )
        windows_only_state = _publication_state("absent", setup, manifest)
        result, calls, _ = _execute_publication(
            script,
            windows_only_root,
            initial_state=windows_only_state,
            environment_overrides={
                "FINAL_HEAD": _FINAL_HEAD,
                "LINUX_PRESENT": "false",
                "WINDOWS_PRESENT": "true",
            },
        )
        if result.returncode == 0 or _publish_mutations(calls):
            raise AssertionError("Windows-only publication bypassed the Linux asset requirement")
        cases += 1

        mismatch_root = root / "linux-manifest-mismatch"
        mismatch_root.mkdir()
        mismatch_candidate = mismatch_root / "release-candidate"
        mismatch_candidate.mkdir()
        copy_linux_assets(mismatch_candidate)
        mismatch_manifest = next(mismatch_candidate.glob("*.manifest.json"))
        mismatch_document = json.loads(mismatch_manifest.read_text(encoding="utf-8"))
        mismatch_document["files"][0]["sha256"] = "0" * 64
        mismatch_manifest.write_text(
            json.dumps(mismatch_document), encoding="utf-8"
        )
        mismatch_candidate.joinpath("CodexInfo.WindowsClient.Setup.exe").write_bytes(
            b"fixture installer"
        )
        mismatch_candidate.joinpath(
            "CodexInfo.WindowsClient.update.json"
        ).write_text(json.dumps({"version": _VERSION}), encoding="utf-8")
        mismatch_state = _publication_state("absent", mismatch_candidate, mismatch_candidate)
        result, calls, _ = _execute_publication(
            script,
            mismatch_root,
            initial_state=mismatch_state,
            environment_overrides={
                "FINAL_HEAD": _FINAL_HEAD,
                "LINUX_PRESENT": "true",
                "WINDOWS_PRESENT": "true",
            },
        )
        if result.returncode == 0 or _publish_mutations(calls):
            raise AssertionError("Linux manifest mismatch was published")
        cases += 1
    return cases


def release_self_test() -> int:
    baseline = sources()
    errors = validate(baseline)
    if errors:
        raise AssertionError("production workflow contract failed: " + "; ".join(errors))
    cases = _release_resolution_tests(baseline["release.yml"])
    print(f"workflow-quality-gate: PASS release_resolution_cases={cases}")
    return 0


def self_test() -> int:
    baseline = sources()
    errors = validate(baseline)
    if errors:
        raise AssertionError("production workflow contract failed: " + "; ".join(errors))

    mutations = (
        ("feat-integration.yml", "release_candidate: false", "release_candidate: true"),
        ("feat-integration.yml", "--find-copies-harder", "--no-renames"),
        (
            "version-prepare.yml",
            "expected_version_transition=true",
            "expected_version_transition=false",
        ),
        ("version-prepare.yml", "cancel-in-progress: false", "cancel-in-progress: true"),
        ("version-prepare.yml", "git push origin", "git push --force origin"),
        ("selective-quality.yml", "ref: ${{ github.workflow_sha }}", "ref: ${{ inputs.source_sha }}"),
        (
            "selective-quality.yml",
            "if: always() && inputs.release_candidate",
            "if: always()",
        ),
        ("selective-quality.yml", "  windows-quality:\n", "  omitted-windows-quality:\n"),
        ("windows-client.yml", "New-WindowsUpdateManifest.ps1", "Omitted-Manifest.ps1"),
        ("rust.yml", "cargo test --locked --all-targets -- --nocapture", "true"),
        ("codeql.yml", "  workflow_call:\n", "  schedule:\n"),
        (
            "release.yml",
            "name: Resolve the immutable publication snapshot",
            "name: Resolve an incomplete snapshot",
        ),
        (
            "release.yml",
            "group: release-windows-client-${{ needs.resolve.outputs.tag }}",
            "group: release-windows-client-global",
        ),
        (
            "version-prepare.yml",
            "if: steps.current.outputs.observer != 'true'",
            "if: steps.current.outputs.observer == 'true'",
        ),
        (
            "version-prepare.yml",
            "persist-credentials: true",
            "persist-credentials: false",
        ),
        (
            "version-prepare.yml",
            "ref: ${{ github.workflow_sha }}",
            "ref: ${{ github.event.pull_request.base.sha }}",
        ),
        (
            "feat-integration.yml",
            "ref: ${{ github.workflow_sha }}",
            "ref: ${{ github.event.pull_request.head.sha }}",
        ),
        (
            "version-prepare.yml",
            "event_head=${{ github.event.pull_request.head.sha }}",
            "event_head=${{ github.event.pull_request.base.sha }}",
        ),
        (
            "version-prepare.yml",
            "QUALITY_RESULT: ${{ needs.selective-quality.result }}",
            "QUALITY_RESULT: ${{ needs.version-prepared.result }}",
        ),
        (
            "selective-quality.yml",
            "    name: windows-quality\n",
            "    name: renamed-windows-quality\n",
        ),
        (
            "selective-quality.yml",
            "pr_number: ${{ inputs.pr_number }}",
            "pr_number: ${{ inputs.source_sha }}",
        ),
        (
            "linux-ui-quality.yml",
            "ref: ${{ inputs.source_sha }}",
            "ref: ${{ github.sha }}",
        ),
        (
            "release.yml",
            "artifact_id: ${{ steps.resolve.outputs.artifact_id }}",
            "artifact_id: ${{ steps.resolve.outputs.run_id }}",
        ),
        (
            "release.yml",
            "if: steps.revalidate.outputs.proceed == 'true'",
            "if: steps.revalidate.outputs.proceed != 'true'",
        ),
        (
            "release.yml",
            "actions/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1",
            "actions/create-github-app-token@v3.2.0",
        ),
    )
    cases = 1
    for name, old, new in mutations:
        candidate = dict(baseline)
        if old not in candidate[name]:
            raise AssertionError(f"mutation target is missing: {name}: {old}")
        candidate[name] = candidate[name].replace(old, new, 1)
        if not validate(candidate):
            raise AssertionError(f"workflow mutation was accepted: {name}: {old}")
        cases += 1
    observer_cases = _current_observer_tests(baseline["version-prepare.yml"])
    version_cases = _version_state_tests(baseline["version-prepare.yml"])
    release_candidate_cases = _selected_quality_release_candidate_tests(
        baseline["selective-quality.yml"]
    )
    copy_cases = _git_copy_detection_test()
    acceptance_cases = _acceptance_result_tests(baseline["version-prepare.yml"])
    release_resolution_cases = _release_resolution_tests(baseline["release.yml"])
    release_lock_cases = _release_lock_tests(baseline["release.yml"])
    release_publish_cases = _release_publish_tests(
        baseline["windows-client.yml"], baseline["release.yml"]
    )
    total_cases = (
        cases
        + observer_cases
        + version_cases
        + release_candidate_cases
        + copy_cases
        + acceptance_cases
        + release_resolution_cases
        + release_lock_cases
        + release_publish_cases
    )
    print(
        "workflow-quality-gate: PASS "
        f"total_cases={total_cases} static_cases={cases} "
        f"observer_cases={observer_cases} version_cases={version_cases} "
        f"release_candidate_cases={release_candidate_cases} "
        f"copy_cases={copy_cases} "
        f"acceptance_cases={acceptance_cases} "
        f"release_resolution_cases={release_resolution_cases} "
        f"release_lock_cases={release_lock_cases} "
        f"release_publish_cases={release_publish_cases}"
    )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--release-self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test and args.release_self_test:
        parser.error("--self-test and --release-self-test are mutually exclusive")
    if args.release_self_test:
        return release_self_test()
    if args.self_test:
        return self_test()
    errors = validate(sources())
    if errors:
        for error in errors:
            print(f"workflow-quality-gate: FAIL {error}")
        return 1
    print("workflow-quality-gate: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

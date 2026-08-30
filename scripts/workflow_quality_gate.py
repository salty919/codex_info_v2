#!/usr/bin/env python3
"""Local-only causal contract for the selective workflow graph."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Mapping, Sequence

import yaml


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_NAMES = (
    "codeql.yml",
    "feat-integration.yml",
    "linux-ui-quality.yml",
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


def validate(workflows: Mapping[str, str]) -> list[str]:
    errors: list[str] = []

    def count(name: str, marker: str, expected: int) -> None:
        actual = workflows[name].count(marker)
        if actual != expected:
            errors.append(f"{name}: {marker!r}: expected {expected}, found {actual}")

    if set(workflows) != set(WORKFLOW_NAMES):
        errors.append("workflow set differs from the eight declared owners")
        return errors

    joined = "\n".join(workflows.values())
    for forbidden in (
        "feat_integration_check_reporter.py",
        "final_head_check_reporter.py",
        "acceptance-verdict",
        "final_acceptance_gate.sh",
        "release_state_gate.py",
        "ci_trust_fixture.py",
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
        "name: feat-acceptance",
        "release_candidate: false",
        '--name-status -z "$BASE_SHA...$HEAD_SHA"',
    ):
        if marker not in feat:
            errors.append(f"feat-integration.yml: missing {marker}")
    for marker in ('"$HEAD_REF" !=', "workflow_dispatch:"):
        if marker in feat:
            errors.append(f"feat-integration.yml: branch/dispatch overconstraint {marker}")
    count("feat-integration.yml", "--find-copies-harder", 1)

    version = workflows["version-prepare.yml"]
    for marker in (
        "name: version-prepared",
        "'Observe generated H1' || 'acceptance'",
        "ref: ${{ github.event.pull_request.base.sha }}",
        "git log --first-parent --format=%H",
        "generated_version=true",
        "generated_observer=true",
        "':(exclude)Cargo.toml'",
        "cmp -s",
        'git push origin "$commit_sha:refs/heads/$HEAD_REF"',
        "codex-main-quality:pr=$PR_NUMBER:head=$QUALITY_SHA:run=$GITHUB_RUN_ID",
        "release_candidate: true",
    ):
        if marker not in version:
            errors.append(f"version-prepare.yml: missing {marker}")
    for marker in (
        "--force",
        "actions/checkout@v4\n        with:\n          ref: ${{ github.event.pull_request.head.sha }}",
    ):
        if marker in version:
            errors.append(f"version-prepare.yml: write path overconstraint {marker}")
    count("version-prepare.yml", "checks: write", 1)
    count("version-prepare.yml", "checks: read", 1)
    count("version-prepare.yml", "cancel-in-progress: false", 1)
    count("version-prepare.yml", "repos/$REPOSITORY/check-runs", 1)
    count("version-prepare.yml", "commits/$HEAD_SHA/check-runs", 1)
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
    count("selective-quality.yml", "ref: ${{ inputs.base_sha }}", 1)

    windows = workflows["windows-client.yml"]
    for marker in (
        "dotnet test windows-client/CodexInfo.WindowsClient.sln",
        "Build-WindowsInstaller.ps1",
        "Run-WindowsClientE2E.ps1",
        "windows_window_move_smoke.ps1",
        "New-WindowsUpdateManifest.ps1",
        "name: release-candidate-${{ inputs.pr_number }}",
    ):
        if marker not in windows:
            errors.append(f"windows-client.yml: missing {marker}")
    count("windows-client.yml", "uses: actions/upload-artifact@v4", 1)

    rust = workflows["rust.yml"]
    for marker in (
        "cargo test --locked --all-targets -- --nocapture",
        "cargo build --release --locked",
        "scripts/cli_contract_e2e.sh",
        "scripts/record_daemon_e2e.sh",
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
        "github.event.pull_request.merged == true",
        "commits/$HEAD_SHA/check-runs?check_name=acceptance",
        "codex-main-quality:pr=$PR_NUMBER:head=$HEAD_SHA:run=",
        'test("(^| / )windows-quality$")',
        "name: release-candidate-${{ github.event.pull_request.number }}",
        'gh release create "$tag" "$setup" "$manifest"',
        "--draft",
        'gh release edit "$tag" --repo "$REPOSITORY" --draft=false',
    ):
        if marker not in release:
            errors.append(f"release.yml: missing {marker}")
    if "actions/checkout" in release:
        errors.append("release.yml: write job must not checkout source")
    if "status=success" in release:
        errors.append("release.yml: latest failure must not fall back to an older success")

    return errors


def _step_script(workflow: str, step_name: str) -> str:
    document = yaml.safe_load(workflow)
    for job in document["jobs"].values():
        for step in job.get("steps", []):
            if step.get("name") == step_name:
                script = step.get("run")
                if isinstance(script, str) and script:
                    return script
    raise AssertionError(f"workflow step was not found: {step_name}")


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


def _run_version_step(
    fixture: dict[str, Path | str | int],
    script: str,
    head: str,
    *,
    producer_run_id: int | None = None,
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
    checks = runner_temp / "checks.json"
    check_runs: list[dict[str, object]] = []
    if producer_run_id is not None:
        check_runs.append(
            {
                "name": "acceptance",
                "head_sha": head,
                "external_id": (
                    f"codex-main-quality:pr=44:head={head}:run={producer_run_id}"
                ),
            }
        )
    checks.write_text(json.dumps({"check_runs": check_runs}), encoding="utf-8")
    gh = bin_dir / "gh"
    gh.write_text(
        "#!/usr/bin/env bash\n"
        "set -euo pipefail\n"
        "cat \"$MOCK_CHECKS\"\n",
        encoding="utf-8",
    )
    gh.chmod(0o755)
    environment = os.environ.copy()
    environment.update(
        {
            "BASE_SHA": base,
            "GH_TOKEN": "fixture",
            "GITHUB_OUTPUT": str(output),
            "HEAD_REF": "case",
            "HEAD_REPOSITORY": "example/project",
            "HEAD_SHA": head,
            "MOCK_CHECKS": str(checks),
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

        for name, relative, expected_owners in (
            ("windows", "windows-client/src/App.cs", ["WINDOWS"]),
            ("linux", "src/lib.rs", ["LINUX_BACKEND"]),
        ):
            fixture = _new_version_fixture(root, name)
            seed = fixture["seed"]
            assert isinstance(seed, Path)
            (seed / relative).write_text(f"{name} change\n", encoding="utf-8")
            h0 = _commit(fixture, name)
            first, first_values = _run_version_step(fixture, script, h0)
            h1 = _remote_head(fixture)
            if (
                first.returncode != 0
                or first_values.get("ready") != "true"
                or first_values.get("generated_head") != "true"
                or first_values.get("quality_sha") != h1
                or h1 == h0
            ):
                raise AssertionError(f"{name} H0 did not continue on its generated H1")
            first_owners = json.loads(first_values["selection_json"])["owners"]
            if first_owners != expected_owners:
                raise AssertionError(
                    f"{name} H0 selected {first_owners}, expected {expected_owners}"
                )
            cases += 1
            second, second_values = _run_version_step(
                fixture, script, h1, producer_run_id=12345
            )
            if (
                second.returncode != 0
                or second_values.get("ready") != "false"
                or second_values.get("generated_head") != "false"
                or second_values.get("generated_observer") != "true"
                or second_values.get("quality_sha") != h1
            ):
                raise AssertionError(f"{name} generated H1 was not suppressed")
            owners = json.loads(second_values["selection_json"])["owners"]
            if owners != expected_owners:
                raise AssertionError(f"{name} H1 selected {owners}, expected {expected_owners}")
            cases += 1

            if name == "windows":
                _git(seed, "pull", "--quiet", "--ff-only", "origin", "case")
                later = seed / "windows-client/src/Later.cs"
                later.write_text("class Later {}\n", encoding="utf-8")
                h2 = _commit(fixture, "later Windows change")
                third, third_values = _run_version_step(fixture, script, h2)
                third_owners = json.loads(third_values["selection_json"])["owners"]
                if (
                    third.returncode != 0
                    or third_values.get("quality_sha") != h2
                    or third_values.get("generated_head") != "false"
                    or third_owners != ["WINDOWS"]
                ):
                    raise AssertionError(
                        "H2 retained generated version files in owner selection"
                    )
                cases += 1

        fixture = _new_version_fixture(root, "manual-next")
        seed = fixture["seed"]
        version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(version, str)
        (seed / "windows-client/src/App.cs").write_text("manual next\n", encoding="utf-8")
        _bump(fixture, version)
        head = _commit(fixture, "manual next")
        result, values = _run_version_step(fixture, script, head)
        if result.returncode != 0 or values.get("ready") != "true":
            raise AssertionError("manual exact-next binary head was not accepted")
        if values.get("quality_sha") != head or values.get("generated_head") != "false":
            raise AssertionError("manual exact-next head was treated as generated")
        cases += 1

        fixture = _new_version_fixture(root, "manual-split-next")
        seed = fixture["seed"]
        version = fixture["version"]
        assert isinstance(seed, Path) and isinstance(version, str)
        (seed / "windows-client/src/App.cs").write_text(
            "manual split next\n", encoding="utf-8"
        )
        _commit(fixture, "manual source")
        _bump(fixture, version)
        head = _commit(fixture, "manual version")
        result, values = _run_version_step(fixture, script, head)
        if (
            result.returncode != 0
            or values.get("ready") != "true"
            or values.get("generated_observer") != "false"
            or json.loads(values["selection_json"])["owners"] != ["WINDOWS"]
        ):
            raise AssertionError(
                "byte-identical manual version commit was mistaken for a generated observer"
            )
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


def _final_check_tests(version_workflow: str) -> int:
    script = _step_script(version_workflow, "Require selected quality success")
    cases = 0
    with tempfile.TemporaryDirectory(prefix="codex-info-final-checks-") as raw_root:
        root = Path(raw_root)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        gh = bin_dir / "gh"
        gh.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            "payload=$(cat)\n"
            "printf '%s\\n' \"$payload\" >> \"$MOCK_GH_LOG\"\n"
            "exit \"${MOCK_GH_RC:-0}\"\n",
            encoding="utf-8",
        )
        gh.chmod(0o755)

        def execute(
            *,
            generated: bool,
            observer: bool = False,
            prepare: str = "success",
            quality: str = "success",
            gh_rc: int = 0,
        ) -> tuple[subprocess.CompletedProcess[str], list[dict[str, object]]]:
            log = root / "checks.jsonl"
            log.write_text("", encoding="utf-8")
            environment = os.environ.copy()
            environment.update(
                {
                    "GENERATED_HEAD": str(generated).lower(),
                    "GENERATED_OBSERVER": str(observer).lower(),
                    "GH_TOKEN": "fixture",
                    "GITHUB_RUN_ID": "12345",
                    "MOCK_GH_LOG": str(log),
                    "MOCK_GH_RC": str(gh_rc),
                    "PATH": f"{bin_dir}:{environment['PATH']}",
                    "PREPARE_RESULT": prepare,
                    "PR_NUMBER": "44",
                    "QUALITY_RESULT": quality,
                    "QUALITY_SHA": "c" * 40,
                    "REPOSITORY": "example/project",
                }
            )
            result = _command(
                ("bash", "-c", script), cwd=root, env=environment, check=False
            )
            payloads = [
                json.loads(line)
                for line in log.read_text(encoding="utf-8").splitlines()
            ]
            return result, payloads

        result, payloads = execute(generated=False)
        if result.returncode != 0 or payloads:
            raise AssertionError("current-head success created redundant custom checks")
        cases += 1

        result, payloads = execute(generated=False, quality="failure")
        if result.returncode == 0 or payloads:
            raise AssertionError("current-head quality failure was accepted")
        cases += 1

        result, payloads = execute(
            generated=False, observer=True, quality="skipped"
        )
        if result.returncode != 0 or payloads:
            raise AssertionError("generated-H1 observer created or repeated quality checks")
        cases += 1

        result, payloads = execute(generated=True)
        expected_external = (
            "codex-main-quality:pr=44:head=" + "c" * 40 + ":run=12345"
        )
        if (
            result.returncode != 0
            or [payload["name"] for payload in payloads]
            != ["version-prepared", "acceptance"]
            or any(payload["head_sha"] != "c" * 40 for payload in payloads)
            or any(payload["external_id"] != expected_external for payload in payloads)
            or [payload["conclusion"] for payload in payloads]
            != ["success", "success"]
        ):
            raise AssertionError(f"generated-head success checks are wrong: {payloads}")
        cases += 1

        result, payloads = execute(generated=True, quality="cancelled")
        if (
            result.returncode == 0
            or len(payloads) != 2
            or payloads[-1]["conclusion"] != "failure"
        ):
            raise AssertionError("generated-head abnormal quality was accepted")
        cases += 1

        result, payloads = execute(generated=True, gh_rc=1)
        if result.returncode == 0 or len(payloads) != 1:
            raise AssertionError("failed final-check mutation continued")
        cases += 1
    return cases


def _mock_gh(directory: Path) -> Path:
    binary = directory / "gh"
    binary.write_text(
        "#!/usr/bin/env bash\n"
        "set -euo pipefail\n"
        "case \"$*\" in\n"
        "  *'/check-runs?'*) cat \"$MOCK_CHECKS\" ;;\n"
        "  *'/actions/workflows/version-prepare.yml/runs?'*) cat \"$MOCK_RUNS\" ;;\n"
        "  *\"/actions/runs/$MOCK_DIRECT_RUN_ID/jobs?\"*) cat \"$MOCK_DIRECT_JOBS\" ;;\n"
        "  *\"/actions/runs/$MOCK_DIRECT_RUN_ID\"*) cat \"$MOCK_DIRECT_RUN\" ;;\n"
        "  *'/jobs?'*) cat \"$MOCK_JOBS\" ;;\n"
        "  *'/actions/runs/'*) cat \"$MOCK_RUN\" ;;\n"
        "  *) exit 4 ;;\n"
        "esac\n",
        encoding="utf-8",
    )
    binary.chmod(0o755)
    return binary


def _release_resolution_tests(release_workflow: str) -> int:
    script = _step_script(release_workflow, "Resolve the successful final-head quality run")
    head = "a" * 40
    base_run = {
        "id": 10,
        "path": ".github/workflows/version-prepare.yml",
        "event": "pull_request_target",
        "head_sha": head,
        "conclusion": "success",
    }
    cases = 0
    with tempfile.TemporaryDirectory(prefix="codex-info-release-resolver-") as raw_root:
        root = Path(raw_root)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        _mock_gh(bin_dir)

        def execute(
            runs: list[dict[str, object]],
            jobs: list[dict[str, object]],
            *,
            checks: list[dict[str, object]] | None = None,
            selected_run: dict[str, object] | None = None,
            direct_run: dict[str, object] | None = None,
            direct_jobs: list[dict[str, object]] | None = None,
        ):
            checks_path = root / "checks.json"
            runs_path = root / "runs.json"
            run_path = root / "run.json"
            jobs_path = root / "jobs.json"
            direct_run_path = root / "direct-run.json"
            direct_jobs_path = root / "direct-jobs.json"
            output = root / "output"
            checks_path.write_text(
                json.dumps({"check_runs": checks or []}), encoding="utf-8"
            )
            runs_path.write_text(json.dumps({"workflow_runs": runs}), encoding="utf-8")
            run_path.write_text(
                json.dumps(selected_run or (runs[-1] if runs else {})), encoding="utf-8"
            )
            jobs_path.write_text(json.dumps({"jobs": jobs}), encoding="utf-8")
            direct_run_path.write_text(json.dumps(direct_run or {}), encoding="utf-8")
            direct_jobs_path.write_text(
                json.dumps({"jobs": direct_jobs or []}), encoding="utf-8"
            )
            output.write_text("", encoding="utf-8")
            environment = os.environ.copy()
            environment.update(
                {
                    "GITHUB_OUTPUT": str(output),
                    "HEAD_SHA": head,
                    "MOCK_CHECKS": str(checks_path),
                    "MOCK_DIRECT_JOBS": str(direct_jobs_path),
                    "MOCK_DIRECT_RUN": str(direct_run_path),
                    "MOCK_DIRECT_RUN_ID": str((direct_run or {}).get("id", 0)),
                    "MOCK_JOBS": str(jobs_path),
                    "MOCK_RUN": str(run_path),
                    "MOCK_RUNS": str(runs_path),
                    "PATH": f"{bin_dir}:{environment['PATH']}",
                    "PR_NUMBER": "44",
                    "REPOSITORY": "example/project",
                }
            )
            result = _command(
                ("bash", "-c", script), cwd=root, env=environment, check=False
            )
            return result, _output(output)

        for conclusion, expected in (("skipped", "false"), ("success", "true")):
            result, values = execute(
                [base_run],
                [{"name": "Run selected quality owners / windows-quality", "conclusion": conclusion}],
            )
            if result.returncode != 0 or values.get("publish") != expected:
                raise AssertionError(f"Windows {conclusion} release decision is wrong")
            cases += 1

        latest = dict(base_run, id=20)
        result, values = execute(
            [base_run, latest],
            [{"name": "selected / windows-quality / windows-quality", "conclusion": "success"}],
            selected_run=latest,
        )
        if result.returncode != 0 or values.get("run_id") != "20":
            raise AssertionError("latest equivalent successful producer was not selected")
        cases += 1

        generated_run = dict(base_run, head_sha="b" * 40)
        generated_check = {
            "name": "acceptance",
            "head_sha": head,
            "external_id": (
                "codex-main-quality:pr=44:head=" + head + ":run=10"
            ),
        }
        result, values = execute(
            [],
            [{"name": "selected / windows-quality", "conclusion": "success"}],
            checks=[generated_check],
            selected_run=generated_run,
        )
        if result.returncode != 0 or values.get("run_id") != "10":
            raise AssertionError("generated-head check did not resolve its H0 producer")
        cases += 1

        observer_run = dict(base_run, id=20)
        result, values = execute(
            [observer_run],
            [{"name": "selected / windows-quality", "conclusion": "success"}],
            checks=[generated_check],
            selected_run=generated_run,
            direct_run=observer_run,
            direct_jobs=[{"name": "Observe generated H1", "conclusion": "success"}],
        )
        if result.returncode != 0 or values.get("run_id") != "10":
            raise AssertionError("generated-H1 observer replaced its quality producer")
        cases += 1

        for conclusion in ("failure", "cancelled"):
            later_abnormal = dict(base_run, id=20, conclusion=conclusion)
            result, _ = execute(
                [later_abnormal],
                [{"name": "selected / windows-quality", "conclusion": "success"}],
                checks=[generated_check],
                selected_run=later_abnormal,
            )
            if result.returncode == 0:
                raise AssertionError(
                    f"later {conclusion} fell back to an older successful producer"
                )
            cases += 1

        bad_cases = (
            ([], [{"name": "selected / windows-quality", "conclusion": "success"}]),
            ([base_run], []),
            (
                [base_run],
                [
                    {"name": "a / windows-quality", "conclusion": "success"},
                    {"name": "b / windows-quality", "conclusion": "success"},
                ],
            ),
            ([base_run], [{"name": "selected / windows-quality", "conclusion": "failure"}]),
            ([base_run], [{"name": "selected / windows-quality", "conclusion": "cancelled"}]),
        )
        for runs, jobs in bad_cases:
            result, _ = execute(runs, jobs)
            if result.returncode == 0:
                raise AssertionError(f"invalid release producer was accepted: {runs}, {jobs}")
            cases += 1

        malformed_check = dict(generated_check, external_id="codex-main-quality:broken")
        result, _ = execute(
            [],
            [{"name": "selected / windows-quality", "conclusion": "success"}],
            checks=[malformed_check],
            selected_run=generated_run,
        )
        if result.returncode == 0:
            raise AssertionError("malformed generated-head authority was accepted")
        cases += 1
    return cases


def _release_publish_tests(release_workflow: str) -> int:
    script = _step_script(release_workflow, "Publish the Windows release")
    cases = 0
    with tempfile.TemporaryDirectory(prefix="codex-info-release-publish-") as raw_root:
        root = Path(raw_root)
        bin_dir = root / "bin"
        candidate = root / "release-candidate"
        bin_dir.mkdir()
        candidate.mkdir()
        setup = candidate / "CodexInfo.WindowsClient.Setup.exe"
        manifest = candidate / "CodexInfo.WindowsClient.update.json"
        setup.write_bytes(b"fixture installer")

        gh = bin_dir / "gh"
        gh.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            "printf '%s\\n' \"$*\" >> \"$MOCK_GH_LOG\"\n"
            "if [[ \"${1:-} ${2:-}\" == 'release create' ]]; then\n"
            "  [[ -f \"${4:-}\" && -f \"${5:-}\" ]] || exit 2\n"
            "  exit \"${MOCK_CREATE_RC:-0}\"\n"
            "fi\n"
            "if [[ \"${1:-} ${2:-}\" == 'release edit' ]]; then\n"
            "  exit \"${MOCK_EDIT_RC:-0}\"\n"
            "fi\n"
            "exit 3\n",
            encoding="utf-8",
        )
        gh.chmod(0o755)

        def execute(
            manifest_value: object,
            *,
            create_rc: int = 0,
            edit_rc: int = 0,
            setup_present: bool = True,
        ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
            manifest.write_text(json.dumps(manifest_value), encoding="utf-8")
            if setup_present:
                setup.write_bytes(b"fixture installer")
            else:
                setup.unlink(missing_ok=True)
            log = root / "gh.log"
            log.write_text("", encoding="utf-8")
            environment = os.environ.copy()
            environment.update(
                {
                    "GH_TOKEN": "fixture",
                    "MERGE_SHA": "b" * 40,
                    "MOCK_CREATE_RC": str(create_rc),
                    "MOCK_EDIT_RC": str(edit_rc),
                    "MOCK_GH_LOG": str(log),
                    "PATH": f"{bin_dir}:{environment['PATH']}",
                    "REPOSITORY": "example/project",
                }
            )
            result = _command(
                ("bash", "-c", script), cwd=root, env=environment, check=False
            )
            return result, log.read_text(encoding="utf-8").splitlines()

        result, calls = execute({"version": "1.2.3"})
        if result.returncode != 0 or len(calls) != 2:
            raise AssertionError("valid candidate did not complete the two release transitions")
        if not (
            calls[0].startswith(
                "release create windows-v1.2.3 "
                "release-candidate/CodexInfo.WindowsClient.Setup.exe "
                "release-candidate/CodexInfo.WindowsClient.update.json"
            )
            and "--target " + "b" * 40 in calls[0]
            and calls[0].endswith("--draft")
            and calls[1]
            == "release edit windows-v1.2.3 --repo example/project --draft=false"
        ):
            raise AssertionError(f"release transition order or arguments are wrong: {calls}")
        cases += 1

        result, calls = execute({"version": "1.2.3"}, create_rc=1)
        if result.returncode == 0 or len(calls) != 1 or "release create" not in calls[0]:
            raise AssertionError("failed draft creation continued to publication")
        cases += 1

        result, calls = execute({"version": "1.2.3"}, edit_rc=1)
        if result.returncode == 0 or len(calls) != 2 or not calls[0].endswith("--draft"):
            raise AssertionError("failed publication did not leave the release at draft transition")
        cases += 1

        result, calls = execute({"version": "1.2"})
        if result.returncode == 0 or calls:
            raise AssertionError("invalid candidate version reached the release API")
        cases += 1

        result, calls = execute({"version": "1.2.3"}, setup_present=False)
        if result.returncode == 0 or len(calls) != 1:
            raise AssertionError("missing installer was published")
        cases += 1
    return cases


def self_test() -> int:
    baseline = sources()
    errors = validate(baseline)
    if errors:
        raise AssertionError("production workflow contract failed: " + "; ".join(errors))

    mutations = (
        ("feat-integration.yml", "name: feat-acceptance", "name: finalize"),
        ("feat-integration.yml", "release_candidate: false", "release_candidate: true"),
        ("feat-integration.yml", "--find-copies-harder", "--no-renames"),
        ("version-prepare.yml", "generated_version=true", "generated_version=false"),
        ("version-prepare.yml", "cancel-in-progress: false", "cancel-in-progress: true"),
        ("version-prepare.yml", "git push origin", "git push --force origin"),
        ("version-prepare.yml", "checks: write", "checks: read"),
        ("selective-quality.yml", "ref: ${{ inputs.base_sha }}", "ref: ${{ inputs.source_sha }}"),
        ("selective-quality.yml", "  windows-quality:\n", "  omitted-windows-quality:\n"),
        ("windows-client.yml", "New-WindowsUpdateManifest.ps1", "Omitted-Manifest.ps1"),
        ("rust.yml", "cargo test --locked --all-targets -- --nocapture", "true"),
        ("codeql.yml", "  workflow_call:\n", "  schedule:\n"),
        ("release.yml", "github.event.pull_request.merged == true", "always()"),
        ("release.yml", 'test("(^| / )windows-quality$")', 'test("quality")'),
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
    version_cases = _version_state_tests(baseline["version-prepare.yml"])
    copy_cases = _git_copy_detection_test()
    final_check_cases = _final_check_tests(baseline["version-prepare.yml"])
    release_resolution_cases = _release_resolution_tests(baseline["release.yml"])
    release_publish_cases = _release_publish_tests(baseline["release.yml"])
    print(
        "workflow-quality-gate: PASS "
        f"static_cases={cases} version_cases={version_cases} "
        f"copy_cases={copy_cases} "
        f"final_check_cases={final_check_cases} "
        f"release_resolution_cases={release_resolution_cases} "
        f"release_publish_cases={release_publish_cases}"
    )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
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

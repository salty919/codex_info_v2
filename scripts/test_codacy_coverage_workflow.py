"""Focused contract for complete, isolated Codacy coverage upload."""

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUST = (ROOT / ".github/workflows/rust.yml").read_text(encoding="utf-8")
WINDOWS = (ROOT / ".github/workflows/windows-client.yml").read_text(encoding="utf-8")
UPLOAD = (ROOT / ".github/workflows/codacy-coverage.yml").read_text(encoding="utf-8")
RUNSETTINGS = (ROOT / "windows-client/CodeCoverage.runsettings").read_text(
    encoding="utf-8"
)


def require(source: str, marker: str) -> None:
    if marker not in source:
        raise AssertionError(f"missing Codacy coverage contract: {marker}")


def forbid(source: str, marker: str) -> None:
    if marker in source:
        raise AssertionError(f"forbidden Codacy coverage contract: {marker}")


def main() -> int:
    for marker in (
        "cargo-llvm-cov@0.9.0",
        "cargo llvm-cov --workspace --locked --all-targets --cobertura",
        "codacy-coverage-rust-v1-head-${{ inputs.source_sha }}",
        "if-no-files-found: error",
    ):
        require(RUST, marker)
    forbid(RUST, "run: cargo test --workspace")
    forbid(RUST, "CODACY_API_TOKEN")

    for marker in (
        "-p:DeterministicSourcePaths=true",
        '--collect:"Code Coverage"',
        "codacy-coverage-windows-v1-head-${{ inputs.source_sha }}",
        "filename=\"/_/",
    ):
        require(WINDOWS, marker)
    if WINDOWS.count("dotnet test windows-client/CodexInfo.WindowsClient.sln") != 1:
        raise AssertionError("Windows tests must execute exactly once")
    forbid(WINDOWS, "CODACY_API_TOKEN")
    require(RUNSETTINGS, "<DeterministicReport>True</DeterministicReport>")

    for marker in (
        'name: Codacy coverage upload',
        'workflows: ["Feat integration", "Main PR quality"]',
        "permissions: {}",
        "github.event.workflow_run.head_repository.full_name == github.repository",
        "github.event.workflow_run.actor.login != 'dependabot[bot]'",
        "github.event.workflow_run.triggering_actor.login != 'dependabot[bot]'",
        "python3 scripts/codacy_coverage_artifacts.py",
        "steps.pair.outputs.ready == 'true'",
        "CODACY_API_TOKEN: ${{ secrets.CODACY_API_TOKEN }}",
        "CODACY_ORGANIZATION_PROVIDER: gh",
        "CODACY_PROJECT_NAME: codex_info_v2",
        "CODACY_USERNAME: salty919",
        'report --commit-uuid "$SOURCE_SHA"',
        "codacy-coverage-reporter/releases/download/14.1.3/",
        "15c5f052207d27b8501ab5d5910c68c206ea70b0157ee1725132780530580875",
    ):
        require(UPLOAD, marker)
    for marker in (
        "  push:\n",
        "  pull_request:\n",
        "secrets: inherit",
        "WORKFLOW_HEAD_SHA",
    ):
        forbid(UPLOAD, marker)

    print("codacy-coverage-workflow-test: PASS cases=4")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

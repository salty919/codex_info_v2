#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "main-pr-reproduction: FAIL: $*" >&2
    exit 1
}

source_sha=''
phase='all'
pr_number=''
while (($# > 0)); do
    case "$1" in
        --source-sha)
            [[ $# -ge 2 && -z "$source_sha" ]] || fail '--source-sha requires one value'
            source_sha="$2"
            shift 2
            ;;
        --phase)
            [[ $# -ge 2 && "$phase" == 'all' ]] || fail '--phase requires one value'
            phase="$2"
            shift 2
            ;;
        --pr)
            [[ $# -ge 2 && -z "$pr_number" ]] || fail '--pr requires one value'
            pr_number="$2"
            shift 2
            ;;
        *) fail "unknown argument: $1" ;;
    esac
done

[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || fail '--source-sha must be one full lowercase commit SHA'
git cat-file -e "${source_sha}^{commit}" 2>/dev/null || fail "source commit is unavailable: $source_sha"
head_sha="$(git rev-parse HEAD)"
[[ "$head_sha" == "$source_sha" ]] ||
    fail "worktree HEAD does not match --source-sha: head=$head_sha source=$source_sha"

run_linux_cli() {
    command -v cargo >/dev/null || fail 'cargo is unavailable'
    cargo build --workspace --release --locked
    bash scripts/cli_contract_e2e.sh
}

run_linux_ui() {
    command -v cargo >/dev/null || fail 'cargo is unavailable'
    command -v xvfb-run >/dev/null || fail 'xvfb-run is unavailable'
    cargo build --release --locked
    xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
        bash scripts/x11_startup_visual_gate.sh
    xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
        bash scripts/x11_graph_visual_gate.sh
}

run_linux_distribution() {
    command -v cargo >/dev/null || fail 'cargo is unavailable'
    command -v xvfb-run >/dev/null || fail 'xvfb-run is unavailable'
    command -v jq >/dev/null || fail 'jq is unavailable'
    local target='x86_64-unknown-linux-gnu'
    local output_root candidate_root archive
    output_root="$(mktemp -d /tmp/codex-info-main-pr-bundle.XXXXXX)"
    candidate_root="$(mktemp -d /tmp/codex-info-main-pr-candidate.XXXXXX)"
    echo "main-pr-reproduction: bundle evidence=$output_root"
    echo "main-pr-reproduction: extracted candidate=$candidate_root"
    cargo build --release --locked --target "$target" \
        -p codex_info -p codex-info-recorder -p codex-info-rest
    bash scripts/build_linux_bundle.sh \
        --ui-binary "target/$target/release/codex_info" \
        --recorder-binary "target/$target/release/codex_info_recorder" \
        --rest-binary "target/$target/release/codex_info_rest" \
        --output-dir "$output_root" \
        --source-sha "$source_sha" \
        --run-id 1 \
        --run-attempt 1
    bash scripts/test_linux_bundle.sh --bundle-dir "$output_root"
    archive="$(find "$output_root" -mindepth 1 -maxdepth 1 -type f \
        -name 'codex-info-*-x86_64-unknown-linux-gnu.tar.gz' -print -quit)"
    [[ -n "$archive" ]] || fail 'Linux bundle archive is missing'
    tar -xzf "$archive" -C "$candidate_root" --no-same-owner
    CODEX_INFO_ACCEPTANCE_BINARY="$candidate_root/codex_info" \
        xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
        bash scripts/x11_service_recovery_visual_gate.sh
}

run_windows_ui() {
    command -v powershell.exe >/dev/null || fail 'powershell.exe is unavailable'
    command -v wslpath >/dev/null || fail 'wslpath is unavailable'
    local runner
    runner="$(wslpath -w "$repo_root/windows-client/tools/Reproduce-WindowsInstalledE2E.ps1")"
    powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass \
        -File "$runner" -SourceSha "$source_sha"
}

show_external_checks() {
    [[ "$pr_number" =~ ^[1-9][0-9]*$ ]] || fail '--pr is required for external-checks'
    command -v gh >/dev/null || fail 'gh is unavailable'
    gh pr checks "$pr_number" --repo salty919/codex_info_v2
}

case "$phase" in
    linux-cli) run_linux_cli ;;
    linux-ui) run_linux_ui ;;
    linux-distribution) run_linux_distribution ;;
    windows-ui) run_windows_ui ;;
    external-checks) show_external_checks ;;
    all)
        run_linux_cli
        run_linux_ui
        run_linux_distribution
        run_windows_ui
        if [[ -n "$pr_number" ]]; then show_external_checks; fi
        ;;
    *) fail "unknown phase: $phase" ;;
esac

echo "main-pr-reproduction: PASS phase=$phase source_sha=$source_sha"

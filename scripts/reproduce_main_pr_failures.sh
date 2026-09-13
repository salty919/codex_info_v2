#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "main-pr-reproduction: FAIL: $*" >&2
    exit 1
}

linux_ui_tools_available() {
    local command_name
    for command_name in xvfb-run xauth xwininfo xprop xwd python3; do
        command -v "$command_name" >/dev/null || return 1
    done
    if command -v ldconfig >/dev/null; then
        ldconfig -p 2>/dev/null | grep -F 'libxkbcommon-x11.so.0' >/dev/null || return 1
    fi
}

ensure_linux_ui_host_dependencies() {
    linux_ui_tools_available && return

    command -v dpkg-query >/dev/null ||
        fail 'Linux UI tools are missing and automatic provisioning requires dpkg/apt-get'
    command -v apt-get >/dev/null ||
        fail 'Linux UI tools are missing and automatic provisioning requires dpkg/apt-get'

    local package_name
    local -a missing_packages=()
    for package_name in xvfb xauth x11-utils x11-apps libxkbcommon-x11-0; do
        if ! dpkg-query -W -f='${db:Status-Abbrev}' "$package_name" 2>/dev/null |
            grep -qx 'ii '; then
            missing_packages+=("$package_name")
        fi
    done
    if ! command -v python3 >/dev/null; then
        missing_packages+=(python3)
    fi

    if ((${#missing_packages[@]} > 0)); then
        local -a apt_command=(apt-get)
        if ((EUID != 0)); then
            command -v sudo >/dev/null ||
                fail "root access is required to install: ${missing_packages[*]}"
            apt_command=(sudo apt-get)
        fi
        echo "main-pr-reproduction: installing missing Linux UI packages: ${missing_packages[*]}"
        "${apt_command[@]}" update
        "${apt_command[@]}" install --no-install-recommends --yes "${missing_packages[@]}"
    fi

    linux_ui_tools_available ||
        fail 'Linux UI dependencies remain unavailable after host provisioning'
}

phase='all'
pr_number=''
while (($# > 0)); do
    case "$1" in
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

source_sha="$(git rev-parse HEAD)"

ensure_native_quality_host_dependencies() {
    command -v cargo >/dev/null || fail 'cargo is unavailable'
    command -v rustup >/dev/null || fail 'rustup is unavailable'
    if ! rustup component list --installed | grep -Eq '^llvm-tools-preview(-|$)'; then
        rustup component add llvm-tools-preview
    fi
    if ! cargo llvm-cov --version 2>/dev/null | grep -Eq '^cargo-llvm-cov 0\.9\.0([[:space:]]|$)'; then
        cargo install cargo-llvm-cov --version 0.9.0 --locked
    fi
    ensure_linux_ui_host_dependencies
}

run_native_quality() {
    ensure_native_quality_host_dependencies
    mkdir -p artifacts/codacy-coverage-rust
    cargo llvm-cov --workspace --locked --all-targets --cobertura \
        --output-path artifacts/codacy-coverage-rust/rust.cobertura.xml \
        -- --nocapture
    local report='artifacts/codacy-coverage-rust/rust.cobertura.xml'
    test -s "$report"
    grep -Eq 'lines-valid="[1-9][0-9]*"' "$report"
    if grep -Eq 'filename="(/|[A-Za-z]:[\\/])' "$report"; then
        fail 'Rust Cobertura contains a runner-specific path'
    fi
    cargo clippy --workspace --locked --all-targets -- -D warnings
    cargo build --workspace --release --locked
    bash scripts/check_recorder_rest_boundary.sh --static-only
    bash scripts/cli_contract_e2e.sh
    xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
        bash scripts/record_daemon_e2e.sh
}

run_linux_ui() {
    command -v cargo >/dev/null || fail 'cargo is unavailable'
    ensure_linux_ui_host_dependencies
    cargo build --release --locked \
        -p codex_info -p codex-info-recorder -p codex-info-rest
    xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
        bash scripts/x11_startup_visual_gate.sh
    xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
        bash scripts/x11_graph_visual_gate.sh
}

run_linux_distribution() {
    command -v cargo >/dev/null || fail 'cargo is unavailable'
    ensure_linux_ui_host_dependencies
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
        -File "$runner" -SourceSha "$source_sha" -PrepareCandidate
}

show_external_checks() {
    [[ "$pr_number" =~ ^[1-9][0-9]*$ ]] || fail '--pr is required for external-checks'
    command -v gh >/dev/null || fail 'gh is unavailable'
    gh pr checks "$pr_number" --repo salty919/codex_info_v2
}

case "$phase" in
    native-quality) run_native_quality ;;
    linux-ui) run_linux_ui ;;
    linux-distribution) run_linux_distribution ;;
    windows-ui) run_windows_ui ;;
    external-checks) show_external_checks ;;
    all)
        run_native_quality
        run_linux_ui
        run_linux_distribution
        run_windows_ui
        if [[ -n "$pr_number" ]]; then show_external_checks; fi
        ;;
    *) fail "unknown phase: $phase" ;;
esac

echo "main-pr-reproduction: PASS phase=$phase source_sha=$source_sha"

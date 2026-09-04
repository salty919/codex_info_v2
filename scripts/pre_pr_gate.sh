#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "pre-pr-gate: FAIL: $*" >&2
    exit 1
}

base_revision=''
quality_profile=''
requested_args=()
while (($# > 0)); do
    case "$1" in
        --base)
            [[ $# -ge 2 && -z "$base_revision" ]] ||
                fail '--base requires one value and may appear only once'
            base_revision="$2"
            shift 2
            ;;
        --requested-check)
            [[ $# -ge 2 ]] || fail '--requested-check requires one check ID'
            requested_args+=(--requested-check "$2")
            shift 2
            ;;
        --quality-profile)
            [[ $# -ge 2 && -z "$quality_profile" ]] ||
                fail '--quality-profile requires one value and may appear only once'
            quality_profile="$2"
            shift 2
            ;;
        *) fail "unknown argument: $1" ;;
    esac
done

if [[ -n "$base_revision" ]]; then
    git rev-parse --verify "${base_revision}^{commit}" >/dev/null 2>&1 ||
        fail "base is not a commit: $base_revision"
fi

mapfile -d '' changed_paths < <({
    if [[ -n "$base_revision" ]]; then
        git -c core.quotePath=false diff --no-renames --name-only -z \
            "$base_revision" HEAD
    fi
    git -c core.quotePath=false diff --no-renames --name-only -z
    git -c core.quotePath=false diff --cached --no-renames --name-only -z
    git -c core.quotePath=false ls-files --others --exclude-standard -z |
        while IFS= read -r -d '' path; do
            [[ "$path" == */__pycache__/*.pyc ]] || printf '%s\0' "$path"
        done
} | sort -zu)

((${#changed_paths[@]} > 0)) || fail 'no changed paths to validate'

plan_args=()
for path in "${changed_paths[@]}"; do
    plan_args+=(--path "$path")
done
profile_args=()
[[ -z "$quality_profile" ]] || profile_args+=(--quality-profile "$quality_profile")
plan_json="$(python3 scripts/quality_plan.py \
    "${plan_args[@]}" "${profile_args[@]}" "${requested_args[@]}")" || exit $?
printf 'pre-pr-gate: plan %s\n' "$plan_json"

mapfile -t checks < <(
    python3 -c 'import json,sys; print("\n".join(json.load(sys.stdin)["checks"]))' \
        <<<"$plan_json"
)
((${#checks[@]} > 0)) || fail 'quality plan contains no checks'

run_governance_contract() {
    local path run_authority_fixtures=0 run_selector_fixtures=0 run_workflow_fixtures=0
    for path in "${changed_paths[@]}"; do
        if [[ "$path" == *.sh && -f "$path" ]]; then
            bash -n "$path"
        elif [[ "$path" == *.py && -f "$path" ]]; then
            python3 - "$path" <<'PY'
import ast
from pathlib import Path
import sys

path = Path(sys.argv[1])
ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
PY
        fi
        case "$path" in
            scripts/requirements_authority.py|scripts/test_requirements_authority.py|scripts/requirements_ledger_gate.sh)
                run_authority_fixtures=1
                ;;
            scripts/quality_plan.py|scripts/test_quality_plan.py|scripts/ci_change_scope.py|scripts/test_ci_change_scope.py|scripts/selected_quality_gate.py|scripts/test_selected_quality_gate.py|scripts/pre_pr_gate.sh)
                run_selector_fixtures=1
                ;;
            .github/workflows/*|scripts/workflow_quality_gate.py)
                run_workflow_fixtures=1
                ;;
        esac
    done

    ((run_authority_fixtures == 0)) || python3 scripts/test_requirements_authority.py
    if ((run_selector_fixtures != 0)); then
        python3 scripts/test_quality_plan.py
        python3 scripts/test_ci_change_scope.py
        python3 scripts/test_selected_quality_gate.py
    fi
    if ((run_workflow_fixtures != 0)); then
        python3 scripts/workflow_quality_gate.py \
            --self-test --profile workflow-selection
        python3 scripts/test_codeql_workflow.py
    fi
}

for check in "${checks[@]}"; do
    printf 'pre-pr-gate: run check=%s\n' "$check"
    case "$check" in
        requirements-authority)
            bash scripts/requirements_ledger_gate.sh
            ;;
        governance-contract)
            run_governance_contract
            ;;
        rust-format)
            bash scripts/regression_guard.sh --format
            ;;
        rust-test)
            bash scripts/regression_guard.sh --test
            ;;
        windows-contract)
            bash scripts/windows_client_contract_gate.sh
            ;;
        rust-history-graph)
            bash scripts/regression_guard.sh --history-graph
            ;;
        linux-ui-history-graph)
            cargo build --release --locked
            xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
                bash scripts/x11_graph_visual_gate.sh
            ;;
        windows-history-graph)
            bash scripts/windows_client_contract_gate.sh --history-graph
            ;;
        governance-workflow-selection)
            run_governance_contract
            ;;
        *)
            fail "quality plan returned an unimplemented check: $check"
            ;;
    esac
done

echo 'pre-pr-gate: PASS'

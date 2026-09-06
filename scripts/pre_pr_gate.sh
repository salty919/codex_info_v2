#!/usr/bin/env bash
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "pre-pr-gate: FAIL: $*" >&2
    exit 1
}

base_revision=''
requested_args=()
requested_checks=()
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
            requested_checks+=("$2")
            shift 2
            ;;
        *) fail "unknown argument: $1" ;;
    esac
done

if [[ -n "$base_revision" ]]; then
    git rev-parse --verify "${base_revision}^{commit}" >/dev/null 2>&1 ||
        fail "base is not a commit: $base_revision"
    git merge-base "$base_revision" HEAD >/dev/null 2>&1 ||
        fail "base and HEAD have no common ancestor: $base_revision"
fi

mapfile -d '' changed_paths < <({
    if [[ -n "$base_revision" ]]; then
        git -c core.quotePath=false diff --no-renames --name-only -z \
            "$base_revision...HEAD"
    fi
    git -c core.quotePath=false diff --no-renames --name-only -z
    git -c core.quotePath=false diff --cached --no-renames --name-only -z
    git -c core.quotePath=false ls-files --others --exclude-standard -z
} | sort -zu)

((${#changed_paths[@]} > 0)) || fail 'no changed paths to validate'

plan_args=()
for path in "${changed_paths[@]}"; do
    plan_args+=(--path "$path")
done
plan_json="$(python3 scripts/quality_plan.py \
    "${plan_args[@]}" "${requested_args[@]}")" || exit $?
printf 'pre-pr-gate: plan %s\n' "$plan_json"

if ((${#requested_checks[@]} > 0)); then
    checks=("${requested_checks[@]}")
else
    mapfile -t checks < <(
        python3 -c 'import json,sys; print("\n".join(json.load(sys.stdin)["checks"]))' \
            <<<"$plan_json"
    )
fi
((${#checks[@]} > 0)) || fail 'quality plan contains no checks'

run_governance_contract() {
    local path run_authority_fixtures=0 run_quality_plan_fixture=0 run_scope_fixture=0
    local run_product_version_fixture=0
    local run_workflow_selection_fixture=0
    local run_release_authority_fixture=0 run_publisher_fixture=0 run_pr_resolver_fixture=0
    local -a governance_paths=()
    mapfile -t governance_paths < <(
        python3 - "${changed_paths[@]}" <<'PY'
import sys

sys.path.insert(0, "scripts")
from ci_change_scope import selection_for_paths

for path in sys.argv[1:]:
    if "GOVERNANCE" in selection_for_paths([path]).owners:
        print(path)
PY
    )
    for path in "${governance_paths[@]}"; do
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
            scripts/quality_plan.py|scripts/test_quality_plan.py|scripts/pre_pr_gate.sh)
                run_quality_plan_fixture=1
                ;;
            scripts/ci_change_scope.py|scripts/test_ci_change_scope.py)
                run_scope_fixture=1
                ;;
            scripts/product_version.py|scripts/test_product_version.py)
                run_product_version_fixture=1
                ;;
            .github/workflows/selective-quality.yml|scripts/test_workflow_selection.py)
                run_workflow_selection_fixture=1
                ;;
        esac
        case "$path" in
            .github/workflows/feat-integration.yml|.github/workflows/main-quality.yml|scripts/resolve_pr_quality.py|scripts/test_resolve_pr_quality.py)
                run_pr_resolver_fixture=1
                ;;
            .github/workflows/release.yml|scripts/release_authority.py|scripts/test_release_authority.py)
                run_release_authority_fixture=1
                ;;
            .github/workflows/release.yml|scripts/publish_release.py|scripts/test_publish_release.py)
                run_publisher_fixture=1
                ;;
        esac
    done

    ((run_authority_fixtures == 0)) || python3 scripts/test_requirements_authority.py
    ((run_quality_plan_fixture == 0)) || python3 scripts/test_quality_plan.py
    ((run_scope_fixture == 0)) || python3 scripts/test_ci_change_scope.py
    ((run_product_version_fixture == 0)) || python3 scripts/test_product_version.py
    ((run_workflow_selection_fixture == 0)) || python3 scripts/test_workflow_selection.py
    ((run_pr_resolver_fixture == 0)) || python3 scripts/test_resolve_pr_quality.py
    ((run_release_authority_fixture == 0)) || python3 scripts/test_release_authority.py
    ((run_publisher_fixture == 0)) || python3 scripts/test_publish_release.py
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
        linux-ui-contract)
            command -v xvfb-run >/dev/null 2>&1 || fail 'xvfb-run is unavailable'
            cargo build --release --locked
            xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
                bash scripts/x11_graph_visual_gate.sh
            ;;
        windows-contract)
            powershell_paths=()
            for path in "${changed_paths[@]}"; do
                [[ "$path" == *.ps1 && -f "$path" ]] && powershell_paths+=("$path")
            done
            powershell_paths_json="$(python3 -c \
                'import json,sys; print(json.dumps(sys.argv[1:], separators=(",", ":")))' \
                "${powershell_paths[@]}")"
            POWERSHELL_PATHS_JSON="$powershell_paths_json" \
                bash scripts/windows_client_contract_gate.sh
            ;;
        *)
            fail "quality plan returned an unimplemented check: $check"
            ;;
    esac
done

echo 'pre-pr-gate: PASS'

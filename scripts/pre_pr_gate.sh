#!/usr/bin/env bash
set -euo pipefail

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
fi

# Complete the reads before exposing any paths to the plan. Process
# substitution would otherwise hide a failed later diff behind earlier output.
changed_paths_json="$(python3 - "$base_revision" <<'PY'
import json
import subprocess
import sys

sys.path.insert(0, "scripts")
from ci_change_scope import paths_from_name_status

diff = ["git", "-c", "core.quotePath=false", "diff", "--name-status", "-z",
        "--find-renames=50%", "--find-copies=50%", "--find-copies-harder"]
revisions = [[sys.argv[1], "HEAD"]] if sys.argv[1] else []
paths = set()
try:
    for revision in [*revisions, [], ["--cached"]]:
        raw = subprocess.check_output([*diff, *revision, "--"])
        if raw:
            paths.update(paths_from_name_status(raw))
    raw = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard", "-z"])
    for path in raw.decode("utf-8").split("\0"):
        if path and not ("/__pycache__/" in path and path.endswith(".pyc")):
            paths.add(path)
except (OSError, ValueError, subprocess.CalledProcessError) as error:
    raise SystemExit(f"pre-pr-gate: FAIL: cannot read complete change set: {error}") from error
print(json.dumps(sorted(paths)))
PY
)" || exit $?
mapfile -d '' changed_paths < <(
    python3 -c 'import json,sys; sys.stdout.buffer.write(b"".join(p.encode()+b"\0" for p in json.loads(sys.argv[1])))' \
        "$changed_paths_json"
)

((${#changed_paths[@]} > 0)) || fail 'no changed paths to validate'

plan_args=()
for path in "${changed_paths[@]}"; do
    plan_args+=(--path "$path")
done
plan_json="$(python3 scripts/quality_plan.py \
    "${plan_args[@]}" "${requested_args[@]}")" || exit $?
printf 'pre-pr-gate: plan %s\n' "$plan_json"

mapfile -t planned_checks < <(
    python3 -c 'import json,sys; print("\n".join(json.load(sys.stdin)["checks"]))' \
        <<<"$plan_json"
)
if ((${#requested_checks[@]} > 0)); then
    checks=("${requested_checks[@]}")
else
    checks=("${planned_checks[@]}")
fi
((${#checks[@]} > 0)) || fail 'quality plan contains no checks'

run_governance_contract() {
    local path run_authority_fixtures=0 run_selector_fixtures=0 run_regression_guard_fixture=0 run_workflow_fixtures=0 run_codeql_fixture=0 run_codacy_coverage_fixture=0
    local run_python_quality_fixtures=0
    local run_windows_current_fixtures=0
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
            scripts/windows_client_contract_gate.sh|scripts/test_windows_current_contract_gate.py)
                run_windows_current_fixtures=1
                ;;
        esac
        case "$path" in
            scripts/requirements_authority.py|scripts/test_requirements_authority.py|scripts/requirements_ledger_gate.sh)
                run_authority_fixtures=1
                ;;
            scripts/quality_plan.py|scripts/test_quality_plan.py|scripts/ci_change_scope.py|scripts/test_ci_change_scope.py|scripts/selected_quality_gate.py|scripts/test_selected_quality_gate.py|scripts/pre_pr_gate.sh)
                run_selector_fixtures=1
                ;;
            scripts/regression_guard.sh|scripts/test_regression_guard.py)
                run_regression_guard_fixture=1
                ;;
            .github/workflows/*|.github/scripts/*|.github/tests/*|scripts/workflow_quality_gate.py)
                run_workflow_fixtures=1
                ;;
        esac
        case "$path" in
            scripts/pre_pr_gate.sh|scripts/python_quality_gate.sh|scripts/python_quality.toml|scripts/test_pre_pr_python_quality.py|.github/requirements-python-quality.txt|.github/workflows/selective-quality.yml)
                run_python_quality_fixtures=1
                ;;
        esac
        if [[ "$path" == .github/workflows/codeql.yml || "$path" == scripts/test_codeql_workflow.py ]]; then
            run_codeql_fixture=1
        fi
        case "$path" in
            .github/workflows/rust.yml|.github/workflows/windows-client.yml|.github/workflows/codacy-coverage.yml|windows-client/CodeCoverage.runsettings|scripts/codacy_coverage_artifacts.py|scripts/test_codacy_coverage_artifacts.py|scripts/test_codacy_coverage_workflow.py)
                run_codacy_coverage_fixture=1
                ;;
        esac
    done

    bash scripts/python_quality_gate.sh "${changed_paths[@]}"
    if ((run_windows_current_fixtures != 0)); then
        python3 scripts/test_windows_current_contract_gate.py
    fi
    if ((run_python_quality_fixtures != 0)); then
        python3 scripts/test_pre_pr_python_quality.py
    fi
    ((run_authority_fixtures == 0)) || python3 scripts/test_requirements_authority.py
    if ((run_selector_fixtures != 0)); then
        python3 scripts/test_quality_plan.py
        python3 scripts/test_ci_change_scope.py
        python3 scripts/test_selected_quality_gate.py
    fi
    if ((run_regression_guard_fixture != 0)); then
        python3 scripts/test_regression_guard.py
    fi
    if ((run_workflow_fixtures != 0)); then
        python3 scripts/workflow_quality_gate.py \
            --owner-selection-self-test
    fi
    if ((run_codeql_fixture != 0)); then
        python3 scripts/test_codeql_workflow.py
    fi
    if ((run_codacy_coverage_fixture != 0)); then
        python3 scripts/test_codacy_coverage_artifacts.py
        python3 scripts/test_codacy_coverage_workflow.py
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
        linux-ui-contract)
            command -v xvfb-run >/dev/null 2>&1 || fail 'xvfb-run is unavailable'
            cargo build --release --locked
            xvfb-run --auto-servernum --server-args='-screen 0 1280x800x24' \
                bash scripts/x11_graph_visual_gate.sh
            ;;
        windows-contract)
            bash scripts/windows_client_contract_gate.sh
            ;;
        *)
            fail "quality plan returned an unimplemented check: $check"
            ;;
    esac
done

pending_checks=()
for planned in "${planned_checks[@]}"; do
    found=0
    for executed in "${checks[@]}"; do
        [[ "$planned" != "$executed" ]] || found=1
    done
    ((found != 0)) || pending_checks+=("$planned")
done
if ((${#pending_checks[@]} > 0)); then
    echo "pre-pr-gate: PARTIAL executed=${checks[*]} not-executed=${pending_checks[*]}"
else
    echo "pre-pr-gate: PASS checks=${checks[*]}"
fi

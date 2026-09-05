#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "windows-client-contract-gate: FAIL: $*" >&2
    exit 1
}

profile='full'
if [[ $# -eq 1 && "$1" == --history-graph ]]; then
    profile='history-graph'
elif [[ $# -eq 1 && "$1" == --model-history ]]; then
    profile='model-history'
elif [[ $# -ne 0 ]]; then
    fail "unexpected argument: $1"
fi
command -v dotnet >/dev/null 2>&1 || fail 'dotnet is unavailable'

solution='windows-client/CodexInfo.WindowsClient.sln'
test_target="$solution"
test_filter=()
expected_methods=()
if [[ "$profile" == history-graph || "$profile" == model-history ]]; then
    test_target='windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/CodexInfo.WindowsClient.Presentation.Tests.csproj'
    if [[ "$profile" == history-graph ]]; then
        expected_methods=(
        CodexInfo.WindowsClient.Presentation.Tests.GraphWindowViewModelProjectionTests.Clips_current_graph_period_at_start_and_reset_boundaries
        CodexInfo.WindowsClient.Presentation.Tests.GraphWindowViewModelProjectionTests.Keeps_historical_graph_period_boundary_intact
        CodexInfo.WindowsClient.Presentation.Tests.GraphWindowViewModelProjectionTests.Graph_samples_start_at_first_observation_without_synthetic_anchor
        CodexInfo.WindowsClient.Presentation.Tests.GraphWindowViewModelProjectionTests.Graph_samples_are_empty_when_period_has_no_history
        CodexInfo.WindowsClient.Presentation.Tests.GraphWindowViewModelProjectionTests.Graph_samples_do_not_fabricate_quota_when_quota_observations_are_missing
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Shared_graph_fixture_matches_the_native_history_oracle_through_details_http_parser
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Shared_rollover_fixture_atomically_refreshes_open_main_graph_and_threads_from_details
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Live_incident_regression_recovery_is_never_connected_as_solid
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Issue137_cumulative_correction_fixture_never_paints_a_solid_recovery_bridge
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Confirmed_history_gap_ends_both_subpaths_without_a_cross_gap_connector
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Remaining_quota_observations_survive_flat_model_rows_as_unattributed_dashes
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Missing_remote_quota_is_never_painted_as_a_solid_bridge
        CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.Reduction_preserves_regression_quota_and_confirmed_gap_boundaries
        )
    else
        expected_methods=(
            CodexInfo.WindowsClient.Core.Tests.LoopbackStatusClientTests.DetailsV3IsPreferredAndCarriesAstraHistory
            CodexInfo.WindowsClient.Core.Tests.LoopbackStatusClientTests.DetailsV3ReusesTheAcceptedGenerationWithAZeroBody304
            CodexInfo.WindowsClient.Core.Tests.LoopbackStatusClientTests.DetailsFallsBackToV1OnlyWhenV3AndV2ReturnNotFound
            CodexInfo.WindowsClient.Presentation.Tests.GraphPlotControlTests.V3AstraHistoryRendersWithoutLegacyModelRows
        )
    fi
    filter=''
    for method in "${expected_methods[@]}"; do
        filter+="${filter:+|}FullyQualifiedName=$method"
    done
    test_filter=(--filter "$filter")
fi
results_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-info-windows-tests.XXXXXX")"
case "$results_dir" in
    "${TMPDIR:-/tmp}"/codex-info-windows-tests.*) ;;
    *) fail "unsafe temporary result path: $results_dir" ;;
esac
trap 'rm -rf -- "$results_dir"' EXIT

# Restore, formatting, and unit behavior form one Windows-source check.  The
# gate owns each command once and deliberately does not mirror test names,
# source strings, coverage percentages, installer, or real-OS E2E contracts.
dotnet restore "$test_target" --locked-mode
if [[ "$profile" == full ]]; then
    dotnet format "$solution" --no-restore --verify-no-changes
fi
dotnet test "$test_target" \
    --no-restore \
    --configuration Release \
    "${test_filter[@]}" \
    --results-directory "$results_dir" \
    --logger 'trx;LogFilePrefix=windows-client'

python3 - "$results_dir" "$profile" "${expected_methods[@]}" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

reports = sorted(Path(sys.argv[1]).rglob("*.trx"))
profile = sys.argv[2]
expected_methods = set(sys.argv[3:])
if not reports:
    raise SystemExit("windows-client-contract-gate: FAIL: TRX report is missing")

totals = {name: 0 for name in ("total", "executed", "passed", "failed", "notExecuted")}
observed_methods = set()
for report in reports:
    root = ET.parse(report).getroot()
    counters = [element for element in root.iter() if element.tag.endswith("Counters")]
    if not counters:
        raise SystemExit(
            f"windows-client-contract-gate: FAIL: TRX counters are missing: {report}"
        )
    values = counters[-1].attrib
    for name in totals:
        try:
            totals[name] += int(values.get(name, "0"))
        except ValueError as exc:
            raise SystemExit(
                f"windows-client-contract-gate: FAIL: malformed {name} counter: {report}"
            ) from exc
    for element in root.iter():
        if element.tag.endswith("TestMethod"):
            class_name = element.attrib.get("className")
            method_name = element.attrib.get("name")
            if class_name and method_name:
                observed_methods.add(f"{class_name}.{method_name}")

# A positive passing observation is required.  Skipped/not-executed tests are
# reported but are not converted into failures merely to satisfy a count.
if totals["total"] <= 0 or totals["executed"] <= 0 or totals["passed"] <= 0:
    raise SystemExit("windows-client-contract-gate: FAIL: zero Windows tests executed")
if totals["failed"] != 0:
    raise SystemExit("windows-client-contract-gate: FAIL: Windows test failure recorded")
if profile in {"history-graph", "model-history"}:
    missing = sorted(expected_methods - observed_methods)
    unexpected = sorted(observed_methods - expected_methods)
    if missing or unexpected:
        raise SystemExit(
            "windows-client-contract-gate: FAIL: focused method set mismatch "
            f"missing={missing} unexpected={unexpected}"
        )
print(
    "windows-client-contract-gate: evidence "
    + " ".join(f"{name}={value}" for name, value in totals.items())
)
PY

if [[ "$profile" == history-graph ]]; then
    echo "windows-client-contract-gate: PASS check=windows-history-graph methods=${#expected_methods[@]}"
elif [[ "$profile" == model-history ]]; then
    echo "windows-client-contract-gate: PASS check=windows-model-history methods=${#expected_methods[@]}"
else
    echo 'windows-client-contract-gate: PASS check=windows-contract'
fi

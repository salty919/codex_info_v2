#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() { echo "regression-guard: FAIL: $*" >&2; exit 1; }
require_text() { rg -q --fixed-strings -- "$2" "$1" || fail "missing $1: $2"; }
require_file() { [[ -f "$1" ]] || fail "missing file: $1"; }

run_checked() {
    local description="$1"
    shift
    local output
    output="$("$@" 2>&1)" || {
        printf '%s\n' "$output" >&2
        fail "$description failed"
    }
    printf '%s\n' "$output"
}

# Check both the working tree (local delivery) and the current commit against
# its parent (CI delivery). A clean checkout otherwise makes `git diff --check`
# vacuous and allows whitespace errors in the commit itself to pass.
run_checked 'working-tree whitespace check' git diff --check
run_checked 'index whitespace check' git diff --cached --check
if git rev-parse --verify HEAD^ >/dev/null 2>&1; then
    run_checked 'committed whitespace check' git diff --check HEAD^ HEAD
fi
run_checked 'Rust format check' cargo fmt --check
run_checked 'Rust all-target check' cargo check --locked --all-targets
run_checked 'Requirements ledger schema check' bash scripts/requirements_ledger_gate.sh
run_checked 'Requirements ledger final check' bash scripts/requirements_ledger_gate.sh --final

require_text docs/PRODUCT_REQUIREMENTS.md '全直積、N倍、N二乗、N階乗のcase生成を行わない'
require_text docs/PRODUCT_REQUIREMENTS.md '製品バージョンはメイン画面に一度だけ表示し'
require_text docs/REGRESSION_PREVENTION_POLICY.md 'REG-WIN-DRAG'
require_text docs/REGRESSION_PREVENTION_POLICY.md 'X先行の変更凍結を必須とする'
require_text docs/REGRESSION_PREVENTION_POLICY.md 'windows_window_move_smoke.ps1 -AllowPhysicalInput'
for required_ledger_id in X-START-01 X-START-02 X-START-03 X-GRAPH-01 X-THREAD-01 WIN-START-01 WIN-GRAPH-01 WIN-VERSION-01 PROC-LEDGER-01; do
    require_text docs/REQUIREMENTS_LEDGER.md "| $required_ledger_id |"
done
require_file scripts/x11_graph_visual_gate.sh
require_file scripts/x11_startup_visual_gate.sh
require_text scripts/x11_graph_visual_gate.sh 'lib.XMoveWindow(display, main, 1280, 0)'
require_text scripts/x11_graph_visual_gate.sh 'lib.XLowerWindow(display, main)'
if rg -q --fixed-strings 'XUnmapWindow' scripts/x11_graph_visual_gate.sh; then
    fail 'X11 graph gate must not unmap the shared Slint owner before first-frame acceptance'
fi
require_file scripts/cli_contract_e2e.sh
require_text scripts/cli_contract_e2e.sh "initial_commit='codex-info: recorder committed 1 samples'"
require_text scripts/cli_contract_e2e.sh "fixture_now=\"\$(date -u +%s)\""
require_text scripts/cli_contract_e2e.sh '"CODEX_INFO_DAEMON_INTERVAL_SECS=5"'
require_text scripts/cli_contract_e2e.sh 'BEGIN EXCLUSIVE;'
require_text scripts/cli_contract_e2e.sh "'forced transient recorder failure was not observed'"
require_text scripts/cli_contract_e2e.sh "sqlite3 -batch -bail -cmd '.timeout 2000'"
require_text scripts/x11_graph_visual_gate.sh 'graph child window title redundantly exposes product version'
require_text scripts/windows_client_contract_gate.sh 'main version automation marker must appear exactly once'
require_text scripts/windows_client_contract_gate.sh 'child window must not render a product version'
require_text scripts/windows_client_contract_gate.sh "--logger 'trx;LogFilePrefix=windows-client'"
require_text scripts/windows_client_contract_gate.sh 'TRX counters are missing'
require_text scripts/windows_client_contract_gate.sh 'notExecuted'
require_text windows-client/src/CodexInfo.WindowsClient/WindowDragBehavior.cs 'window.BeginMoveDrag(eventArgs)'
require_text windows-client/src/CodexInfo.WindowsClient/ViewModels/DetailsWindowViewModels.cs 'EffectiveGraphEnd'
require_text windows-client/src/CodexInfo.WindowsClient/Graphing/GraphPlotProjection.cs 'IsSyntheticFirstObservation'
require_text windows-client/src/CodexInfo.WindowsClient/Settings/ConnectionProcessFactory.cs 'codex_info'
require_text windows-client/src/CodexInfo.WindowsClient/Settings/ConnectionProcessFactory.cs '--port'
require_text windows-client/tools/Run-WindowsClientE2E.ps1 'main-details-status: PASS (matching status/details generation accepted)'
require_text windows-client/tools/Run-WindowsClientE2E.ps1 'main-startup-loading: PASS (first complete generation is visible)'
require_text windows-client/tools/Run-WindowsClientE2E.ps1 'legal-plain-text: PASS (all 9 rendered notices, Back, Minimize, and Close are usable)'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/LegalNoticeCatalogTests.cs 'EveryPackagedMarkdownUrlAndCodeBodySurvivesProjection'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/LegalNoticeCatalogTests.cs 'FencedCodePreservesHtmlCommentDelimitersWhileOutsideProjectionRemovesThem'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/LegalNoticeCatalogTests.cs 'MalformedInjectedMarkdownUsesTheExistingFailClosedLoadPage'
require_text windows-client/tools/Run-WindowsClientE2E.ps1 'Assert-E2EGraphModelPixels'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs 'PlotProjectionUsesFlatVerticalAndContiguousSegmentsForFirstObservation'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/GraphPlotControlTests.cs 'IdleBandsUseTheDedicatedVisibleNeutralColor'
require_text src/main.rs 'remaining_graph_does_not_infer_quota_loss_from_model_spend'
require_text src/main.rs 'native_startup_loading_requires_a_complete_authenticated_generation'
require_text src/main.rs 'native_startup_failure_releases_loading_surface'
require_text src/main.rs '"startup-loading"'
require_text ui/app.slint 'startup-loading: false'
require_text ui/app.slint 'text: "◌  " + root.strings.checking;'
require_text windows-client/src/CodexInfo.WindowsClient/Settings/ClientSettings.cs 'ConnectionConfigured'
require_text windows-client/src/CodexInfo.WindowsClient/ViewModels/SetupFlow.cs 'SetupLaunchPolicy'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/WindowDragGeometryTests.cs 'SetupLaunchPolicy.ShouldOpen'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/MainWindowViewModelTests.cs 'Startup_keeps_content_hidden_until_the_first_snapshot_is_complete'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/MainWindowViewModelTests.cs 'Startup_failure_releases_spinner_and_exposes_retry_state'
require_text windows-client/tests/CodexInfo.WindowsClient.Presentation.Tests/MainWindowViewModelTests.cs 'Startup_waits_for_the_matching_details_generation_before_publishing_content'

run_required_rust_test() {
    local test_name="$1"
    local output
    output="$(cargo test --locked --bin codex_info "tests::$test_name" -- --exact --nocapture 2>&1)" || {
        printf '%s\n' "$output" >&2
        fail "required Rust regression test failed: $test_name"
    }
    printf '%s\n' "$output"
    if ! rg -q --fixed-strings "test tests::$test_name ... ok" <<<"$output"; then
        fail "required Rust regression test did not run and pass exactly: $test_name"
    fi
    if ! rg -q --fixed-strings 'running 1 test' <<<"$output"; then
        fail "required Rust regression test executed an unexpected test count: $test_name"
    fi
}

run_required_rust_lib_test() {
    local test_name="$1"
    local output
    output="$(cargo test --locked --lib "thread_contract::tests::$test_name" -- --exact --nocapture 2>&1)" || {
        printf '%s\n' "$output" >&2
        fail "required Rust lib regression test failed: $test_name"
    }
    printf '%s\n' "$output"
    if ! rg -q --fixed-strings "test thread_contract::tests::$test_name ... ok" <<<"$output"; then
        fail "required Rust lib regression test did not run and pass exactly: $test_name"
    fi
    if ! rg -q --fixed-strings 'running 1 test' <<<"$output"; then
        fail "required Rust lib regression test executed an unexpected test count: $test_name"
    fi
}

all_target_output="$(cargo test --locked --all-targets -- --nocapture 2>&1)" || {
    printf '%s\n' "$all_target_output" >&2
    fail 'Rust all-target tests failed'
}
printf '%s\n' "$all_target_output"
if ! rg -q 'running [1-9][0-9]* tests?' <<<"$all_target_output"; then
    fail 'Rust all-target tests executed zero tests'
fi
if rg -q '^running 0 tests?$' <<<"$all_target_output"; then
    fail 'Rust all-target test set contains a zero-test target'
fi
run_checked 'Rust release build' cargo build --release --locked
run_checked 'Public CLI lifecycle E2E' bash scripts/cli_contract_e2e.sh

# This guard is executable evidence, not only a source-policy scan.  A future
# caller cannot obtain PASS by omitting the history/graph tests or by selecting
# a filter that matches zero tests.
run_required_rust_test historical_week_fixture_preserves_each_period_and_graph_samples
run_required_rust_test observed_moving_reset_sequence_keeps_the_spend_in_the_selected_graph
run_required_rust_test long_rolling_reset_sequence_stays_in_one_period_after_a_real_boundary
run_required_rust_test quota_only_reset_fragments_stay_with_the_adjacent_spend_period
run_required_rust_test live_rolling_quota_chain_does_not_expose_an_empty_past_period
run_required_rust_test affected_period_keeps_sol_spend_and_unobserved_quota_distinct
run_required_rust_test shared_graph_fixture_is_the_x_history_oracle
run_required_rust_test model_graph_does_not_invent_spend_during_an_unobserved_gap
run_required_rust_test unused_intervals_mark_long_gap_before_observed_spend
run_required_rust_test graph_controls_use_one_visual_boundary_and_show_short_histories
run_required_rust_test remaining_graph_does_not_infer_quota_loss_from_model_spend
run_required_rust_test affected_timestamp_does_not_mix_a_singleton_reset_period_into_history
run_required_rust_test ambiguous_missing_quota_row_at_a_spend_timestamp_is_not_a_period
run_required_rust_test singleton_reset_snapshot_overlapping_a_spend_period_stays_separate
run_required_rust_test graph_collision_preview_matches_the_historical_singleton_oracle
run_required_rust_test moving_reset_collision_at_30_and_60_seconds_fails_closed
run_required_rust_test record_rejects_alias_quota_collision_before_canonical_merge
run_required_rust_test same_timestamp_reset_drift_above_jitter_fails_closed
run_required_rust_test startup_load_sanitizes_legacy_same_timestamp_quota_collision
run_required_rust_test canonical_samples_merge_missing_and_observed_quota_for_reset_jitter
run_required_rust_test canonical_samples_keep_conflicting_same_timestamp_quota_missing
run_required_rust_test canonical_samples_have_unique_reset_timestamp_keys
run_required_rust_test periodic_quota_refresh_retains_last_good_main_snapshot
run_required_rust_test product_version_is_visible_once_on_native_main_surface
run_required_rust_test public_snapshot_is_whitelisted_and_tracks_auth_state
run_required_rust_lib_test recoverable_rollout_parser_skips_only_malformed_token_count_records
for required_thread_failure_test in \
    thread_c_all_current_cycle_failure_classes_return_no_partial_snapshot \
    thread_c_candidate_failure_rejects_the_complete_cycle \
    thread_c_known_token_invalid_event_rejects_entire_rollout \
    thread_c_no_thread_and_all_candidate_failure_are_distinct \
    thread_c_private_accumulator_abort_never_yields_partial_snapshot \
    thread_c_snapshot_rejects_partial_candidate_reads; do
    run_required_rust_lib_test "$required_thread_failure_test"
done

# When a local X11 display is available, require a fresh rendered graph image
# as part of the same delivery check. Headless runners cannot satisfy this
# visual requirement and therefore do not claim X11 image PASS.
if [[ -n "${DISPLAY:-}" ]]; then
    run_checked 'X11 graph visual gate' bash scripts/x11_graph_visual_gate.sh
    run_checked 'X11 startup visual gate' bash scripts/x11_startup_visual_gate.sh
else
    fail 'X11 graph visual gate unverified (DISPLAY unavailable)'
fi

if rg -q 'SetCursorPos|mouse_event|SendInput' windows-client/src; then
    fail 'product source contains physical cursor or synthetic mouse API'
fi

echo 'regression-guard: PASS'

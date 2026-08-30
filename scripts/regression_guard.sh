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

require_text docs/PRODUCT_REQUIREMENTS.md '全直積、N倍、N二乗、N階乗のcase生成を行わない'
require_text docs/PRODUCT_REQUIREMENTS.md '製品バージョンはメイン画面に一度だけ表示し'
require_text docs/REGRESSION_PREVENTION_POLICY.md 'X先行の変更凍結を必須とする'
for required_ledger_id in X-START-01 X-START-02 X-START-03 X-GRAPH-01 X-THREAD-01 PROC-LEDGER-01; do
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
require_text src/main.rs 'remaining_graph_does_not_infer_quota_loss_from_model_spend'
require_text src/main.rs 'native_startup_loading_requires_a_complete_authenticated_generation'
require_text src/main.rs 'native_startup_failure_releases_loading_surface'
require_text src/main.rs '"startup-loading"'
require_text ui/app.slint 'startup-loading: false'
require_text ui/app.slint 'text: "◌  " + root.strings.checking;'

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

require_rust_test_pass() {
    local qualified_name="$1"
    rg -q --fixed-strings "test ${qualified_name} ... ok" <<<"$all_target_output" ||
        fail "required Rust test did not run and pass in the all-target output: $qualified_name"
}

# These focused regressions are acceptance requirements, but must be proven by
# the one all-target test invocation above so that no test gets a second run.
for required_test in \
    historical_week_fixture_preserves_each_period_and_graph_samples \
    observed_moving_reset_sequence_keeps_the_spend_in_the_selected_graph \
    long_rolling_reset_sequence_stays_in_one_period_after_a_real_boundary \
    quota_only_reset_fragments_stay_with_the_adjacent_spend_period \
    live_rolling_quota_chain_does_not_expose_an_empty_past_period \
    affected_period_keeps_sol_spend_and_unobserved_quota_distinct \
    shared_graph_fixture_is_the_x_history_oracle \
    model_graph_does_not_invent_spend_during_an_unobserved_gap \
    unused_intervals_mark_long_gap_before_observed_spend \
    graph_controls_use_one_visual_boundary_and_show_short_histories \
    remaining_graph_does_not_infer_quota_loss_from_model_spend \
    affected_timestamp_does_not_mix_a_singleton_reset_period_into_history \
    ambiguous_missing_quota_row_at_a_spend_timestamp_is_not_a_period \
    singleton_reset_snapshot_overlapping_a_spend_period_stays_separate \
    graph_collision_preview_matches_the_historical_singleton_oracle \
    moving_reset_collision_at_30_and_60_seconds_fails_closed \
    record_rejects_alias_quota_collision_before_canonical_merge \
    same_timestamp_reset_drift_above_jitter_fails_closed \
    startup_load_sanitizes_legacy_same_timestamp_quota_collision \
    periodic_quota_refresh_retains_last_good_main_snapshot \
    product_version_is_visible_once_on_native_main_surface \
    public_snapshot_is_whitelisted_and_tracks_auth_state; do
    require_rust_test_pass "tests::$required_test"
done
require_rust_test_pass \
    'thread_contract::tests::recoverable_rollout_parser_skips_only_malformed_token_count_records'
for required_thread_failure_test in \
    thread_c_all_current_cycle_failure_classes_return_no_partial_snapshot \
    thread_c_candidate_failure_rejects_the_complete_cycle \
    thread_c_known_token_invalid_event_rejects_entire_rollout \
    thread_c_no_thread_and_all_candidate_failure_are_distinct \
    thread_c_private_accumulator_abort_never_yields_partial_snapshot \
    thread_c_snapshot_rejects_partial_candidate_reads; do
    require_rust_test_pass "thread_contract::tests::$required_thread_failure_test"
done

# Data-protection tests live in several Rust targets.  Match their fully
# qualified output names without hard-coding a module path that is not part of
# the contract.
for required_data_test in \
    db_protection_runtime_backup_migration_restore \
    failed_backup_rotation_keeps_existing_generation_untouched \
    migration_that_drops_a_valid_row_is_rejected_before_switch \
    oversized_tool_records_do_not_hide_following_usage_samples \
    recoverable_rollout_parser_keeps_running_state_around_large_tool_output \
    concurrent_collectors_merge_one_minute_without_duplicate_rows \
    backup_generations_are_sqlite_consistent_and_bounded \
    verified_migration_switches_only_after_candidate_validation \
    invalid_migration_candidate_leaves_source_untouched \
    stale_pid_lock_is_reclaimed_and_live_lock_is_singleton \
    opening_an_old_schema_is_rejected_without_migration \
    corrupt_database_error_preserves_the_original_file; do
    if ! rg -q --fixed-strings "test ${required_data_test} ... ok" <<<"$all_target_output" &&
       ! rg -q -e "^test [^[:space:]]+::${required_data_test} \.\.\. ok$" <<<"$all_target_output"; then
        fail "required data-protection test did not run and pass in the all-target output: $required_data_test"
    fi
done

run_checked 'Rust release build' cargo build --release --locked

# When a local X11 display is available, require a fresh rendered graph image
# as part of the same delivery check. Headless runners cannot satisfy this
# visual requirement and therefore do not claim X11 image PASS.
if [[ -n "${DISPLAY:-}" ]]; then
    run_checked 'X11 graph visual gate' bash scripts/x11_graph_visual_gate.sh
    run_checked 'X11 startup visual gate' bash scripts/x11_startup_visual_gate.sh
else
    fail 'X11 graph visual gate unverified (DISPLAY unavailable)'
fi

echo 'regression-guard: PASS'

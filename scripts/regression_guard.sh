#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "regression-guard: FAIL: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || fail 'expected exactly one check: --format, --test, or --history-graph'

run_exact_test() {
    local target="$1" test_name="$2" output_file
    output_file="$(mktemp "${TMPDIR:-/tmp}/codex-info-rust-test.XXXXXX")"
    if ! cargo test --locked "$target" "tests::$test_name" -- --exact --nocapture \
        >"$output_file" 2>&1; then
        cat "$output_file" >&2
        rm -f -- "$output_file"
        fail "focused Rust test failed: $target tests::$test_name"
    fi
    if ! rg -q 'test result: ok\. 1 passed; 0 failed; 0 ignored' "$output_file"; then
        cat "$output_file" >&2
        rm -f -- "$output_file"
        fail "focused Rust test did not execute exactly once: $target tests::$test_name"
    fi
    rm -f -- "$output_file"
    printf 'regression-guard: evidence test=%s tests::%s count=1\n' "$target" "$test_name"
}

case "$1" in
    --format)
        cargo fmt --check
        echo 'regression-guard: PASS check=rust-format'
        ;;
    --test)
        test_output="$(cargo test --locked --all-targets -- --nocapture 2>&1)" || {
            printf '%s\n' "$test_output" >&2
            fail 'Rust tests failed'
        }
        printf '%s\n' "$test_output"
        # A repository may legitimately contain a zero-test binary target.
        # Require positive evidence somewhere in the selected Rust test run;
        # do not enforce a brittle test-name inventory or arbitrary count.
        rg -q 'running [1-9][0-9]* tests?' <<<"$test_output" ||
            fail 'selected Rust test run executed zero tests'
        echo 'regression-guard: PASS check=rust-test'
        ;;
    --history-graph)
        main_tests=(
            startup_load_keeps_dense_alias_history_and_publishes_each_period
            shared_graph_fixture_is_the_x_history_oracle
            weekly_reset_rollover_projects_one_current_cycle_without_mixing
            current_period_bounds_stay_canonical_across_selected_reset_drift
            incident_shape_never_turns_a_regressed_scan_into_a_late_vertical_drop
            confirmed_recorder_gap_breaks_both_sources_instead_of_interpolating
            graph_paths_start_at_first_observation_without_inventing_a_reset_value
            remaining_graph_preserves_unattributed_quota_changes_as_inferred
            remaining_graph_does_not_infer_quota_loss_from_model_spend
            model_graph_does_not_invent_spend_during_an_unobserved_gap
            remaining_graph_stays_empty_without_observations
            zero_cost_period_starts_at_the_first_observation
            graph_breaks_for_legacy_and_unavailable_until_confirmed_recovery
        )
        store_tests=(
            recent_read_uses_one_month_half_open_interval_at_month_ends
            recent_read_filters_invalid_values_without_deleting_rows
        )
        for test_name in "${main_tests[@]}"; do
            run_exact_test --bin=codex_info "$test_name"
        done
        for test_name in "${store_tests[@]}"; do
            run_exact_test --test=usage_store "$test_name"
        done
        echo 'regression-guard: PASS check=rust-history-graph cases=15'
        ;;
    *)
        fail "unknown check: $1"
        ;;
esac

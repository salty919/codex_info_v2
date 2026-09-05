#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "regression-guard: FAIL: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || fail 'expected exactly one check: --format, --test, --history-graph, --model-history, --recorder-gap, or --resident-publication'

run_exact_test() {
    local target="$1" test_name="$2" output_file
    output_file="$(mktemp "${TMPDIR:-/tmp}/codex-info-rust-test.XXXXXX")"
    if ! cargo test --locked "$target" "$test_name" -- --exact --nocapture \
        >"$output_file" 2>&1; then
        cat "$output_file" >&2
        rm -f -- "$output_file"
        fail "focused Rust test failed: $target $test_name"
    fi
    if ! grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored' "$output_file"; then
        cat "$output_file" >&2
        rm -f -- "$output_file"
        fail "focused Rust test did not execute exactly once: $target $test_name"
    fi
    rm -f -- "$output_file"
    printf 'regression-guard: evidence test=%s %s count=1\n' "$target" "$test_name"
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
            run_exact_test --bin=codex_info "tests::$test_name"
        done
        for test_name in "${store_tests[@]}"; do
            run_exact_test --test=usage_store "wave_b_correction_tests::$test_name"
        done
        echo 'regression-guard: PASS check=rust-history-graph cases=15'
        ;;
    --model-history)
        main_tests=(
            astra_session_delta_survives_database_restart_without_duplicate_tokens
            astra_usage_keeps_write_tokens_and_uses_four_component_prices
            versioned_candidates_preserve_known_legacy_models_and_unavailable_rows
            session_traversal_bounds_count_and_depth_not_lifetime_file_size
            linux_details_v3_uses_conditional_cache_and_legacy_fallback_only_on_404
            service_refresh_preserves_a_bounded_historical_selection
            v3_historical_selection_keeps_astra_without_requiring_legacy_models
            graph_observed_model_paths_do_not_look_inferred
            unused_intervals_do_not_call_unobserved_spend_idle
            graph_collision_preview_matches_the_historical_singleton_oracle
        )
        server_tests=(
            versioned_details_share_one_pair_and_v3_is_domain_shaped
        )
        store_tests=(
            three_month_prune_removes_old_sidecars_but_keeps_new_rows_and_row_one
            pruning_removes_only_old_rows_and_preserves_boundary_across_reset_periods
        )
        for test_name in "${main_tests[@]}"; do
            run_exact_test --bin=codex_info "tests::$test_name"
        done
        for test_name in "${server_tests[@]}"; do
            run_exact_test --lib "server::tests::$test_name"
        done
        for test_name in "${store_tests[@]}"; do
            run_exact_test --lib "usage_store::tests::$test_name"
        done
        echo 'regression-guard: PASS check=rust-model-history cases=13'
        ;;
    --resident-publication)
        main_tests=(
            unchanged_resident_tick_reuses_snapshot_and_worker_event_publishes_once
            recorder_failure_keeps_interval_retry_when_snapshot_publication_also_fails
            resident_publication_holds_incomplete_usage_and_errors_without_mixing_roots
            resident_recorder_retries_after_interval_without_dropping_pending_batch
            outage_recovery_uses_one_periodic_local_collector_lane
            resident_scheduler_keeps_periodic_thread_reads_single_flight
        )
        for test_name in "${main_tests[@]}"; do
            run_exact_test --bin=codex_info "tests::$test_name"
        done
        echo 'regression-guard: PASS check=rust-resident-publication cases=6'
        ;;
    --recorder-gap)
        run_exact_test --bin=codex_info \
            daemon::tests::recorder_production_source_result_reaches_all_gap_states_without_session_quota_proof
        main_tests=(
            local_failure_queues_fresh_quota_as_unavailable_observation
            local_failure_does_not_queue_quota_for_outage_or_stale_admission
            local_failure_quota_batch_survives_recorder_retry_exactly_once
        )
        for test_name in "${main_tests[@]}"; do
            run_exact_test --bin=codex_info "tests::$test_name"
        done
        echo 'regression-guard: PASS check=rust-recorder-gap cases=4'
        ;;
    *)
        fail "unknown check: $1"
        ;;
esac

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "regression-guard: FAIL: $*" >&2
    exit 1
}

[[ $# -eq 1 ]] || fail 'expected exactly one check: --format or --test'

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
    *)
        fail "unknown check: $1"
        ;;
esac

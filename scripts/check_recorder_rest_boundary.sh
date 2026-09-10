#!/usr/bin/env bash
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only

set -euo pipefail

mode=${1:-full}
if [[ "$mode" != full && "$mode" != --static-only ]]; then
    printf 'usage: %s [--static-only]\n' "${0##*/}" >&2
    exit 2
fi

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "$repo_root"

rest_tree=$(cargo tree --locked -p codex-info-rest --edges normal --prefix none)
for forbidden in codex_info codex-info-recorder codex-info-db-writer slint winit x11rb; do
    if awk '{print $1}' <<<"$rest_tree" | grep -Fxq -- "$forbidden"; then
        echo "REST dependency boundary violation: $forbidden" >&2
        exit 1
    fi
done

for forbidden_source in \
    RecorderWorker \
    SessionTraversalBudget \
    collect_session_append \
    commit_session_collection \
    commit_durable_state \
    prune_older_than_three_months \
    backup_generations \
    migrate_verified \
    SQLITE_OPEN_READ_WRITE \
    SQLITE_OPEN_CREATE \
    TransactionBehavior; do
    if rg -n --glob '*.rs' -- "$forbidden_source" \
        crates/codex-info-rest \
        crates/codex-info-db-reader \
        crates/codex-info-rest-contract; then
        echo "REST source boundary violation: $forbidden_source" >&2
        exit 1
    fi
done

if [[ "$mode" == --static-only ]]; then
    printf 'recorder/REST static boundary: PASS\n'
    exit 0
fi

cargo test --locked -p codex-info-db-reader -p codex-info-rest-contract -p codex-info-rest
cargo test --locked -p codex-info-db-writer -p codex-info-recorder
cargo build --locked --release -p codex-info-recorder -p codex-info-rest
cargo build --locked --release --bin codex_info

recorder=target/release/codex_info_recorder
rest=target/release/codex_info_rest
test -x "$recorder"
test -x "$rest"

recorder_hash=$(sha256sum -- "$recorder" | awk '{print $1}')
rest_hash=$(sha256sum -- "$rest" | awk '{print $1}')
if [[ "$recorder_hash" == "$rest_hash" ]]; then
    echo "recorder and REST artifacts unexpectedly have the same SHA-256" >&2
    exit 1
fi

# The public root binary is a REST client. Exercise every legacy service
# control at the release boundary and ensure rejection happens before a
# listener or recorder can be created. Keep this runtime check independent of
# the parser unit test so a dead combined path cannot be exposed by accident.
root=target/release/codex_info
test -x "$root"
probe_port=28787
before_listener=$(ss -H -ltn "( sport = :$probe_port )" || true)
for args in \
    "--port $probe_port" \
    "--stop" \
    "--service" \
    "--listen" \
    "--record-daemon" \
    "--once" \
    "--ui-only" \
    "--all"; do
    read -r -a argv <<<"$args"
    set +e
    timeout --kill-after=1s 3s "$root" "${argv[@]}" >/dev/null 2>&1
    status=$?
    set -e
    if [[ "$status" -eq 0 || "$status" -eq 124 || "$status" -eq 137 ]]; then
        echo "legacy service control was not rejected: $args (status=$status)" >&2
        exit 1
    fi
    after_listener=$(ss -H -ltn "( sport = :$probe_port )" || true)
    if [[ "$after_listener" != "$before_listener" ]]; then
        echo "legacy service control opened listener: $args" >&2
        exit 1
    fi
done

printf 'recorder_sha256=%s\nrest_sha256=%s\n' "$recorder_hash" "$rest_hash"

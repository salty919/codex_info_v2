#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

case "${1:-}" in
    '') exec python3 scripts/requirements_authority.py ;;
    --final) exec python3 scripts/requirements_authority.py --final ;;
    *)
        echo "requirements-ledger-gate: FAIL: unknown option: $1" >&2
        exit 1
        ;;
esac

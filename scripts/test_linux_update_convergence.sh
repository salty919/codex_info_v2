#!/usr/bin/env bash
set -euo pipefail

# The convergence gate is deliberately finite and fixture-only.  The bundle
# test exercises the common resolver/installer for manual, interrupted, equal
# version, and remove paths; the launcher test covers the reverse old-launcher
# regression and wrapper grammar.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
bash "$SCRIPT_DIR/test_linux_bundle.sh"
bash "$SCRIPT_DIR/test_run_launcher_version_sync.sh"
printf 'linux update convergence cases passed\n'

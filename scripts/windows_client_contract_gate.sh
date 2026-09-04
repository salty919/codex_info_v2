#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

fail() {
    echo "windows-client-contract-gate: FAIL: $*" >&2
    exit 1
}

[[ $# -eq 0 ]] || fail "unexpected argument: $1"
command -v dotnet >/dev/null 2>&1 || fail 'dotnet is unavailable'

solution='windows-client/CodexInfo.WindowsClient.sln'
results_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-info-windows-tests.XXXXXX")"
case "$results_dir" in
    "${TMPDIR:-/tmp}"/codex-info-windows-tests.*) ;;
    *) fail "unsafe temporary result path: $results_dir" ;;
esac
trap 'rm -rf -- "$results_dir"' EXIT

# Restore, formatting, and unit behavior form one Windows-source check.  The
# gate owns each command once and deliberately does not mirror test names,
# source strings, coverage percentages, installer, or real-OS E2E contracts.
dotnet restore "$solution" --locked-mode
dotnet format "$solution" --no-restore --verify-no-changes
dotnet test "$solution" \
    --no-restore \
    --configuration Release \
    --results-directory "$results_dir" \
    --logger 'trx;LogFilePrefix=windows-client'

python3 - "$results_dir" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

reports = sorted(Path(sys.argv[1]).rglob("*.trx"))
if not reports:
    raise SystemExit("windows-client-contract-gate: FAIL: TRX report is missing")

totals = {name: 0 for name in ("total", "executed", "passed", "failed", "notExecuted")}
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

# A positive passing observation is required.  Skipped/not-executed tests are
# reported but are not converted into failures merely to satisfy a count.
if totals["total"] <= 0 or totals["executed"] <= 0 or totals["passed"] <= 0:
    raise SystemExit("windows-client-contract-gate: FAIL: zero Windows tests executed")
if totals["failed"] != 0:
    raise SystemExit("windows-client-contract-gate: FAIL: Windows test failure recorded")
print(
    "windows-client-contract-gate: evidence "
    + " ".join(f"{name}={value}" for name, value in totals.items())
)
PY

echo 'windows-client-contract-gate: PASS check=windows-contract'

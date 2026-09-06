#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

[[ $# -eq 0 ]] || {
    echo "windows-client-contract-gate: FAIL: unexpected argument: $1" >&2
    exit 1
}
command -v dotnet >/dev/null 2>&1 || {
    echo 'windows-client-contract-gate: FAIL: dotnet is unavailable' >&2
    exit 1
}

powershell_path_lines="$(python3 - <<'PY'
import json
import os
import pathlib
import sys

try:
    paths = json.loads(os.environ.get("POWERSHELL_PATHS_JSON", "[]"))
except json.JSONDecodeError as error:
    raise SystemExit(f"windows-client-contract-gate: FAIL: invalid PowerShell path JSON: {error}")
if not isinstance(paths, list) or any(not isinstance(path, str) for path in paths):
    raise SystemExit("windows-client-contract-gate: FAIL: PowerShell paths must be a JSON string list")
for path in paths:
    value = pathlib.PurePosixPath(path)
    if not path or value.is_absolute() or ".." in value.parts or any(char in path for char in "\0\r\n"):
        raise SystemExit("windows-client-contract-gate: FAIL: unsafe PowerShell path")
    print(path)
PY
)"
powershell_paths=()
[[ -z "$powershell_path_lines" ]] || mapfile -t powershell_paths <<<"$powershell_path_lines"
if ((${#powershell_paths[@]} > 0)); then
    if command -v pwsh >/dev/null 2>&1; then
        powershell_command='pwsh'
    elif command -v powershell.exe >/dev/null 2>&1; then
        powershell_command='powershell.exe'
    else
        echo 'windows-client-contract-gate: FAIL: PowerShell parser is unavailable' >&2
        exit 1
    fi
    for path in "${powershell_paths[@]}"; do
        [[ -f "$path" && ! -L "$path" ]] || continue
        powershell_environment=(env "CODEX_INFO_PS_PATH=$PWD/$path")
        if [[ "$powershell_command" == powershell.exe ]]; then
            wsl_environment="${WSLENV:-}"
            [[ -z "$wsl_environment" ]] || wsl_environment+=':'
            powershell_environment+=("WSLENV=${wsl_environment}CODEX_INFO_PS_PATH/p")
        fi
        "${powershell_environment[@]}" "$powershell_command" \
            -NoProfile -NonInteractive -Command '
            $tokens = $null
            $errors = $null
            [System.Management.Automation.Language.Parser]::ParseFile(
                $env:CODEX_INFO_PS_PATH, [ref]$tokens, [ref]$errors
            ) > $null
            if ($errors.Count -gt 0) {
                $errors | ForEach-Object { [Console]::Error.WriteLine($_.Message) }
                exit 1
            }
        '
    done
fi

solution='windows-client/CodexInfo.WindowsClient.sln'
results_dir="$(mktemp -d "${TMPDIR:-/tmp}/codex-info-windows-tests.XXXXXX")"
case "$results_dir" in
    "${TMPDIR:-/tmp}"/codex-info-windows-tests.*) ;;
    *)
        echo "windows-client-contract-gate: FAIL: unsafe temporary result path: $results_dir" >&2
        exit 1
        ;;
esac
trap 'rm -rf -- "$results_dir"' EXIT

dotnet restore "$solution" --locked-mode
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

totals = {name: 0 for name in ("total", "executed", "passed", "failed")}
for report in reports:
    counters = [
        element
        for element in ET.parse(report).getroot().iter()
        if element.tag.endswith("Counters")
    ]
    if not counters:
        raise SystemExit(
            f"windows-client-contract-gate: FAIL: TRX counters are missing: {report}"
        )
    for name in totals:
        try:
            totals[name] += int(counters[-1].attrib.get(name, "0"))
        except ValueError as exc:
            raise SystemExit(
                f"windows-client-contract-gate: FAIL: malformed {name} counter: {report}"
            ) from exc

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

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

ledger='docs/REQUIREMENTS_LEDGER.md'
fail() { echo "requirements-ledger-gate: FAIL: $*" >&2; exit 1; }
final=0
if [[ "${1:-}" == "--final" ]]; then
    final=1
elif [[ $# -gt 0 ]]; then
    fail "unknown option: $1"
fi
[[ -f "$ledger" ]] || fail "missing $ledger"
grep -Fq '| ID | 要求（観測可能な契約） | 境界・失敗動作 | 実装範囲 | 独立オラクル | 状態 |' "$ledger" || fail 'ledger header is missing'

mapfile -t rows < <(awk -F'|' '/^\| [A-Z0-9]+-[A-Z0-9-]+ \|/ { gsub(/[[:space:]]/, "", $2); print $2 }' "$ledger")
[[ "${#rows[@]}" -gt 0 ]] || fail 'ledger contains no requirement rows'
duplicates="$(printf '%s\n' "${rows[@]}" | sort | uniq -d)"
[[ -z "$duplicates" ]] || fail "duplicate requirement IDs: $duplicates"
unverified=()

# shellcheck disable=SC2034  # Fields are read indirectly through ${!field}.
while IFS='|' read -r _ id contract boundary scope oracle status _; do
    [[ -n "${id//[[:space:]]/}" ]] || continue
    for field in contract boundary scope oracle status; do
        value="${!field}"
        [[ -n "${value//[[:space:]]/}" ]] || fail "empty $field for $id"
    done
    [[ "$status" =~ ^[[:space:]]*(implemented|verified)[[:space:]]*$ ]] ||
        fail "unverified status for $id: $status"
    if (( final )) && [[ "$status" != *verified* ]]; then
        unverified+=("${id//[[:space:]]/}")
    fi
done < <(awk '/^\| [A-Z0-9]+-[A-Z0-9-]+ \|/ { print }' "$ledger")

# Every changed path must be named by at least one ledger scope. This covers
# worktree, index, and untracked files so an unregistered implementation cannot
# sneak into a PR merely because the requirement table itself still parses.
# shellcheck disable=SC2016  # Backticks are literal Markdown delimiters.
mapfile -t scoped_paths < <(grep -oE '`[^`]+`' "$ledger" | tr -d '`' | sort -u)
mapfile -t changed_paths < <({
    git -c core.quotePath=false diff --name-only
    git -c core.quotePath=false diff --cached --name-only
    git ls-files --others --exclude-standard |
        awk '$0 !~ /(^|\/)__pycache__\/[^/]+\.pyc$/'
} | sort -u)
for changed_path in "${changed_paths[@]}"; do
    [[ -n "$changed_path" ]] || continue
    listed=0
    for scoped_path in "${scoped_paths[@]}"; do
        if [[ "$scoped_path" == "$changed_path" || "$scoped_path" == */\* && "$changed_path" == "${scoped_path%/*}"/* ]]; then
            listed=1
            break
        fi
    done
    (( listed )) || fail "changed path is outside every ledger implementation scope: $changed_path"
done

if (( final )) && (( ${#unverified[@]} > 0 )); then
    fail "final gate requires verified status for: ${unverified[*]}"
fi

if (( final )); then
    echo "requirements-ledger-gate: PASS (final) rows=${#rows[@]}"
else
    echo "requirements-ledger-gate: PASS (schema) rows=${#rows[@]}"
fi

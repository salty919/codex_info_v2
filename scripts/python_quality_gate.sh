#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
python_paths=()
for path in "$@"; do
    if [[ "$path" == *.py && -f "$path" ]]; then
        python_paths+=("$path")
    fi
done
((${#python_paths[@]} > 0)) || exit 0

# Ruff intentionally skips EXE001 on WSL. Check the mode that Git will publish
# on both WSL and Actions, using filesystem mode only for untracked files.
python3 - "${python_paths[@]}" <<'PY'
import os
import subprocess
import sys
from pathlib import Path

entries = subprocess.check_output(["/usr/bin/git", "ls-files", "--stage", "-z"])
modes = {}
for entry in entries.split(b"\0"):
    if entry:
        metadata, name = entry.split(b"\t", 1)
        modes[os.fsdecode(name)] = int(metadata.split(b" ", 1)[0], 8)
failed = False
for name in sys.argv[1:]:
    path = Path(name)
    mode = modes.get(name, path.stat().st_mode)
    with path.open("rb") as source:
        if source.read(2) == b"#!" and not mode & 0o111:
            print(f"{name}:1: EXE001 shebang is present but Git mode is not executable")
            failed = True
raise SystemExit(1 if failed else 0)
PY

version="$(python3 - "$repo_root/scripts/python_quality.toml" <<'PY'
import sys
import tomllib
with open(sys.argv[1], "rb") as source:
    print(tomllib.load(source)["required-version"].removeprefix("=="))
PY
)"
ruff_bin="$(command -v ruff || true)"
if [[ -z "$ruff_bin" ]] || [[ "$("$ruff_bin" --version)" != "ruff $version" ]]; then
    # The trusted main workflow may predate this check. The check owns its
    # dependency, so it can run before that workflow is updated as well.
    tool_env="$repo_root/artifacts/python-quality-venv"
    ruff_bin="$tool_env/bin/ruff"
    if [[ ! -x "$ruff_bin" ]] || [[ "$("$ruff_bin" --version)" != "ruff $version" ]]; then
        python3 -m venv "$tool_env"
        "$tool_env/bin/python" -I -m pip --isolated install \
            --disable-pip-version-check --index-url https://pypi.org/simple \
            --only-binary=:all: --no-deps --require-hashes \
            -r "$repo_root/.github/requirements-python-quality.txt"
    fi
fi
"$ruff_bin" check --config "$repo_root/scripts/python_quality.toml" -- "${python_paths[@]}"

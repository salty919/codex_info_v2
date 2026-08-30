#!/usr/bin/env python3
"""Focused local contract for selected-language CodeQL dispatch."""

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CODEQL = ROOT / ".github/workflows/codeql.yml"
SELECTIVE = ROOT / ".github/workflows/selective-quality.yml"


def require(source: str, marker: str) -> None:
    if marker not in source:
        raise AssertionError(f"missing workflow contract: {marker}")


def forbid(source: str, marker: str) -> None:
    if marker in source:
        raise AssertionError(f"forbidden workflow contract: {marker}")


def main() -> int:
    codeql = CODEQL.read_text(encoding="utf-8")
    selective = SELECTIVE.read_text(encoding="utf-8")
    for marker in (
        "  workflow_call:\n",
        "      languages_json:\n",
        "        language: ${{ fromJSON(inputs.languages_json) }}\n",
        "          ref: ${{ inputs.source_sha }}\n",
        "          sha: ${{ inputs.source_sha }}\n",
        "          ref: refs/heads/${{ inputs.head_ref }}\n",
        "          build-mode: none\n",
        "github/codeql-action/init@v4",
        "github/codeql-action/analyze@v4",
    ):
        require(codeql, marker)
    for marker in ("  schedule:\n", "  push:\n", "  pull_request:\n", "autobuild@"):
        forbid(codeql, marker)

    require(selective, "  codeql-quality:\n")
    require(selective, "    uses: ./.github/workflows/codeql.yml\n")
    require(
        selective,
        "      languages_json: ${{ toJSON(fromJSON(inputs.selection_json).codeql_languages) }}\n",
    )
    print("codeql-workflow-test: PASS cases=3")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

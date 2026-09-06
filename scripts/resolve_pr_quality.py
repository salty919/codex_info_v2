#!/usr/bin/env python3
"""Resolve one PR diff to its owners and enforce the main version transition."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Sequence

import ci_change_scope
import product_version


SHA = re.compile(r"^[0-9a-f]{40}$", re.ASCII)


class ResolutionError(RuntimeError):
    pass


def _git(*arguments: str) -> bytes:
    result = subprocess.run(
        ("git", *arguments), capture_output=True, check=False
    )
    if result.returncode != 0:
        message = result.stderr.decode("utf-8", errors="replace").strip()
        raise ResolutionError(message or f"git {' '.join(arguments)} failed")
    return result.stdout


def _revision_versions(revision: str) -> str:
    with tempfile.TemporaryDirectory(prefix="codex-info-pr-version-") as raw_root:
        root = Path(raw_root)
        paths = product_version.VersionPaths(
            cargo_toml=root / "Cargo.toml",
            cargo_lock=root / "Cargo.lock",
            windows_props=root / "windows-client" / "Directory.Build.props",
        )
        paths.windows_props.parent.mkdir()
        for repository_path, destination in (
            ("Cargo.toml", paths.cargo_toml),
            ("Cargo.lock", paths.cargo_lock),
            ("windows-client/Directory.Build.props", paths.windows_props),
        ):
            destination.write_bytes(_git("show", f"{revision}:{repository_path}"))
        return product_version.check_versions(paths)


def resolve(
    *,
    base_sha: str,
    head_sha: str,
    repository: str,
    head_repository: str,
    release_candidate: bool,
) -> ci_change_scope.Selection:
    if SHA.fullmatch(base_sha) is None or SHA.fullmatch(head_sha) is None:
        raise ResolutionError("base and head must be complete lowercase commit SHAs")
    if not repository or head_repository != repository:
        raise ResolutionError("quality evaluation requires a same-repository PR")

    raw_diff = _git(
        "diff",
        "--find-renames=50%",
        "--find-copies=50%",
        "--name-status",
        "-z",
        f"{base_sha}...{head_sha}",
    )
    try:
        selection = ci_change_scope.selection_from_name_status(
            raw_diff, release_candidate=release_candidate
        )
        if release_candidate:
            base_version = _revision_versions(base_sha)
            head_version = _revision_versions(head_sha)
            valid = (
                product_version.is_forward_version(base_version, head_version)
                if selection.binary_impact
                else head_version == base_version
            )
            if not valid:
                kind = "binary" if selection.binary_impact else "non-binary"
                raise ResolutionError(
                    f"{kind} PR version transition is invalid: "
                    f"{base_version} -> {head_version}"
                )
    except (ci_change_scope.ScopeError, product_version.ProductVersionError) as exc:
        raise ResolutionError(str(exc)) from exc
    return selection


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--head-repository", required=True)
    parser.add_argument("--release-candidate", action="store_true")
    args = parser.parse_args(argv)
    try:
        selection = resolve(
            base_sha=args.base,
            head_sha=args.head,
            repository=args.repository,
            head_repository=args.head_repository,
            release_candidate=args.release_candidate,
        )
    except ResolutionError as exc:
        print(f"resolve-pr-quality: FAIL {exc}", file=sys.stderr)
        return 1
    print(selection.as_json())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

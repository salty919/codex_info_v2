#!/usr/bin/env python3
"""Read and validate the synchronized product version inputs."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path


VERSION_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$",
    re.ASCII,
)


class ProductVersionError(RuntimeError):
    pass


@dataclass(frozen=True)
class VersionPaths:
    cargo_toml: Path
    cargo_lock: Path
    windows_props: Path

    def ordered(self) -> tuple[Path, Path, Path]:
        return self.cargo_toml, self.cargo_lock, self.windows_props


def _stable_version(value: object, source: Path) -> str:
    if not isinstance(value, str) or VERSION_PATTERN.fullmatch(value) is None:
        raise ProductVersionError(
            f"{source}: version must be a stable canonical X.Y.Z value: {value!r}"
        )
    return value


def _toml(path: Path) -> dict[str, object]:
    try:
        value = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ProductVersionError(f"{path}: invalid TOML: {exc}") from exc
    if not isinstance(value, dict):
        raise ProductVersionError(f"{path}: TOML root must be a table")
    return value


def _cargo_version(path: Path) -> str:
    package = _toml(path).get("package")
    if not isinstance(package, dict) or package.get("name") != "codex_info":
        raise ProductVersionError(f"{path}: [package].name must be codex_info")
    return _stable_version(package.get("version"), path)


def _lock_version(path: Path) -> str:
    packages = _toml(path).get("package")
    if not isinstance(packages, list):
        raise ProductVersionError(f"{path}: Cargo.lock has no package list")
    roots = [
        package
        for package in packages
        if isinstance(package, dict) and package.get("name") == "codex_info"
    ]
    if len(roots) != 1:
        raise ProductVersionError(
            f"{path}: Cargo.lock must contain exactly one codex_info package"
        )
    return _stable_version(roots[0].get("version"), path)


def _local_name(tag: object) -> str:
    return tag.rsplit("}", 1)[-1] if isinstance(tag, str) else ""


def _windows_version(path: Path) -> str:
    try:
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError) as exc:
        raise ProductVersionError(f"{path}: invalid XML: {exc}") from exc
    if _local_name(root.tag) != "Project":
        raise ProductVersionError(f"{path}: XML root must be Project")
    versions = [
        node
        for group in list(root)
        if _local_name(group.tag) == "PropertyGroup"
        for node in list(group)
        if _local_name(node.tag) == "Version"
    ]
    if len(versions) != 1 or list(versions[0]):
        raise ProductVersionError(
            f"{path}: Project/PropertyGroup must contain one plain Version element"
        )
    return _stable_version((versions[0].text or "").strip(), path)


def check_versions(paths: VersionPaths) -> str:
    resolved = tuple(path.resolve(strict=False) for path in paths.ordered())
    if len(set(resolved)) != 3:
        raise ProductVersionError("version targets must be three distinct files")
    versions = (
        _cargo_version(resolved[0]),
        _lock_version(resolved[1]),
        _windows_version(resolved[2]),
    )
    if len(set(versions)) != 1:
        detail = ", ".join(
            f"{path}={version}" for path, version in zip(resolved, versions, strict=True)
        )
        raise ProductVersionError(f"version targets are not synchronized: {detail}")
    return versions[0]


def is_forward_version(base: str, candidate: str) -> bool:
    def parts(value: str) -> tuple[int, int, int]:
        return tuple(
            int(part) for part in _stable_version(value, Path("--version")).split(".")
        )

    return parts(candidate) > parts(base)


def _default_paths() -> VersionPaths:
    root = Path(__file__).resolve().parents[1]
    return VersionPaths(
        cargo_toml=root / "Cargo.toml",
        cargo_lock=root / "Cargo.lock",
        windows_props=root / "windows-client" / "Directory.Build.props",
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check")
    check.add_argument("--cargo-toml", type=Path)
    check.add_argument("--cargo-lock", type=Path)
    check.add_argument("--windows-props", type=Path)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        defaults = _default_paths()
        paths = VersionPaths(
            cargo_toml=arguments.cargo_toml or defaults.cargo_toml,
            cargo_lock=arguments.cargo_lock or defaults.cargo_lock,
            windows_props=arguments.windows_props or defaults.windows_props,
        )
        print(f"version={check_versions(paths)}")
        print("synchronized=true")
        return 0
    except ProductVersionError as exc:
        print(f"product-version: ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Validate the requirement-owner registry and requirements ledger.

The validator intentionally performs only local, finite markdown/schema checks.
It is meant to run before an expensive quality gate and does not inspect remote
evidence, build products, or test suites.
"""

from __future__ import annotations

import argparse
import os
from dataclasses import dataclass
from pathlib import Path
import re
import sys
from typing import Iterable, Mapping, Sequence


class AuthorityValidationError(ValueError):
    """A structural requirement-authority check failed."""


# Owner IDs and master requirement IDs are deliberately restricted to the
# portable identifier alphabet used by the repository's requirement labels.
# The same expression accepts an ID without a hyphen so small fixture
# registries can use names such as ``REQ1`` while still rejecting whitespace,
# punctuation, and lower-case aliases.
IDENTIFIER_RE = re.compile(r"[A-Z][A-Z0-9]*(?:-[A-Z0-9]+)*\Z")

OWNER_MARKER_RE = re.compile(
    r"<!--\s*codex-info-requirement-owner\s*:\s*(.*?)\s*-->", re.DOTALL
)
MASTER_IDS_MARKER_RE = re.compile(
    r"<!--\s*codex-info-master-ids\s*:\s*(.*?)\s*-->", re.DOTALL
)

REGISTRY_HEADER = ("owner ID", "唯一のowner", "所有する契約境界")
LEDGER_HEADER = ("ID", "owner", "実装範囲", "直接オラクル", "状態")
VALID_STATUSES = frozenset({"implemented", "verified"})
DOCUMENT_SUFFIXES = frozenset({".md", ".markdown", ".rst", ".txt", ".adoc"})
SKIPPED_DIRECTORY_NAMES = frozenset(
    {".git", "target", "__pycache__", ".venv", "venv", "node_modules"}
)


@dataclass(frozen=True)
class RegistryOwner:
    """One owner/path row from the product-requirements registry."""

    owner_id: str
    relative_path: str
    path: Path


@dataclass(frozen=True)
class ValidationReport:
    """Counts emitted by a successful validation."""

    owners: int
    requirements: int
    final: bool


def _fail(message: str) -> None:
    raise AuthorityValidationError(message)


def _read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        _fail(f"missing document: {path}")
    except (OSError, UnicodeError) as exc:
        _fail(f"cannot read {path}: {exc}")
    raise AssertionError("unreachable")


def _clean_cell(value: str) -> str:
    """Trim a markdown cell and one optional pair of inline-code ticks."""

    value = value.strip()
    if len(value) >= 2 and value.startswith("`") and value.endswith("`"):
        value = value[1:-1].strip()
    return value


def _table_cells(line: str) -> list[str] | None:
    """Return cells for a pipe row, accepting an optional trailing pipe."""

    stripped = line.strip()
    if not stripped.startswith("|"):
        return None
    body = stripped[1:]
    if body.endswith("|"):
        body = body[:-1]
    return [cell.strip() for cell in body.split("|")]


def _is_separator(cells: Sequence[str]) -> bool:
    if not cells:
        return False
    return all(bool(re.fullmatch(r":?-{3,}:?", cell.strip())) for cell in cells)


def _find_table(
    text: str, header: Sequence[str], *, label: str, width: int
) -> list[list[str]]:
    """Find one contiguous markdown table with an exact header."""

    lines = text.splitlines()
    header_rows: list[int] = []
    for index, line in enumerate(lines):
        cells = _table_cells(line)
        if cells is not None and tuple(cells) == tuple(header):
            header_rows.append(index)

    if not header_rows:
        _fail(f"{label} table header is missing")
    if len(header_rows) != 1:
        _fail(f"{label} table header is duplicated")

    header_index = header_rows[0]
    if header_index + 1 >= len(lines):
        _fail(f"{label} table separator is missing")
    separator = _table_cells(lines[header_index + 1])
    if separator is None or len(separator) != width or not _is_separator(separator):
        _fail(f"{label} table separator is malformed")

    rows: list[list[str]] = []
    for line in lines[header_index + 2 :]:
        cells = _table_cells(line)
        if cells is None:
            break
        if _is_separator(cells):
            continue
        if len(cells) != width:
            _fail(f"{label} table row has {len(cells)} cells; expected {width}")
        rows.append(cells)

    if not rows:
        _fail(f"{label} table contains no rows")
    return rows


def _safe_registry_path(root: Path, raw_path: str, owner_id: str) -> tuple[str, Path]:
    """Validate and resolve a registry path without allowing path escape."""

    relative = raw_path.strip()
    if not relative:
        _fail(f"empty registry path for owner {owner_id}")
    path = Path(relative)
    if path.is_absolute() or "\\" in relative:
        _fail(f"malformed registry path for owner {owner_id}: {raw_path!r}")
    if any(part in {"", ".", ".."} for part in relative.split("/")):
        _fail(f"malformed registry path for owner {owner_id}: {raw_path!r}")

    root_resolved = root.resolve()
    candidate = (root / path).resolve()
    try:
        candidate.relative_to(root_resolved)
    except ValueError:
        _fail(f"registry path escapes root for owner {owner_id}: {raw_path!r}")
    if not candidate.is_file():
        _fail(f"registry path does not exist for owner {owner_id}: {relative}")
    return path.as_posix(), candidate


def _parse_registry(root: Path) -> list[RegistryOwner]:
    registry_path = root / "docs" / "PRODUCT_REQUIREMENTS.md"
    text = _read_text(registry_path)
    rows = _find_table(
        text, REGISTRY_HEADER, label="registry", width=len(REGISTRY_HEADER)
    )

    owners: list[RegistryOwner] = []
    owner_rows: dict[str, int] = {}
    path_rows: dict[str, int] = {}
    for row_number, row in enumerate(rows, start=1):
        owner_id = _clean_cell(row[0])
        relative_path_raw = _clean_cell(row[1])
        boundary = row[2].strip()
        if not IDENTIFIER_RE.fullmatch(owner_id):
            _fail(f"malformed registry owner ID on row {row_number}: {owner_id!r}")
        if not boundary:
            _fail(f"empty registry contract boundary for owner {owner_id}")
        if owner_id in owner_rows:
            _fail(f"duplicate registry owner: {owner_id}")
        relative_path, path = _safe_registry_path(root, relative_path_raw, owner_id)
        if relative_path in path_rows:
            _fail(f"duplicate registry owner path: {relative_path}")
        owner_rows[owner_id] = row_number
        path_rows[relative_path] = row_number
        owners.append(RegistryOwner(owner_id, relative_path, path))
    return owners


def _iter_document_paths(root: Path, registered_paths: Iterable[Path]) -> list[Path]:
    """Enumerate likely documentation files for unregistered marker checks."""

    paths: set[Path] = set(registered_paths)
    for directory, dirnames, filenames in os.walk(root):
        dirnames[:] = [
            name for name in dirnames if name not in SKIPPED_DIRECTORY_NAMES
        ]
        directory_path = Path(directory)
        for filename in filenames:
            path = directory_path / filename
            if path.suffix.lower() in DOCUMENT_SUFFIXES:
                paths.add(path)

    root_resolved = root.resolve()
    result: list[Path] = []
    for path in sorted(paths, key=lambda item: item.as_posix()):
        try:
            resolved = path.resolve()
            resolved.relative_to(root_resolved)
        except (OSError, ValueError):
            # Registered paths have already been checked by _safe_registry_path;
            # this branch only protects the broad marker scan from symlink
            # escapes introduced by an unrelated document.
            continue
        if resolved.is_file():
            result.append(resolved)
    return result


def _parse_owner_documents(
    root: Path, owners: Sequence[RegistryOwner]
) -> Mapping[str, set[str]]:
    registry_owners = {owner.owner_id for owner in owners}
    registered_paths = {
        owner.path.resolve(): owner.owner_id for owner in owners
    }
    paths = _iter_document_paths(root, (owner.path for owner in owners))

    # An owner marker is an authority declaration, so it is legal only in the
    # exact path registered for that owner.  Copyright/SPDX comments may come
    # before it; their placement has no bearing on ownership.
    for path in paths:
        text = _read_text(path)
        expected_owner = registered_paths.get(path.resolve())
        for match in OWNER_MARKER_RE.finditer(text):
            marker_owner = match.group(1).strip()
            if not IDENTIFIER_RE.fullmatch(marker_owner):
                _fail(f"malformed owner marker in {path}: {marker_owner!r}")
            if marker_owner not in registry_owners:
                _fail(f"unregistered marker owner in {path}: {marker_owner}")
            if expected_owner is None:
                _fail(
                    f"owner marker is outside its registered path: "
                    f"{marker_owner} in {path.relative_to(root)}"
                )
            if marker_owner != expected_owner:
                _fail(
                    f"owner marker mismatch in {path.relative_to(root)}: "
                    f"expected {expected_owner}, got {marker_owner}"
                )
        if expected_owner is None and MASTER_IDS_MARKER_RE.search(text):
            _fail(
                f"master-ID marker is outside a registered owner path: "
                f"{path.relative_to(root)}"
            )

    master_by_owner: dict[str, set[str]] = {}
    seen_master_ids: dict[str, str] = {}
    for owner in owners:
        text = _read_text(owner.path)
        owner_matches = list(OWNER_MARKER_RE.finditer(text))
        if not owner_matches:
            _fail(f"owner marker is missing in {owner.relative_path}")
        if len(owner_matches) != 1:
            _fail(f"owner marker is duplicated in {owner.relative_path}")

        marker_owner = owner_matches[0].group(1).strip()
        if marker_owner != owner.owner_id:
            _fail(
                f"owner marker mismatch in {owner.relative_path}: "
                f"expected {owner.owner_id}, got {marker_owner}"
            )

        master_matches = list(MASTER_IDS_MARKER_RE.finditer(text))
        if not master_matches:
            _fail(f"master-ID marker is missing in {owner.relative_path}")
        if len(master_matches) != 1:
            _fail(f"master-ID marker is duplicated in {owner.relative_path}")
        master_ids = master_matches[0].group(1).split()
        ids_for_owner: set[str] = set()
        for master_id in master_ids:
            if not IDENTIFIER_RE.fullmatch(master_id):
                _fail(
                    f"malformed master ID in {owner.relative_path}: {master_id!r}"
                )
            previous_owner = seen_master_ids.get(master_id)
            if previous_owner is not None:
                _fail(
                    f"master ID belongs to multiple owners: {master_id} "
                    f"({previous_owner}, {owner.owner_id})"
                )
            seen_master_ids[master_id] = owner.owner_id
            ids_for_owner.add(master_id)
        master_by_owner[owner.owner_id] = ids_for_owner

    return master_by_owner


def _parse_ledger(
    root: Path, registry_owners: set[str], master_by_owner: Mapping[str, set[str]], final: bool
) -> int:
    ledger_path = root / "docs" / "REQUIREMENTS_LEDGER.md"
    text = _read_text(ledger_path)
    rows = _find_table(text, LEDGER_HEADER, label="ledger", width=len(LEDGER_HEADER))

    master_owner_by_id: dict[str, str] = {}
    for owner, ids in master_by_owner.items():
        for master_id in ids:
            master_owner_by_id[master_id] = owner

    ledger_owner_by_id: dict[str, str] = {}
    for row_number, row in enumerate(rows, start=1):
        requirement_id = _clean_cell(row[0])
        owner_id = _clean_cell(row[1])
        scope = row[2].strip()
        oracle = row[3].strip()
        status = row[4].strip()

        if not IDENTIFIER_RE.fullmatch(requirement_id):
            _fail(f"malformed ledger ID on row {row_number}: {requirement_id!r}")
        if not IDENTIFIER_RE.fullmatch(owner_id):
            _fail(f"malformed ledger owner on row {row_number}: {owner_id!r}")
        for field_name, value in (
            ("scope", scope),
            ("oracle", oracle),
            ("status", status),
        ):
            if not value:
                _fail(f"empty ledger {field_name} for {requirement_id}")
        if status not in VALID_STATUSES:
            _fail(f"invalid ledger status for {requirement_id}: {status!r}")
        if owner_id not in registry_owners:
            _fail(f"ledger owner is unregistered for {requirement_id}: {owner_id}")
        if requirement_id in ledger_owner_by_id:
            _fail(f"duplicate ledger ID: {requirement_id}")
        ledger_owner_by_id[requirement_id] = owner_id

        master_owner = master_owner_by_id.get(requirement_id)
        if master_owner is None:
            _fail(f"ledger orphan (not listed by an owner marker): {requirement_id}")
        if master_owner != owner_id:
            _fail(
                f"ledger owner mismatch for {requirement_id}: "
                f"marker={master_owner}, ledger={owner_id}"
            )
        if final and status != "verified":
            _fail(f"final validation requires verified status for {requirement_id}")

    for requirement_id, owner_id in master_owner_by_id.items():
        if requirement_id not in ledger_owner_by_id:
            _fail(
                f"master ID without ledger row: {requirement_id} "
                f"(owner {owner_id})"
            )
    return len(rows)


def validate(root: Path | str, *, final: bool = False) -> ValidationReport:
    """Validate one repository root, raising on the first structural error."""

    root_path = Path(root).expanduser()
    if not root_path.exists() or not root_path.is_dir():
        _fail(f"root is not a directory: {root_path}")
    root_path = root_path.resolve()
    owners = _parse_registry(root_path)
    master_by_owner = _parse_owner_documents(root_path, owners)
    requirements = _parse_ledger(
        root_path, {owner.owner_id for owner in owners}, master_by_owner, final
    )
    return ValidationReport(len(owners), requirements, final)


def _default_root() -> Path:
    return Path(__file__).resolve().parents[1]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate the requirement authority registry and ledger."
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=None,
        help="repository root (default: the parent of scripts/)",
    )
    parser.add_argument(
        "--final",
        action="store_true",
        help="require every ledger row to have status verified",
    )
    args = parser.parse_args(argv)
    root = args.root if args.root is not None else _default_root()
    try:
        report = validate(root, final=args.final)
    except AuthorityValidationError as exc:
        print(f"requirements-authority: FAIL: {exc}", file=sys.stderr)
        return 1
    except OSError as exc:
        print(f"requirements-authority: FAIL: {exc}", file=sys.stderr)
        return 1

    suffix = " (final)" if report.final else ""
    print(
        f"requirements-authority: PASS{suffix} "
        f"owners={report.owners} requirements={report.requirements}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

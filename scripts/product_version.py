#!/usr/bin/env python3
"""Validate and bump the product version in all release inputs.

The Windows release workflow has three copies of the product version today:
the Rust package manifest, the generated Cargo lockfile entry for the root
package, and the Windows MSBuild property.  This module deliberately parses
only the release-owned fields in those files.  It never rewrites a dependency
lockfile wholesale, and it prepares every replacement before touching any
target file.
"""

from __future__ import annotations

import argparse
import os
import re
import stat
import sys
import tempfile
import xml.etree.ElementTree as ET  # nosec B405  # nosemgrep
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path

VERSION_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$",
    re.ASCII,
)
_LINE_END = re.compile(r"(?:\r\n|\n|\r)$")
_PACKAGE_HEADER = re.compile(r"^[ \t]*\[package\][ \t]*(?:#.*)?$")
_TOML_NAME = re.compile(
    r'^[ \t]*name[ \t]*=[ \t]*"([^"]*)"[ \t]*(?:#.*)?$',
    re.ASCII,
)
_TOML_VERSION = re.compile(
    r'^[ \t]*version[ \t]*=[ \t]*"([^"]*)"[ \t]*(?:#.*)?$',
    re.ASCII,
)
_LOCK_PACKAGE_HEADER = re.compile(r"^[ \t]*\[\[package\]\][ \t]*(?:#.*)?$")
_PROPS_VERSION_ELEMENT = re.compile(
    r"<(?:[A-Za-z_][A-Za-z0-9_.-]*:)?Version(?:\s[^>]*)?>"
    r"(?P<value>[^<]*)"
    r"</(?:[A-Za-z_][A-Za-z0-9_.-]*:)?Version\s*>",
    re.ASCII,
)
_WINDOWS_PROPS_MAX_BYTES = 64 * 1024
_WINDOWS_PROPS_MAX_DEPTH = 16
_UNSAFE_XML_DECLARATION = re.compile(
    r"<!\s*(?:DOCTYPE|ENTITY)\b",
    re.IGNORECASE | re.ASCII,
)


class ProductVersionError(RuntimeError):
    """A fail-closed validation or transaction error."""


@dataclass(frozen=True)
class VersionPaths:
    cargo_toml: Path
    cargo_lock: Path
    windows_props: Path

    def ordered(self) -> tuple[Path, Path, Path]:
        return self.cargo_toml, self.cargo_lock, self.windows_props


@dataclass(frozen=True)
class ParsedFile:
    path: Path
    original: bytes
    version: str
    replacement: bytes
    mode: int
    source_text: str
    has_bom: bool
    value_start: int
    value_end: int

    def with_version(self, version: str) -> ParsedFile:
        replacement_text = (
            self.source_text[: self.value_start]
            + version
            + self.source_text[self.value_end :]
        )
        return ParsedFile(
            path=self.path,
            original=self.original,
            version=version,
            replacement=_encode_utf8(replacement_text, self.has_bom),
            mode=self.mode,
            source_text=self.source_text,
            has_bom=self.has_bom,
            value_start=self.value_start,
            value_end=self.value_end,
        )


@dataclass(frozen=True)
class VersionResult:
    previous: str
    current: str


def _stable_version(value: str, source: Path) -> str:
    if not isinstance(value, str) or VERSION_PATTERN.fullmatch(value) is None:
        raise ProductVersionError(
            f"{source}: version must be a stable canonical X.Y.Z value: {value!r}"
        )
    return value


def _decode_utf8(data: bytes, path: Path) -> tuple[str, bool]:
    has_bom = data.startswith(b"\xef\xbb\xbf")
    try:
        text = data.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise ProductVersionError(f"{path}: file is not valid UTF-8") from exc
    return text, has_bom


def _encode_utf8(text: str, has_bom: bool) -> bytes:
    encoded = text.encode("utf-8")
    return b"\xef\xbb\xbf" + encoded if has_bom else encoded


def _line_records(text: str) -> list[tuple[int, str]]:
    """Return (start offset, line including its terminator) records."""

    records: list[tuple[int, str]] = []
    offset = 0
    for line in text.splitlines(keepends=True):
        records.append((offset, line))
        offset += len(line)
    if offset < len(text):
        records.append((offset, text[offset:]))
    if not records and text == "":
        records.append((0, ""))
    return records


def _line_body(line: str) -> str:
    return _LINE_END.sub("", line)


def _replace_line_value(
    text: str,
    offset: int,
    match: re.Match[str],
    new_value: str,
) -> str:
    start = offset + match.start(1)
    end = offset + match.end(1)
    return text[:start] + new_value + text[end:]


def _read_regular(path: Path) -> tuple[bytes, int]:
    if not path.exists():
        raise ProductVersionError(f"{path}: file was not found")
    if path.is_symlink() or not path.is_file():
        raise ProductVersionError(f"{path}: target must be a regular non-symlink file")
    try:
        data = path.read_bytes()
        mode = stat.S_IMODE(path.stat().st_mode)
    except OSError as exc:
        raise ProductVersionError(f"{path}: could not read file: {exc}") from exc
    return data, mode


def _parse_cargo_toml(path: Path) -> ParsedFile:
    original, mode = _read_regular(path)
    text, has_bom = _decode_utf8(original, path)
    records = _line_records(text)

    package_sections = 0
    in_package = False
    names: list[tuple[int, str, re.Match[str]]] = []
    versions: list[tuple[int, str, re.Match[str]]] = []
    for offset, line in records:
        body = _line_body(line)
        if body.lstrip().startswith("["):
            if _PACKAGE_HEADER.fullmatch(body):
                package_sections += 1
                in_package = True
            else:
                in_package = False
            continue
        if not in_package:
            continue
        if re.match(r"^[ \t]*(?:name|version)\b", body) is not None and not (
            _TOML_NAME.fullmatch(body) or _TOML_VERSION.fullmatch(body)
        ):
            raise ProductVersionError(
                f"{path}: [package] name/version entry is malformed"
            )
        name_match = _TOML_NAME.fullmatch(body)
        if name_match is not None:
            names.append((offset, body, name_match))
        version_match = _TOML_VERSION.fullmatch(body)
        if version_match is not None:
            versions.append((offset, body, version_match))

    if package_sections != 1:
        raise ProductVersionError(
            f"{path}: Cargo.toml must contain exactly one [package] table"
        )
    if len(names) != 1 or names[0][2].group(1) != "codex_info":
        raise ProductVersionError(
            f"{path}: [package].name must contain exactly one codex_info value"
        )
    if len(versions) != 1:
        raise ProductVersionError(
            f"{path}: [package].version must contain exactly one quoted value"
        )

    value = _stable_version(versions[0][2].group(1), path)
    replacement_text = _replace_line_value(text, versions[0][0], versions[0][2], value)
    value_start = versions[0][0] + versions[0][2].start(1)
    value_end = versions[0][0] + versions[0][2].end(1)
    return ParsedFile(
        path=path,
        original=original,
        version=value,
        replacement=_encode_utf8(replacement_text, has_bom),
        mode=mode,
        source_text=text,
        has_bom=has_bom,
        value_start=value_start,
        value_end=value_end,
    )


def _parse_cargo_lock(path: Path) -> ParsedFile:
    original, mode = _read_regular(path)
    text, has_bom = _decode_utf8(original, path)
    records = _line_records(text)
    package_starts = [
        index
        for index, (_, line) in enumerate(records)
        if _LOCK_PACKAGE_HEADER.fullmatch(_line_body(line))
    ]
    if not package_starts:
        raise ProductVersionError(f"{path}: Cargo.lock has no [[package]] tables")

    roots: list[tuple[int, re.Match[str]]] = []
    for start_number, start in enumerate(package_starts):
        end = (
            package_starts[start_number + 1]
            if start_number + 1 < len(package_starts)
            else len(records)
        )
        block_names: list[tuple[int, re.Match[str]]] = []
        block_versions: list[tuple[int, re.Match[str]]] = []
        for index in range(start + 1, end):
            offset, line = records[index]
            body = _line_body(line)
            if re.match(r"^[ \t]*(?:name|version)\b", body) is not None and not (
                _TOML_NAME.fullmatch(body) or _TOML_VERSION.fullmatch(body)
            ):
                raise ProductVersionError(
                    f"{path}: [[package]] name/version entry is malformed"
                )
            name_match = _TOML_NAME.fullmatch(body)
            if name_match is not None:
                block_names.append((offset, name_match))
            version_match = _TOML_VERSION.fullmatch(body)
            if version_match is not None:
                block_versions.append((offset, version_match))
        if len(block_names) != 1 or len(block_versions) != 1:
            raise ProductVersionError(
                f"{path}: every [[package]] table must contain one name and one version"
            )
        name = block_names[0][1].group(1)
        if name == "codex_info":
            roots.append((block_versions[0][0], block_versions[0][1]))

    if len(roots) != 1:
        raise ProductVersionError(
            f"{path}: Cargo.lock must contain exactly one root codex_info package"
        )

    version_offset, version_match = roots[0]
    value = _stable_version(version_match.group(1), path)
    replacement_text = _replace_line_value(text, version_offset, version_match, value)
    value_start = version_offset + version_match.start(1)
    value_end = version_offset + version_match.end(1)
    return ParsedFile(
        path=path,
        original=original,
        version=value,
        replacement=_encode_utf8(replacement_text, has_bom),
        mode=mode,
        source_text=text,
        has_bom=has_bom,
        value_start=value_start,
        value_end=value_end,
    )


def _local_name(tag: object) -> str:
    if not isinstance(tag, str):
        return ""
    return tag.rsplit("}", 1)[-1]


def _validate_windows_props_depth(root: ET.Element, path: Path) -> None:
    stack = [(root, 1)]
    while stack:
        node, depth = stack.pop()
        if depth > _WINDOWS_PROPS_MAX_DEPTH:
            raise ProductVersionError(
                f"{path}: Directory.Build.props exceeds the XML depth limit"
            )
        stack.extend((child, depth + 1) for child in list(node))


def _parse_windows_props(path: Path) -> ParsedFile:
    original, mode = _read_regular(path)
    if len(original) > _WINDOWS_PROPS_MAX_BYTES:
        raise ProductVersionError(
            f"{path}: Directory.Build.props exceeds the XML byte limit"
        )
    text, has_bom = _decode_utf8(original, path)
    if _UNSAFE_XML_DECLARATION.search(text) is not None:
        raise ProductVersionError(
            f"{path}: DTD and entity declarations are not allowed"
        )
    try:
        # The byte, declaration, and depth boundaries above and below make this
        # dependency-free parser safe for the release-owned local props file.
        root = ET.fromstring(text)  # nosec B314  # nosemgrep
    except ET.ParseError as exc:
        raise ProductVersionError(f"{path}: Directory.Build.props is not valid XML") from exc
    _validate_windows_props_depth(root, path)
    if _local_name(root.tag) != "Project":
        raise ProductVersionError(f"{path}: XML root must be Project")

    nodes: list[ET.Element] = []
    for group in list(root):
        if _local_name(group.tag) != "PropertyGroup":
            continue
        for node in list(group):
            if _local_name(node.tag) == "Version":
                nodes.append(node)
    if len(nodes) != 1:
        raise ProductVersionError(
            f"{path}: Project/PropertyGroup/Version must contain exactly one element"
        )
    node = nodes[0]
    if len(list(node)) != 0:
        raise ProductVersionError(f"{path}: Version element must contain plain text")
    value = _stable_version((node.text or "").strip(), path)

    candidates = list(_PROPS_VERSION_ELEMENT.finditer(text))
    if len(candidates) != 1:
        raise ProductVersionError(
            f"{path}: Version element could not be located without rewriting XML"
        )
    candidate = candidates[0]
    if candidate.group("value").strip() != value:
        raise ProductVersionError(f"{path}: Version XML text is ambiguous")
    replacement_text = text[: candidate.start("value")] + value + text[candidate.end("value") :]
    return ParsedFile(
        path=path,
        original=original,
        version=value,
        replacement=_encode_utf8(replacement_text, has_bom),
        mode=mode,
        source_text=text,
        has_bom=has_bom,
        value_start=candidate.start("value"),
        value_end=candidate.end("value"),
    )


def _parse_all(paths: VersionPaths) -> tuple[ParsedFile, ...]:
    resolved = tuple(path.absolute() for path in paths.ordered())
    normalized = [os.path.normcase(os.path.normpath(str(path))) for path in resolved]
    if len(set(normalized)) != len(normalized):
        raise ProductVersionError("version targets must be three distinct files")
    parsed = (
        _parse_cargo_toml(resolved[0]),
        _parse_cargo_lock(resolved[1]),
        _parse_windows_props(resolved[2]),
    )
    values = {item.version for item in parsed}
    if len(values) != 1:
        detail = ", ".join(f"{item.path}={item.version}" for item in parsed)
        raise ProductVersionError(f"version targets are not synchronized: {detail}")
    return parsed


def _increment_decimal(value: str) -> str:
    digits = list(value)
    carry = 1
    for index in range(len(digits) - 1, -1, -1):
        digit = ord(digits[index]) - ord("0") + carry
        if digit == 10:
            digits[index] = "0"
            carry = 1
        else:
            digits[index] = chr(ord("0") + digit)
            carry = 0
            break
    if carry:
        return "1" + "".join(digits)
    return "".join(digits)


def _next_patch(version: str) -> str:
    major, minor, patch = version.split(".")
    return f"{major}.{minor}.{_increment_decimal(patch)}"


def next_version(version: str) -> str:
    """Return the next patch version without changing any file."""

    return _next_patch(_stable_version(version, Path("--version")))


def _write_staged(path: Path, data: bytes, mode: int, label: str) -> Path:
    fd, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.{label}-", dir=str(path.parent)
    )
    temporary = Path(temporary_name)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        try:
            temporary.unlink()
        except OSError:
            pass
        raise
    return temporary


def _cleanup(paths: Iterable[Path]) -> None:
    for path in paths:
        try:
            path.unlink()
        except FileNotFoundError:
            pass
        except OSError:
            # Cleanup cannot make a target file inconsistent.  Do not mask the
            # original validation or replacement error with a temp-file error.
            pass


def _atomic_replace_all(
    parsed: Sequence[ParsedFile],
    verify: Callable[[], None],
) -> None:
    """Replace all targets, rolling back already-replaced targets on failure."""

    staged_new: list[Path] = []
    staged_backup: list[Path] = []
    replaced: list[int] = []
    try:
        # All target bytes are already in memory.  Stage both the new bytes and
        # an on-disk rollback copy before the first target is replaced.
        for item in parsed:
            staged_new.append(_write_staged(item.path, item.replacement, item.mode, "new"))
            staged_backup.append(_write_staged(item.path, item.original, item.mode, "old"))

        for index, item in enumerate(parsed):
            if item.path.is_symlink() or not item.path.is_file():
                raise ProductVersionError(f"{item.path}: target changed during bump")
            try:
                current = item.path.read_bytes()
            except OSError as exc:
                raise ProductVersionError(
                    f"{item.path}: target changed during bump: {exc}"
                ) from exc
            if current != item.original:
                raise ProductVersionError(f"{item.path}: target changed during bump")
            replaced.append(index)
            os.replace(staged_new[index], item.path)

        verify()
    except BaseException:
        rollback_error: BaseException | None = None
        for index in reversed(replaced):
            try:
                os.replace(staged_backup[index], parsed[index].path)
            except OSError as exc:  # pragma: no cover - filesystem dependent
                rollback_error = exc
                break
        if rollback_error is not None:
            raise ProductVersionError(
                "version replacement failed and rollback could not be completed"
            ) from rollback_error
        raise
    finally:
        _cleanup(staged_new)
        _cleanup(staged_backup)


def check_versions(paths: VersionPaths) -> str:
    """Validate the three release inputs and return their common version."""

    return _parse_all(paths)[0].version


def bump_versions(paths: VersionPaths, expected: str) -> VersionResult:
    """Bump only the patch component after an exact expected-version check."""

    expected = _stable_version(expected, Path("--expected"))
    parsed = _parse_all(paths)
    previous = parsed[0].version
    if previous != expected:
        raise ProductVersionError(
            f"expected previous version {expected}, but synchronized version is {previous}"
        )
    current = _next_patch(previous)
    next_files = tuple(item.with_version(current) for item in parsed)

    # The parser-level replacement above is deliberately ASCII-only: all
    # accepted version strings are canonical ASCII digits and dots.  Verify
    # each complete transformed file before staging anything.
    for item in next_files:
        if item.replacement == item.original:
            raise ProductVersionError(f"{item.path}: version replacement was not prepared")
        if item.replacement.count(current.encode("ascii")) < 1:
            raise ProductVersionError(f"{item.path}: transformed version is not present")

    def verify() -> None:
        verified = _parse_all(paths)
        if verified[0].version != current:
            raise ProductVersionError(
                f"version replacement verification failed: expected {current}"
            )

    _atomic_replace_all(next_files, verify)
    return VersionResult(previous=previous, current=current)


def _default_paths() -> VersionPaths:
    root = Path(__file__).resolve().parents[1]
    return VersionPaths(
        cargo_toml=root / "Cargo.toml",
        cargo_lock=root / "Cargo.lock",
        windows_props=root / "windows-client" / "Directory.Build.props",
    )


def _add_path_arguments(parser: argparse.ArgumentParser, *, suppress_defaults: bool) -> None:
    default = argparse.SUPPRESS if suppress_defaults else None
    parser.add_argument("--cargo-toml", type=Path, default=default)
    parser.add_argument("--cargo-lock", type=Path, default=default)
    parser.add_argument("--windows-props", type=Path, default=default)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Validate or patch the synchronized product version inputs."
    )
    _add_path_arguments(parser, suppress_defaults=False)
    commands = parser.add_subparsers(dest="command", required=True)

    check = commands.add_parser("check", help="validate synchronization without writing")
    _add_path_arguments(check, suppress_defaults=True)

    next_command = commands.add_parser("next", help="print the next patch version")
    next_command.add_argument("--version", required=True, help="stable X.Y.Z version")

    bump = commands.add_parser("bump", help="increment patch after an exact expected version")
    _add_path_arguments(bump, suppress_defaults=True)
    bump.add_argument("--expected", required=True, help="expected synchronized X.Y.Z version")
    return parser


def _paths_from_arguments(arguments: argparse.Namespace) -> VersionPaths:
    defaults = _default_paths()
    return VersionPaths(
        cargo_toml=Path(getattr(arguments, "cargo_toml", None) or defaults.cargo_toml),
        cargo_lock=Path(getattr(arguments, "cargo_lock", None) or defaults.cargo_lock),
        windows_props=Path(getattr(arguments, "windows_props", None) or defaults.windows_props),
    )


def main(argv: Sequence[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    paths = _paths_from_arguments(arguments)
    try:
        if arguments.command == "check":
            version = check_versions(paths)
            print(f"version={version}")
            print("synchronized=true")
            return 0
        if arguments.command == "next":
            print(f"version={next_version(arguments.version)}")
            return 0
        result = bump_versions(paths, arguments.expected)
        print(f"previous_version={result.previous}")
        print(f"version={result.current}")
        print("changed=true")
        print("major_minor_unchanged=true")
        print("synchronized=true")
        return 0
    except ProductVersionError as exc:
        print(f"product-version: ERROR: {exc}", file=sys.stderr)
        return 1
    except (OSError, ValueError, ET.ParseError) as exc:
        # Filesystem and parser surprises remain fail-closed at the CLI
        # boundary.  Do not print a traceback into CI logs.
        print(f"product-version: ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

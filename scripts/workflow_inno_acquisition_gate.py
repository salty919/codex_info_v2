#!/usr/bin/env python3
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only

"""Keep the Inno acquisition step finite and free of duplicate version oracles."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
from typing import Sequence


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "windows-client.yml"
STEP_START = "      - name: Install locked Inno Setup compiler\n"
STEP_END = "      - name: Build standard Windows setup wizard\n"

REQUIRED_MARKERS = (
    "https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe",
    "0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f",
    "Invoke-WebRequest -Uri $installerUrl -OutFile $installer",
    "Get-FileHash -LiteralPath $installer -Algorithm SHA256",
    "Get-AuthenticodeSignature -LiteralPath $installer",
    "[System.Management.Automation.SignatureStatus]::Valid",
    "[System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName",
    "if ($publisher -cne 'Pyrsys B.V.')",
    "Start-Process -FilePath $installer",
    "@('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER')",
    "Join-Path $env:LOCALAPPDATA 'Programs\\Inno Setup 7\\ISCC.exe'",
    '"INNO_SETUP_COMPILER=$compiler"',
)

ALLOWED_THROW_FRAGMENTS = (
    "Inno Setup installer SHA-256 mismatch:",
    "Inno Setup installer Authenticode status is",
    "Unexpected Inno Setup installer publisher:",
    "Inno Setup installer failed with exit code",
    "Inno Setup 7.1.0 compiler was not installed.",
)

FORBIDDEN_SECOND_ORACLES = (
    "VersionInfo",
    "FileVersion",
    "FileMajorPart",
    "FileMinorPart",
    "FileBuildPart",
    "Get-ItemPropertyValue",
    "CurrentVersion\\Uninstall",
    "DisplayName",
    "HKCU:",
    "HKLM:",
)


def _step(workflow: str) -> str:
    if workflow.count(STEP_START) != 1 or workflow.count(STEP_END) != 1:
        raise ValueError("Inno acquisition step boundary is not unique")
    start = workflow.index(STEP_START) + len(STEP_START)
    end = workflow.index(STEP_END, start)
    return workflow[start:end]


def validate(workflow: str) -> list[str]:
    errors: list[str] = []
    try:
        step = _step(workflow)
    except ValueError as error:
        return [str(error)]

    for marker in REQUIRED_MARKERS:
        count = step.count(marker)
        if count != 1:
            errors.append(f"required acquisition contract {marker!r}: expected 1, found {count}")

    throw_lines = [
        line.strip() for line in step.splitlines() if re.search(r"\bthrow\b", line)
    ]
    throw_count = len(re.findall(r"\bthrow\b", step))
    if throw_count != len(ALLOWED_THROW_FRAGMENTS):
        errors.append(
            "Inno acquisition has an unreviewed failure predicate: "
            f"expected {len(ALLOWED_THROW_FRAGMENTS)} throw statements, found {throw_count}"
        )
    for fragment in ALLOWED_THROW_FRAGMENTS:
        count = sum(fragment in line for line in throw_lines)
        if count != 1:
            errors.append(f"allowed failure contract {fragment!r}: expected 1, found {count}")
    for line in throw_lines:
        if len(re.findall(r"\bthrow\b", line)) != 1:
            errors.append(f"multiple failure predicates share one line: {line}")
        if not any(fragment in line for fragment in ALLOWED_THROW_FRAGMENTS):
            errors.append(f"unapproved Inno acquisition failure predicate: {line}")

    condition_count = len(re.findall(r"(?im)(?:^|;)\s*if\s*\(", step))
    if condition_count != len(ALLOWED_THROW_FRAGMENTS):
        errors.append(
            "Inno acquisition conditional set changed: "
            f"expected {len(ALLOWED_THROW_FRAGMENTS)}, found {condition_count}"
        )

    for marker in FORBIDDEN_SECOND_ORACLES:
        if marker in step:
            errors.append(f"duplicate or non-authoritative version oracle remains: {marker}")

    if step.count("$compiler") != 3:
        errors.append(
            "compiler path must only be assigned, existence-checked, and exported"
        )
    if re.search(r"(?i)\b(?:for|foreach|while|do|switch|try|catch)\b", step):
        errors.append("retry, fallback, or alternate control flow remains in Inno acquisition")
    if re.search(
        r"(?i)\b(?:write-error|exit\s+[1-9][0-9]*|return\s+[1-9][0-9]*)\b",
        step,
    ):
        errors.append("an unapproved non-throw failure path remains in Inno acquisition")
    return errors


def self_test(workflow: str) -> int:
    cases = 1
    insertion = '          "INNO_SETUP_COMPILER=$compiler"'
    mutations = (
        (
            "registry version oracle",
            "          $display = Get-ItemPropertyValue 'HKCU:\\Software\\Example' DisplayName\n",
        ),
        (
            "file version oracle",
            "          $version = (Get-Item -LiteralPath $compiler).VersionInfo.FileVersion\n",
        ),
        (
            "extra failure predicate",
            "          if ($true) { throw 'redundant check' }\n",
        ),
        (
            "same-line extra failure predicate",
            "          if ($true) { throw 'first'; throw 'second' }\n",
        ),
        (
            "same-line throwless conditional",
            "          $null = 1; if ($true) { $null = 1 }\n",
        ),
        (
            "retry loop",
            "          foreach ($attempt in 1..2) { Invoke-WebRequest -Uri $installerUrl -OutFile $installer }\n",
        ),
        (
            "same-line retry loop",
            "          $null = 1; foreach ($attempt in 1..2) { $null = $attempt }\n",
        ),
    )
    for label, addition in mutations:
        candidate = workflow.replace(insertion, addition + insertion, 1)
        if not validate(candidate):
            raise AssertionError(f"overcheck mutation was accepted: {label}")
        cases += 1

    candidate = workflow.replace(
        "Get-FileHash -LiteralPath $installer -Algorithm SHA256",
        "Get-FileHash -LiteralPath $installer -Algorithm SHA1",
        1,
    )
    if not validate(candidate):
        raise AssertionError("required SHA-256 contract removal was accepted")
    return cases + 1


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    workflow = WORKFLOW.read_text(encoding="utf-8")
    errors = validate(workflow)
    if errors:
        for error in errors:
            print(f"workflow-inno-acquisition-gate: FAIL {error}")
        return 1
    cases = self_test(workflow) if args.self_test else 1
    print(f"workflow-inno-acquisition-gate: PASS cases={cases}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

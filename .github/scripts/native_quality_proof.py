#!/usr/bin/env python3
# Copyright (C) 2026 salty919
# SPDX-License-Identifier: GPL-3.0-only
"""Data-only, exact-input native unit/Clippy reuse with fresh fallback."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess  # nosec B404 # required tool API; individual execution calls remain reviewed.
import tempfile
import zipfile
from pathlib import Path

from defusedxml import ElementTree as ET
from defusedxml.common import DefusedXmlException

MAX_ARCHIVE = 16 * 1024 * 1024
MAX_REPORT = 32 * 1024 * 1024
ATTRIBUTE = "codex-native-proof"
REPORT = Path("artifacts/codacy-coverage-rust/rust.cobertura.xml")
JOB = "Run selected advisory quality / linux-backend-quality / native-quality"


def command(*args: str, data: bytes | None = None, env=None) -> bytes:
    if not args or args[0] not in {"git", "gh", "rustc", "cargo", "dpkg-query"}:
        raise ValueError("native proof program is not allowed")
    executable = shutil.which(args[0])
    if executable is None or not Path(executable).is_absolute():
        raise ValueError("native proof program has no absolute executable")
    return subprocess.check_output(
        [executable, *args[1:]], input=data, env=env, shell=False,
        stderr=subprocess.DEVNULL, timeout=30,
    )


def oid(value: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError("native proof object ID is malformed")
    return value


def api(repository: str, suffix: str):
    result = json.loads(command("gh", "api", f"repos/{repository}/{suffix}"))
    if not isinstance(result, dict):
        raise ValueError("GitHub API response is not an object")
    return result


def runtime() -> dict:
    image_os = os.environ.get("ImageOS", "")
    image_version = os.environ.get("ImageVersion", "")
    if not image_os or not image_version:
        raise ValueError("runner image identity is unavailable")
    return {"rustc": command("rustc", "-Vv").decode().strip(),
            "cargo": command("cargo", "-V").decode().strip(),
            "clippy": command("cargo", "clippy", "-V").decode().strip(),
            "coverage": command("cargo", "llvm-cov", "-V").decode().strip(),
            "image_os": image_os, "image_version": image_version,
            "packages": hashlib.sha256(command("dpkg-query", "--show", "--showformat=${binary:Package}=${Version}\n")).hexdigest(),
            "environment": {key: hashlib.sha256(value.encode()).hexdigest()
                            for key, value in sorted(os.environ.items())
                            if key.startswith(("CODEX_INFO_", "RUST", "CARGO", "LLVM"))
                            or key in {"CC", "CXX"}}}


def current_tree() -> str:
    source = oid(os.environ["SOURCE_SHA"])
    if command("git", "rev-parse", "HEAD").decode().strip() != source:
        raise ValueError("native checkout no longer matches the exact source")
    changed = set(command("git", "diff", "--name-only", "HEAD", "--").decode().splitlines())
    files = {"Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props"}
    if not changed.issubset(files):
        raise ValueError("native source has an unexpected tracked change")
    with tempfile.TemporaryDirectory(prefix="codex-native-tree-") as temporary:
        environment = {**os.environ, "GIT_INDEX_FILE": str(Path(temporary) / "index")}
        command("git", "read-tree", "HEAD", env=environment)
        command("git", "add", "--", *sorted(files), env=environment)
        return command("git", "write-tree", env=environment).decode().strip()


def expected(tree: str | None = None, with_runtime=True) -> dict:
    result = {"tree": tree or current_tree(), "main_base": oid(os.environ["MAIN_BASE_SHA"]),
              "workflow": oid(os.environ["TRUSTED_SHA"])}
    if with_runtime:
        result["runtime"] = runtime()
    return result


def read_xml(data: bytes):
    # cargo-llvm-cov 0.9.0 emits this fixed, external Cobertura declaration.
    # Remove it before parsing; no DTD is ever fetched or interpreted. Every
    # other DTD and every ENTITY declaration remains forbidden.
    declaration = b'<!DOCTYPE coverage SYSTEM "https://cobertura.sourceforge.net/xml/coverage-04.dtd">'
    if not data or len(data) > MAX_REPORT:
        raise ValueError("coverage XML is out of bounds")
    # Inspect the same decoded text that is parsed. UTF-16/32 declarations
    # must not bypass the DTD/ENTITY check through interleaved NUL bytes.
    text = data.decode("utf-8-sig")
    if "\x00" in text:
        raise ValueError("coverage XML must be UTF-8 without NUL bytes")
    text = text.replace(declaration.decode("ascii"), "", 1)
    if re.search(r"<!\s*(DOCTYPE|ENTITY)\b", text, re.I):
        raise ValueError("coverage XML is unsafe or out of bounds")
    try:
        root = ET.fromstring(text, forbid_dtd=True, forbid_entities=True, forbid_external=True)
    except DefusedXmlException as error:
        raise ValueError("coverage XML contains a forbidden declaration") from error
    if root.tag != "coverage" or not re.fullmatch(r"[1-9][0-9]*", root.get("lines-valid", "")):
        raise ValueError("coverage report has no valid lines")
    if any(re.match(r"(?:/|[A-Za-z]:[\\/])", node.get("filename", "")) for node in root.iter("class")):
        raise ValueError("coverage report has a runner-specific filename")
    return root


def read_proof(data: bytes) -> dict:
    value = read_xml(data).get(ATTRIBUTE)
    if value is None or len(value) > 65536:
        raise ValueError("native proof is absent or out of bounds")
    result = json.loads(value)
    if not isinstance(result, dict):
        raise ValueError("native proof is not an object")
    return result


def matches(proof: dict, desired: dict) -> bool:
    return (type(proof.get("schema")) is int and proof["schema"] == 1
            and proof.get("kind") == "feat-preflight" and proof.get("main_included") is True
            and all(proof.get(key) == value for key, value in desired.items()))


def producer_matches(proof: dict, run: dict, jobs: list, repository: str) -> bool:
    required = {"status": "completed", "conclusion": "success", "event": "pull_request_target",
                "path": ".github/workflows/feat-integration.yml", "id": proof.get("run"),
                "run_attempt": proof.get("attempt"), "head_sha": proof.get("source")}
    if any(run.get(key) != value for key, value in required.items()):
        return False
    if run.get("repository", {}).get("full_name") != repository:
        return False
    if run.get("display_title") != f"codex-feat-preflight-v1:pr={proof.get('pr')}:event_head={proof.get('source')}":
        return False
    for name in ("rust.yml", "selective-quality.yml"):
        wanted = f"{repository}/.github/workflows/{name}@{proof.get('workflow')}"
        if not any(item.get("path") == wanted and item.get("sha") == proof.get("workflow")
                   for item in run.get("referenced_workflows", [])):
            return False
    native = [job for job in jobs if job.get("name") == JOB]
    if not (len(native) == 1 and native[0].get("status") == "completed"
            and native[0].get("conclusion") == "success"):
        return False
    for name in ("Reconstruct the trusted planned release tree", "Run native unit tests with coverage",
                 "Reject native compiler and Clippy warnings", "Bind successful native checks to exact inputs"):
        steps = [item for item in native[0].get("steps", []) if item.get("name") == name]
        if len(steps) != 1 or steps[0].get("status") != "completed" or steps[0].get("conclusion") != "success":
            return False
    return True


def report_from_archive(data: bytes, digest: str) -> bytes:
    if len(data) > MAX_ARCHIVE or digest != "sha256:" + hashlib.sha256(data).hexdigest():
        raise ValueError("artifact digest or size differs")
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        if (len(entries) != 1 or entries[0].filename != "rust.cobertura.xml"
                or entries[0].file_size > MAX_REPORT or entries[0].flag_bits & 1):
            raise ValueError("artifact must contain one bounded coverage XML file")
        result = archive.read(entries[0])
    read_xml(result)
    return result


def output(name: str, value: str) -> None:
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        stream.write(f"{name}={value}\n")


def resolve() -> None:
    # Never fall back from the latest matching run to an older successful one.
    # A bounded search miss, unavailable API, or legacy run means fresh checks.
    repository = os.environ["REPOSITORY"]
    source = oid(os.environ["SOURCE_SHA"])
    tree = command("git", "rev-parse", f"{source}^{{tree}}").decode().strip()
    desired = expected(tree=tree, with_runtime=False)
    current_main = api(repository, "git/ref/heads/main").get("object", {}).get("sha")
    if current_main != desired["main_base"]:
        print("native-reuse: actual main advanced beyond the event base; fresh checks required")
        return
    # -I excludes script directories: load only the sibling of this trusted helper.
    spec = importlib.util.spec_from_file_location(
        "trusted_release_preflight", Path(__file__).resolve().with_name("release_preflight.py"),
    )
    if spec is None or spec.loader is None:
        raise ValueError("trusted release planner is unavailable")
    preflight = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(preflight)
    runs = api(repository, "actions/workflows/feat-integration.yml/runs?event=pull_request_target&per_page=30")
    for run in runs.get("workflow_runs", []):
        head = run.get("head_sha", "")
        if re.fullmatch(r"[0-9a-f]{40}", head) is None:
            continue
        try:
            command("git", "merge-base", "--is-ancestor", head, source)
            snapshot = preflight.plan(head, desired["main_base"], desired["workflow"])
        except (ValueError, RuntimeError, subprocess.SubprocessError):
            continue
        if snapshot["expected_tree"] != desired["tree"] or not snapshot["main_included"]:
            continue
        # This is the newest run for this prospective tree. Missing/non-success
        # evidence stops the search; it cannot authorize an older result.
        if run.get("status") != "completed" or run.get("conclusion") != "success":
            return
        run_id, attempt = run["id"], run["run_attempt"]
        identity = f"codacy-coverage-rust-v1-head-{head}-run-{run_id}-attempt-{attempt}"
        listing = api(repository, f"actions/runs/{run_id}/artifacts?per_page=100")
        if listing.get("total_count", 101) > 100:
            return
        found = [item for item in listing.get("artifacts", []) if item.get("name") == identity]
        if len(found) != 1 or found[0].get("expired") is not False:
            return
        artifact = found[0]
        if not 0 < artifact.get("size_in_bytes", 0) <= MAX_ARCHIVE:
            return
        raw = command("gh", "api", f"repos/{repository}/actions/artifacts/{artifact['id']}/zip")
        report = report_from_archive(raw, artifact.get("digest", ""))
        proof = read_proof(report)
        jobs_response = api(repository, f"actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100")
        if jobs_response.get("total_count", 101) > 100:
            return
        if not matches(proof, desired) or not producer_matches(proof, run, jobs_response.get("jobs", []), repository):
            return
        path = Path(os.environ["RUNNER_TEMP"]) / "native-reuse" / "rust.cobertura.xml"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(report)
        artifact_name = (f"native-reuse-v1-head-{source}-run-{os.environ['GITHUB_RUN_ID']}"
                         f"-attempt-{os.environ['GITHUB_RUN_ATTEMPT']}")
        output("artifact", artifact_name)
        print("native-reuse: verified prior successful producer; runner inputs are checked by the leaf")
        return


def probe() -> None:
    accepted = False
    try:
        desired = expected()
        actual_main = api(os.environ["REPOSITORY"], "git/ref/heads/main").get("object", {}).get("sha")
        if actual_main != desired["main_base"]:
            raise ValueError("actual main changed before the native leaf decision")
        output("inputs_json", json.dumps(desired, separators=(",", ":")))
        data = (Path(os.environ["RUNNER_TEMP"]) / "native-reuse-download" / "rust.cobertura.xml").read_bytes()
        if matches(read_proof(data), desired):
            REPORT.parent.mkdir(parents=True, exist_ok=True)
            REPORT.write_bytes(data)
            accepted = True
    except (ValueError, RuntimeError, OSError, KeyError, subprocess.SubprocessError, ET.ParseError):
        pass
    output("reused", "true" if accepted else "false")
    print("native-reuse: " + ("exact runtime matched" if accepted else "fresh unit/Clippy required"))


def emit() -> bool:
    # Called only after both native tests and Clippy succeed (or accepted reuse).
    try:
        snapshot = json.loads(os.environ.get("PLAN") or "null")
        desired = json.loads(os.environ["NATIVE_INPUTS"])
        if (set(desired) != {"tree", "main_base", "workflow", "runtime"}
                or desired["tree"] != current_tree()
                or desired["main_base"] != os.environ["MAIN_BASE_SHA"]
                or desired["workflow"] != os.environ["TRUSTED_SHA"]):
            raise ValueError("native inputs changed after the checks")
        proof = {"schema": 1, "kind": "feat-preflight" if snapshot else "main-quality",
                 "source": os.environ["SOURCE_SHA"], "pr": int(os.environ["PR_NUMBER"]),
                 "run": int(os.environ["GITHUB_RUN_ID"]), "attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
                 "main_included": bool(snapshot and snapshot.get("main_included") is True), **desired}
        root = read_xml(REPORT.read_bytes())
        root.set(ATTRIBUTE, json.dumps(proof, separators=(",", ":")))
        REPORT.write_bytes(ET.tostring(root, encoding="utf-8", xml_declaration=True))
        return True
    except (ValueError, RuntimeError, OSError, KeyError, subprocess.SubprocessError, ET.ParseError):
        # Coverage still uploads, but absent image/tool identity cannot be reused.
        print("native-reuse: proof unavailable; coverage remains fresh-only")
        return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("resolve", "probe", "emit"))
    args = parser.parse_args()
    if args.mode == "resolve":
        try:
            resolve()
        except (ValueError, RuntimeError, OSError, KeyError, subprocess.SubprocessError,
                ET.ParseError, zipfile.BadZipFile, json.JSONDecodeError, TypeError, AttributeError):
            print("native-reuse: prior evidence unavailable; fresh checks required")
    elif args.mode == "probe":
        probe()
    else:
        return 0 if emit() else 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

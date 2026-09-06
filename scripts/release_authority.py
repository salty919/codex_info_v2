#!/usr/bin/env python3
"""Resolve the one immutable main-quality run and its release candidates."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Mapping, Sequence


SHA = r"[0-9a-f]{40}"
VERSION = r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
WINDOWS_CANDIDATE = re.compile(
    rf"^release-candidate-windows-v1-version-(?P<version>{VERSION})$",
    re.ASCII,
)
LINUX_CANDIDATE = re.compile(
    rf"^release-candidate-linux-v1-version-(?P<version>{VERSION})$",
    re.ASCII,
)
DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$", re.ASCII)
VERSION_INPUT = "windows-client/Directory.Build.props"


class AuthorityError(RuntimeError):
    pass


def _object(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise AuthorityError(f"{label} is not an object")
    return value


def _api(endpoint: str) -> Any:
    try:
        result = subprocess.run(
            (
                "gh",
                "api",
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
                endpoint,
            ),
            text=True,
            capture_output=True,
            check=False,
            timeout=300,
        )
    except subprocess.TimeoutExpired as exc:
        raise AuthorityError(f"GitHub API timed out: {endpoint}") from exc
    if result.returncode != 0:
        raise AuthorityError(result.stderr.strip() or f"GitHub API failed: {endpoint}")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise AuthorityError(f"GitHub API returned malformed JSON: {endpoint}") from exc


def _signal(
    event_name: str, event: Mapping[str, Any]
) -> tuple[int | None, str | None, bool]:
    if event_name == "pull_request_target":
        action = event.get("action")
        pull = _object(event.get("pull_request"), "event pull_request")
        if action != "closed" or pull.get("merged") is not True:
            return None, None, False
        number = pull.get("number")
        head = _object(pull.get("head"), "event pull_request head").get("sha")
        if not (
            isinstance(number, int)
            and number >= 1
            and isinstance(head, str)
            and re.fullmatch(SHA, head)
        ):
            raise AuthorityError("event PR identity is malformed")
        return number, head, True
    if event_name == "workflow_run":
        run = _object(event.get("workflow_run"), "event workflow_run")
        if run.get("conclusion") != "success":
            return None, None, False
        head = run.get("head_sha")
        if run.get("event") != "pull_request_target" or not (
            isinstance(head, str) and re.fullmatch(SHA, head)
        ):
            raise AuthorityError("successful workflow signal identity is inconsistent")
        return None, head, True
    raise AuthorityError(f"unsupported release event: {event_name}")


def _pull_authority(repository: str, number: int, pull: Mapping[str, Any]) -> dict[str, Any]:
    base = _object(pull.get("base"), "pull base")
    head = _object(pull.get("head"), "pull head")
    base_repo = _object(base.get("repo"), "pull base repository")
    head_repo = _object(head.get("repo"), "pull head repository")
    final_head = head.get("sha")
    base_sha = base.get("sha")
    merge_sha = pull.get("merge_commit_sha")
    if not (
        pull.get("number") == number
        and pull.get("state") == "closed"
        and pull.get("merged") is True
        and base.get("ref") == "main"
        and base_repo.get("full_name") == repository
        and head_repo.get("full_name") == repository
        and isinstance(base_sha, str)
        and re.fullmatch(SHA, base_sha)
        and isinstance(final_head, str)
        and re.fullmatch(SHA, final_head)
        and isinstance(merge_sha, str)
        and re.fullmatch(SHA, merge_sha)
    ):
        raise AuthorityError("pull request is not a merged same-repository main authority")
    return {"base_sha": base_sha, "final_head": final_head, "merge_sha": merge_sha}


def _version_input_blob(repository: str, revision: str) -> str:
    value = _object(
        _api(f"repos/{repository}/contents/{VERSION_INPUT}?ref={revision}"),
        "version input",
    )
    blob = value.get("sha")
    if not (
        value.get("type") == "file"
        and isinstance(value.get("size"), int)
        and value["size"] > 0
        and isinstance(blob, str)
        and re.fullmatch(SHA, blob)
    ):
        raise AuthorityError(f"version input identity is malformed at {revision}")
    return blob


def _binary_impact(repository: str, base_sha: str, head_sha: str) -> bool:
    return _version_input_blob(repository, base_sha) != _version_input_blob(
        repository, head_sha
    )


def _pull_number_for_head(
    pulls: Sequence[Mapping[str, Any]], repository: str, head_sha: str
) -> int | None:
    matching: list[int] = []
    for pull in pulls:
        base = pull.get("base")
        head = pull.get("head")
        if not isinstance(base, dict) or not isinstance(head, dict):
            continue
        base_repo = base.get("repo")
        head_repo = head.get("repo")
        number = pull.get("number")
        if (
            base.get("ref") == "main"
            and isinstance(base_repo, dict)
            and base_repo.get("full_name") == repository
            and isinstance(head_repo, dict)
            and head_repo.get("full_name") == repository
            and head.get("sha") == head_sha
            and isinstance(number, int)
            and number > 0
        ):
            matching.append(number)
    if len(matching) > 1:
        raise AuthorityError("workflow head belongs to multiple main pull requests")
    return matching[0] if matching else None


def _quality_run(final_head: str, runs: Sequence[Mapping[str, Any]]) -> Mapping[str, Any] | None:
    matching = [run for run in runs if run.get("head_sha") == final_head]
    if not matching:
        return None
    if any(not isinstance(run.get("id"), int) or run["id"] < 1 for run in matching):
        raise AuthorityError("quality run identity is malformed")
    run = max(matching, key=lambda item: item["id"])
    if not (
        run.get("event") == "pull_request_target"
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and run.get("run_attempt") == 1
    ):
        return None
    return run


def _candidates(
    artifacts: Sequence[Mapping[str, Any]], *, binary_impact: bool
) -> tuple[str, list[dict[str, Any]]] | None:
    candidates = [
        item
        for item in artifacts
        if str(item.get("name", "")).startswith("release-candidate")
    ]
    if not binary_impact and not candidates:
        return None
    if not binary_impact:
        raise AuthorityError("non-binary quality produced release candidates")
    if len(candidates) != 2:
        raise AuthorityError("binary publication requires exactly two platform candidates")
    parsed: list[dict[str, Any]] = []
    for platform, pattern in (("windows", WINDOWS_CANDIDATE), ("linux", LINUX_CANDIDATE)):
        matches = [(item, pattern.fullmatch(str(item.get("name", "")))) for item in candidates]
        matches = [(item, match) for item, match in matches if match is not None]
        if len(matches) != 1:
            raise AuthorityError(f"{platform} candidate is missing or duplicated")
        item, match = matches[0]
        assert match is not None
        if not (
            item.get("expired") is False
            and isinstance(item.get("id"), int)
            and item["id"] > 0
            and isinstance(item.get("digest"), str)
            and DIGEST.fullmatch(item["digest"])
        ):
            raise AuthorityError(f"{platform} candidate identity is inconsistent")
        parsed.append(
            {
                "digest": item["digest"],
                "id": item["id"],
                "name": item["name"],
                "platform": platform,
                "version": match.group("version"),
            }
        )
    versions = {item["version"] for item in parsed}
    if len(versions) != 1:
        raise AuthorityError("platform candidates use different versions")
    return versions.pop(), sorted(parsed, key=lambda item: item["platform"])


def resolve_from_objects(
    *,
    repository: str,
    number: int,
    pull: Mapping[str, Any],
    runs: Sequence[Mapping[str, Any]],
    artifacts: Sequence[Mapping[str, Any]],
    binary_impact: bool,
) -> dict[str, Any]:
    authority = _pull_authority(repository, number, pull)
    run = _quality_run(authority["final_head"], runs)
    if run is None:
        return {"publish": False, "reason": "quality-not-authoritative"}
    candidate_result = _candidates(artifacts, binary_impact=binary_impact)
    if candidate_result is None:
        return {"publish": False, "reason": "non-binary-quality"}
    version, candidates = candidate_result
    snapshot: dict[str, Any] = {
        "artifacts": candidates,
        "final_head": authority["final_head"],
        "merge_sha": authority["merge_sha"],
        "pr_number": number,
        "publish": True,
        "run_id": run["id"],
        "tag": f"windows-v{version}",
        "version": version,
    }
    return snapshot


def resolve_live(event_name: str, event: Mapping[str, Any], repository: str) -> dict[str, Any]:
    number, signal_head, eligible = _signal(event_name, event)
    if not eligible:
        return {"publish": False, "reason": "signal-not-ready"}
    assert signal_head is not None
    pulls = _api(f"repos/{repository}/commits/{signal_head}/pulls?per_page=100")
    if not isinstance(pulls, list):
        raise AuthorityError("commit pull-request response is malformed")
    resolved_number = _pull_number_for_head(pulls, repository, signal_head)
    if resolved_number is None:
        return {"publish": False, "reason": "pull-not-found"}
    if number is not None and number != resolved_number:
        raise AuthorityError("release signal PR does not match the exact-head pull request")
    number = resolved_number
    pull = _object(_api(f"repos/{repository}/pulls/{number}"), "pull request")
    if pull.get("merged") is not True:
        return {"publish": False, "reason": "pull-not-merged"}
    authority = _pull_authority(repository, number, pull)
    head = authority["final_head"]
    if signal_head != head:
        raise AuthorityError("release signal head no longer matches the merged pull request")
    runs_response = _object(
        _api(
            f"repos/{repository}/actions/workflows/main-quality.yml/runs"
            f"?event=pull_request_target&head_sha={head}&per_page=100"
        ),
        "quality run response",
    )
    runs = runs_response.get("workflow_runs")
    if not isinstance(runs, list) or runs_response.get("total_count", len(runs)) > 100:
        raise AuthorityError("quality run response is malformed or unbounded")
    run = _quality_run(head, runs)
    if run is None:
        return {"publish": False, "reason": "quality-not-authoritative"}
    binary_impact = _binary_impact(repository, authority["base_sha"], head)
    artifacts_response = _object(
        _api(f"repos/{repository}/actions/runs/{run['id']}/artifacts?per_page=100"),
        "artifact response",
    )
    artifacts = artifacts_response.get("artifacts")
    if not isinstance(artifacts, list) or artifacts_response.get("total_count", len(artifacts)) > 100:
        raise AuthorityError("artifact response is malformed or unbounded")
    return resolve_from_objects(
        repository=repository,
        number=number,
        pull=pull,
        runs=runs,
        artifacts=artifacts,
        binary_impact=binary_impact,
    )


def _write_outputs(path: Path, snapshot: Mapping[str, Any]) -> None:
    with path.open("a", encoding="utf-8") as stream:
        stream.write(f"publish={str(snapshot.get('publish') is True).lower()}\n")
        stream.write("snapshot=" + json.dumps(snapshot, separators=(",", ":"), sort_keys=True) + "\n")
        if snapshot.get("publish") is True:
            artifacts = snapshot["artifacts"]
            stream.write("artifact_ids=" + ",".join(str(item["id"]) for item in artifacts) + "\n")
            for key in ("run_id", "tag"):
                stream.write(f"{key}={snapshot[key]}\n")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--event", required=True, type=Path)
    parser.add_argument("--event-name", required=True, choices=("pull_request_target", "workflow_run"))
    parser.add_argument("--repository", required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--expect")
    args = parser.parse_args(argv)
    try:
        event = _object(json.loads(args.event.read_text(encoding="utf-8")), "event")
        snapshot = resolve_live(args.event_name, event, args.repository)
        if args.expect is not None:
            expected = _object(json.loads(args.expect), "expected snapshot")
            if snapshot != expected or snapshot.get("publish") is not True:
                raise AuthorityError("release authority changed after acquiring the tag lock")
        if args.output is not None:
            _write_outputs(args.output, snapshot)
        else:
            print(json.dumps(snapshot, separators=(",", ":"), sort_keys=True))
    except (AuthorityError, OSError, json.JSONDecodeError) as exc:
        print(f"release-authority: FAIL {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

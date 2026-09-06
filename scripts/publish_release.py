#!/usr/bin/env python3
"""Publish one already-validated Windows/Linux candidate pair as a draft transaction."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence


SHA = re.compile(r"^[0-9a-f]{40}$", re.ASCII)
VERSION = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$", re.ASCII)


class PublicationError(RuntimeError):
    pass


@dataclass(frozen=True)
class Asset:
    path: Path
    name: str
    content_type: str
    size: int
    digest: str


def _object(value: object, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PublicationError(f"{label} is not an object")
    return value


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def candidate_assets(directory: Path, snapshot: Mapping[str, Any], repository: str) -> list[Asset]:
    version = snapshot.get("version")
    final_head = snapshot.get("final_head")
    run_id = snapshot.get("run_id")
    if not (
        isinstance(version, str)
        and VERSION.fullmatch(version)
        and isinstance(final_head, str)
        and SHA.fullmatch(final_head)
        and isinstance(run_id, int)
        and run_id > 0
    ):
        raise PublicationError("snapshot candidate identity is malformed")
    if not directory.is_dir() or directory.is_symlink():
        raise PublicationError("candidate directory is not a regular directory")

    linux_archive_name = f"codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz"
    names = {
        "CodexInfo.WindowsClient.Setup.exe": "application/octet-stream",
        "CodexInfo.WindowsClient.update.json": "application/json",
        linux_archive_name: "application/gzip",
        linux_archive_name + ".sha256": "text/plain",
        f"codex-info-{version}-x86_64-unknown-linux-gnu.manifest.json": "application/json",
    }
    present = {path.name for path in directory.iterdir() if path.is_file() and not path.is_symlink()}
    if present != set(names):
        raise PublicationError("candidate directory must contain the exact five release assets")

    setup = directory / "CodexInfo.WindowsClient.Setup.exe"
    windows_manifest = directory / "CodexInfo.WindowsClient.update.json"
    windows = _object(json.loads(windows_manifest.read_text(encoding="utf-8")), "Windows manifest")
    installer = _object(windows.get("installer"), "Windows installer identity")
    if not (
        windows.get("schema_version") == 1
        and windows.get("version") == version
        and installer.get("name") == setup.name
        and installer.get("url")
        == f"https://github.com/{repository}/releases/download/windows-v{version}/{setup.name}"
        and installer.get("sha256") == _sha256(setup)
        and installer.get("size") == setup.stat().st_size
    ):
        raise PublicationError("Windows manifest does not identify the candidate installer")

    archive = directory / linux_archive_name
    checksum = directory / (linux_archive_name + ".sha256")
    linux_manifest = directory / f"codex-info-{version}-x86_64-unknown-linux-gnu.manifest.json"
    expected_checksum = f"{_sha256(archive)}  {linux_archive_name}\n"
    if checksum.read_text(encoding="utf-8") != expected_checksum:
        raise PublicationError("Linux checksum does not identify the candidate archive")
    linux = _object(json.loads(linux_manifest.read_text(encoding="utf-8")), "Linux manifest")
    if not (
        linux.get("schema") == "codex-info-linux-bundle-v1"
        and linux.get("product") == "codex_info"
        and linux.get("version") == version
        and linux.get("source_sha") == final_head
        and linux.get("target") == "x86_64-unknown-linux-gnu"
        and linux.get("compatibility") == "glibc"
        and isinstance(linux.get("glibc_minimum"), str)
        and re.fullmatch(r"[0-9]+\.[0-9]+", linux["glibc_minimum"])
        and str(linux.get("run_id")) == str(run_id)
        and linux.get("run_attempt") == 1
        and isinstance(linux.get("files"), list)
        and linux["files"]
    ):
        raise PublicationError("Linux manifest does not identify the candidate archive")

    assets: list[Asset] = []
    for name, content_type in names.items():
        path = directory / name
        size = path.stat().st_size
        if size <= 0:
            raise PublicationError(f"release asset is empty: {name}")
        assets.append(Asset(path, name, content_type, size, "sha256:" + _sha256(path)))
    return assets


class GitHubRelease:
    def __init__(self, repository: str, token: str) -> None:
        if not token:
            raise PublicationError("GH_TOKEN is unavailable")
        self.repository = repository
        self.headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "codex-info-release",
        }

    def request(
        self,
        method: str,
        url: str,
        *,
        payload: Mapping[str, Any] | None = None,
        data: bytes | None = None,
        content_type: str = "application/json",
        allow_missing: bool = False,
    ) -> Any | None:
        if payload is not None:
            data = json.dumps(payload, separators=(",", ":")).encode()
        request = urllib.request.Request(url, data=data, method=method, headers=self.headers)
        if data is not None:
            request.add_header("Content-Type", content_type)
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                raw = response.read()
        except urllib.error.HTTPError as exc:
            if allow_missing and exc.code == 404:
                return None
            detail = exc.read().decode("utf-8", errors="replace")
            raise PublicationError(f"GitHub {method} {url} failed: HTTP {exc.code}: {detail}") from exc
        try:
            return json.loads(raw)
        except json.JSONDecodeError as exc:
            raise PublicationError(f"GitHub {method} {url} returned malformed JSON") from exc

    def api(self, method: str, path: str, **kwargs: Any) -> Any | None:
        return self.request(method, f"https://api.github.com/repos/{self.repository}/{path}", **kwargs)

    def upload(self, release_id: int, asset: Asset) -> dict[str, Any]:
        name = urllib.parse.quote(asset.name, safe="")
        result = self.request(
            "POST",
            f"https://uploads.github.com/repos/{self.repository}/releases/{release_id}/assets?name={name}",
            data=asset.path.read_bytes(),
            content_type=asset.content_type,
        )
        return _object(result, f"uploaded asset {asset.name}")


def _verify_release(
    release: Mapping[str, Any], *, release_id: int, tag: str, merge_sha: str, draft: bool
) -> None:
    if not (
        release.get("id") == release_id
        and release.get("tag_name") == tag
        and release.get("target_commitish") == merge_sha
        and release.get("draft") is draft
        and release.get("prerelease") is False
    ):
        raise PublicationError("release metadata does not match the immutable candidate")


def _verify_tag(tag_state: object, *, tag: str, merge_sha: str) -> None:
    tag_ref = _object(tag_state, "release tag")
    target = _object(tag_ref.get("object"), "release tag target")
    if not (
        tag_ref.get("ref") == f"refs/tags/{tag}"
        and target.get("sha") == merge_sha
        and target.get("type") == "commit"
    ):
        raise PublicationError("release tag does not target the merge commit")


def _verify_assets(remote: object, expected: Sequence[Asset]) -> None:
    if not isinstance(remote, list) or len(remote) != len(expected):
        raise PublicationError("release asset count does not match the candidate")
    by_name = {asset.name: asset for asset in expected}
    if len(by_name) != len(expected):
        raise PublicationError("candidate asset names are duplicated")
    objects = [_object(item, "release asset") for item in remote]
    remote_names = [obj.get("name") for obj in objects]
    if not all(isinstance(name, str) for name in remote_names) or set(
        remote_names
    ) != set(by_name):
        raise PublicationError("release asset names do not match the candidate")
    for obj in objects:
        expected_asset = by_name.get(obj.get("name"))
        if expected_asset is None or not (
            obj.get("state") == "uploaded"
            and obj.get("size") == expected_asset.size
            and obj.get("digest") == expected_asset.digest
        ):
            raise PublicationError("release asset identity does not match the candidate")


def publish(
    snapshot: Mapping[str, Any],
    directory: Path,
    repository: str,
    read_token: str,
    write_token: str,
    *,
    reader: GitHubRelease | None = None,
    writer: GitHubRelease | None = None,
) -> None:
    if snapshot.get("publish") is not True:
        raise PublicationError("snapshot is not publishable")
    tag = snapshot.get("tag")
    version = snapshot.get("version")
    merge_sha = snapshot.get("merge_sha")
    if not (
        tag == f"windows-v{version}"
        and isinstance(version, str)
        and VERSION.fullmatch(version)
        and isinstance(merge_sha, str)
        and SHA.fullmatch(merge_sha)
    ):
        raise PublicationError("release snapshot is malformed")
    assets = candidate_assets(directory, snapshot, repository)
    reader = reader or GitHubRelease(repository, read_token)
    writer = writer or GitHubRelease(repository, write_token)

    existing_tag = reader.api("GET", f"git/ref/tags/{urllib.parse.quote(tag, safe='')}", allow_missing=True)
    existing_release = reader.api("GET", f"releases/tags/{urllib.parse.quote(tag, safe='')}", allow_missing=True)
    if existing_tag is not None or existing_release is not None:
        if existing_tag is None or existing_release is None:
            raise PublicationError("tag and release do not exist as one complete state")
        _verify_tag(existing_tag, tag=tag, merge_sha=merge_sha)
        release = _object(existing_release, "existing release")
        release_id = release.get("id")
        if not isinstance(release_id, int) or release_id < 1 or release.get("draft") is not False:
            raise PublicationError("existing release is draft or malformed; automatic repair is forbidden")
        _verify_release(release, release_id=release_id, tag=tag, merge_sha=merge_sha, draft=False)
        _verify_assets(reader.api("GET", f"releases/{release_id}/assets?per_page=100"), assets)
        return

    created = _object(
        writer.api(
            "POST",
            "releases",
            payload={
                "tag_name": tag,
                "target_commitish": merge_sha,
                "name": f"Codex Info Monitor {version}",
                "body": f"Windows and Linux release {version}",
                "draft": True,
                "prerelease": False,
            },
        ),
        "created release",
    )
    release_id = created.get("id")
    if not isinstance(release_id, int) or release_id < 1:
        raise PublicationError("created release has no ID")
    _verify_release(created, release_id=release_id, tag=tag, merge_sha=merge_sha, draft=True)
    for asset in assets:
        writer.upload(release_id, asset)

    draft = _object(reader.api("GET", f"releases/{release_id}"), "draft release")
    _verify_release(draft, release_id=release_id, tag=tag, merge_sha=merge_sha, draft=True)
    _verify_assets(reader.api("GET", f"releases/{release_id}/assets?per_page=100"), assets)
    writer.api("PATCH", f"releases/{release_id}", payload={"draft": False})
    final_tag = reader.api("GET", f"git/ref/tags/{urllib.parse.quote(tag, safe='')}")
    _verify_tag(final_tag, tag=tag, merge_sha=merge_sha)
    final_release = _object(reader.api("GET", f"releases/{release_id}"), "published release readback")
    _verify_release(final_release, release_id=release_id, tag=tag, merge_sha=merge_sha, draft=False)
    _verify_assets(reader.api("GET", f"releases/{release_id}/assets?per_page=100"), assets)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--snapshot", required=True)
    parser.add_argument("--candidate-dir", required=True, type=Path)
    parser.add_argument("--repository", required=True)
    args = parser.parse_args(argv)
    try:
        snapshot = _object(json.loads(args.snapshot), "snapshot")
        publish(
            snapshot,
            args.candidate_dir,
            args.repository,
            os.environ.get("GH_TOKEN", ""),
            os.environ.get("RELEASE_TOKEN", ""),
        )
    except (PublicationError, OSError, json.JSONDecodeError) as exc:
        print(f"publish-release: FAIL {exc}", file=sys.stderr)
        return 1
    print(f"publish-release: PASS {snapshot['tag']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

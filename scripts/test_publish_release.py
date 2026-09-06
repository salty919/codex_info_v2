#!/usr/bin/env python3
"""Distinct candidate and publication-state cases for publish_release.py."""

from __future__ import annotations

import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
import publish_release  # noqa: E402


REPOSITORY = "owner/repository"
HEAD = "1" * 40
MERGE = "2" * 40
VERSION = "1.0.35"


def snapshot() -> dict[str, object]:
    return {
        "publish": True,
        "pr_number": 7,
        "final_head": HEAD,
        "merge_sha": MERGE,
        "run_id": 41,
        "run_attempt": 1,
        "version": VERSION,
        "tag": f"windows-v{VERSION}",
        "artifacts": [],
    }


def write_candidates(root: Path) -> None:
    setup = root / "CodexInfo.WindowsClient.Setup.exe"
    setup.write_bytes(b"setup")
    setup_hash = hashlib.sha256(setup.read_bytes()).hexdigest()
    (root / "CodexInfo.WindowsClient.update.json").write_text(
        json.dumps(
            {
                "schema_version": 1,
                "version": VERSION,
                "installer": {
                    "name": setup.name,
                    "url": f"https://github.com/{REPOSITORY}/releases/download/windows-v{VERSION}/{setup.name}",
                    "sha256": setup_hash,
                    "size": setup.stat().st_size,
                },
            }
        ),
        encoding="utf-8",
    )
    archive_name = f"codex-info-{VERSION}-x86_64-unknown-linux-gnu.tar.gz"
    archive = root / archive_name
    archive.write_bytes(b"archive")
    archive_hash = hashlib.sha256(archive.read_bytes()).hexdigest()
    (root / f"{archive_name}.sha256").write_text(
        f"{archive_hash}  {archive_name}\n", encoding="utf-8"
    )
    (root / f"codex-info-{VERSION}-x86_64-unknown-linux-gnu.manifest.json").write_text(
        json.dumps(
            {
                "schema": "codex-info-linux-bundle-v1",
                "product": "codex_info",
                "version": VERSION,
                "source_sha": HEAD,
                "target": "x86_64-unknown-linux-gnu",
                "compatibility": "glibc",
                "glibc_minimum": "2.35",
                "run_id": "41",
                "run_attempt": 1,
                "files": [{"path": "codex_info"}],
            }
        ),
        encoding="utf-8",
    )


class FakeGitHub:
    def __init__(self, *, existing: str = "absent") -> None:
        self.release: dict[str, object] | None = None
        self.tag: dict[str, object] | None = None
        self.assets: list[dict[str, object]] = []
        self.mutations: list[str] = []
        if existing in {"draft", "published"}:
            self.release = self.release_object(draft=existing == "draft")
        if existing == "published":
            self.tag = self.tag_object()

    @staticmethod
    def release_object(*, draft: bool) -> dict[str, object]:
        return {
            "id": 91,
            "tag_name": f"windows-v{VERSION}",
            "target_commitish": MERGE,
            "name": f"Codex Info Monitor {VERSION}",
            "body": f"Windows and Linux release {VERSION}",
            "draft": draft,
            "prerelease": False,
        }

    @staticmethod
    def tag_object() -> dict[str, object]:
        return {"ref": f"refs/tags/windows-v{VERSION}", "object": {"sha": MERGE, "type": "commit", "url": "u"}}

    def api(self, method: str, path: str, **kwargs):
        if path.startswith("git/ref/tags/"):
            return self.tag
        if path.startswith("releases/tags/"):
            return self.release
        if path == "releases" and method == "POST":
            self.mutations.append("create")
            self.release = self.release_object(draft=True)
            return self.release
        if path == "releases/91" and method == "GET":
            return self.release
        if path == "releases/91/assets?per_page=100":
            return list(self.assets)
        if path == "releases/91" and method == "PATCH":
            self.mutations.append("publish")
            self.release = self.release_object(draft=False)
            self.tag = self.tag_object()
            return self.release
        raise AssertionError((method, path, kwargs))

    def upload(self, release_id: int, asset: publish_release.Asset) -> dict[str, object]:
        self.mutations.append(f"upload:{asset.name}")
        result = {"name": asset.name, "state": "uploaded", "size": asset.size, "digest": asset.digest}
        self.assets.append(result)
        return result


class ReadOnlyGitHub:
    def __init__(self, shared: FakeGitHub) -> None:
        self.shared = shared

    def api(self, method: str, path: str, **kwargs):
        if method != "GET":
            raise AssertionError(f"read token received mutation: {method} {path}")
        return self.shared.api(method, path, **kwargs)


class WriteOnlyGitHub:
    def __init__(self, shared: FakeGitHub) -> None:
        self.shared = shared

    def api(self, method: str, path: str, **kwargs):
        if method not in {"POST", "PATCH"}:
            raise AssertionError(f"write token received read: {method} {path}")
        return self.shared.api(method, path, **kwargs)

    def upload(self, release_id: int, asset: publish_release.Asset) -> dict[str, object]:
        return self.shared.upload(release_id, asset)


class PublicationTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temporary = tempfile.TemporaryDirectory(prefix="codex-info-publish-")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        write_candidates(root)
        return temporary, root

    def test_absent_state_publishes_one_complete_draft(self) -> None:
        _, root = self.fixture()
        github = FakeGitHub()
        publish_release.publish(snapshot(), root, REPOSITORY, "unused", "unused", reader=github, writer=github)
        self.assertEqual(github.mutations[0], "create")
        self.assertEqual(github.mutations[-1], "publish")
        self.assertEqual(len([item for item in github.mutations if item.startswith("upload:")]), 5)

    def test_exact_published_state_is_a_mutation_free_noop(self) -> None:
        _, root = self.fixture()
        github = FakeGitHub(existing="published")
        github.assets = [
            {"name": asset.name, "state": "uploaded", "size": asset.size, "digest": asset.digest}
            for asset in publish_release.candidate_assets(root, snapshot(), REPOSITORY)
        ]
        publish_release.publish(snapshot(), root, REPOSITORY, "unused", "unused", reader=github, writer=github)
        self.assertEqual(github.mutations, [])

    def test_read_and_write_tokens_have_disjoint_operations(self) -> None:
        _, root = self.fixture()
        shared = FakeGitHub()
        publish_release.publish(
            snapshot(),
            root,
            REPOSITORY,
            "unused",
            "unused",
            reader=ReadOnlyGitHub(shared),
            writer=WriteOnlyGitHub(shared),
        )
        self.assertEqual(shared.mutations[0], "create")
        self.assertEqual(shared.mutations[-1], "publish")

    def test_duplicate_remote_name_cannot_hide_a_missing_asset(self) -> None:
        _, root = self.fixture()
        assets = publish_release.candidate_assets(root, snapshot(), REPOSITORY)
        remote = [
            {"name": asset.name, "state": "uploaded", "size": asset.size, "digest": asset.digest}
            for asset in assets
        ]
        remote[-1] = dict(remote[0])
        with self.assertRaises(publish_release.PublicationError):
            publish_release._verify_assets(remote, assets)

    def test_existing_draft_is_not_automatically_repaired(self) -> None:
        _, root = self.fixture()
        with self.assertRaises(publish_release.PublicationError):
            github = FakeGitHub(existing="draft")
            publish_release.publish(snapshot(), root, REPOSITORY, "unused", "unused", reader=github, writer=github)

    def test_existing_release_with_wrong_tag_target_is_rejected(self) -> None:
        _, root = self.fixture()
        github = FakeGitHub(existing="published")
        assert github.tag is not None
        github.tag["object"] = {"sha": "3" * 40, "type": "commit", "url": "u"}
        with self.assertRaises(publish_release.PublicationError):
            publish_release.publish(snapshot(), root, REPOSITORY, "unused", "unused", reader=github, writer=github)

    def test_mismatched_linux_checksum_is_rejected_before_mutation(self) -> None:
        _, root = self.fixture()
        checksum = root / f"codex-info-{VERSION}-x86_64-unknown-linux-gnu.tar.gz.sha256"
        checksum.write_text("0" * 64 + "  wrong.tar.gz\n", encoding="utf-8")
        github = FakeGitHub()
        with self.assertRaises(publish_release.PublicationError):
            publish_release.publish(snapshot(), root, REPOSITORY, "unused", "unused", reader=github, writer=github)
        self.assertEqual(github.mutations, [])


if __name__ == "__main__":
    unittest.main()

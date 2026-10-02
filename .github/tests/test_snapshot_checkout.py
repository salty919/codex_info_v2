#!/usr/bin/env python3
"""Check exact source snapshots without downloading their commit history."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
LEAF_JOBS = {
    "rust.yml": "native-quality",
    "linux-ui-quality.yml": "linux-ui-quality",
    "linux-distribution.yml": "linux-distribution",
}


def git(directory: Path, *arguments: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(directory), *arguments],
        text=True,
        stderr=subprocess.STDOUT,
    ).strip()


class SnapshotCheckoutTests(unittest.TestCase):
    def test_product_jobs_fetch_exact_non_tip_snapshot_without_history(self) -> None:
        with tempfile.TemporaryDirectory(prefix="ci-snapshot-checkout-") as temporary:
            root = Path(temporary)
            seed = root / "seed"
            seed.mkdir()
            git(seed, "init", "--quiet", "--initial-branch=main")
            git(seed, "config", "user.name", "Snapshot fixture")
            git(seed, "config", "user.email", "snapshot@example.invalid")
            for content in ("older", "requested source", "newer branch tip"):
                (seed / "source.txt").write_text(content, encoding="utf-8")
                git(seed, "add", "source.txt")
                git(seed, "commit", "--quiet", "-m", content)
                if content == "requested source":
                    source_sha = git(seed, "rev-parse", "HEAD")
                    source_tree = git(seed, "rev-parse", "HEAD^{tree}")

            for workflow, job in LEAF_JOBS.items():
                with self.subTest(workflow=workflow):
                    document = yaml.safe_load(
                        (ROOT / ".github" / "workflows" / workflow).read_text(
                            encoding="utf-8"
                        )
                    )
                    checkouts = [
                        step
                        for step in document["jobs"][job]["steps"]
                        if step.get("uses", "").startswith("actions/checkout@")
                    ]
                    self.assertEqual(len(checkouts), 1)
                    options = checkouts[0]["with"]
                    self.assertEqual(options["ref"], "${{ inputs.source_sha }}")
                    self.assertIs(options["persist-credentials"], False)
                    depth = options.get("fetch-depth", 1)
                    self.assertIs(type(depth), int)
                    self.assertGreaterEqual(depth, 0)

                    checkout = root / workflow
                    checkout.mkdir()
                    git(checkout, "init", "--quiet")
                    git(checkout, "remote", "add", "origin", seed.as_uri())
                    arguments = ["fetch", "--quiet", "--no-tags"]
                    if depth:
                        arguments.append(f"--depth={depth}")
                    git(checkout, *arguments, "origin", source_sha)
                    git(checkout, "checkout", "--quiet", "--detach", "FETCH_HEAD")

                    self.assertEqual(git(checkout, "rev-parse", "HEAD"), source_sha)
                    self.assertEqual(git(checkout, "rev-parse", "HEAD^{tree}"), source_tree)
                    self.assertEqual(
                        (checkout / "source.txt").read_text(encoding="utf-8"),
                        "requested source",
                    )
                    self.assertEqual(
                        git(checkout, "rev-list", "--count", "HEAD"),
                        "1",
                        "product checks need the source snapshot, not its ancestors",
                    )


if __name__ == "__main__":
    unittest.main()

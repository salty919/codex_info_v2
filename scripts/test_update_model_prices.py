"""Fixed-oracle tests for the official model-price snapshot updater."""

from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import subprocess  # noqa: B404  # nosec B404 # Offline fixture process API only.
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from typing import Any, ClassVar
from unittest import mock
from urllib import request
from urllib.error import URLError
from urllib.request import Request

import update_model_prices as pricing

BASH = shutil.which("bash")
if BASH is None or not Path(BASH).is_absolute():
    raise RuntimeError("offline workflow fixture requires an absolute Bash executable")


ORACLE_RATES = {
    "gpt-6-astra": {
        "input": 10.0,
        "cached_input": 1.0,
        "cache_write_input": 12.5,
        "output": 50.0,
    },
    "gpt-6.1-sol": {
        "input": 2.0,
        "cached_input": 0.1,
        "cache_write_input": 2.5,
        "output": 10.0,
    },
    "gpt-6-luna": {
        "input": 0.1,
        "cached_input": 0.01,
        "cache_write_input": 0.125,
        "output": 0.5,
    },
    "gpt-6-sol": {
        "input": 2.0,
        "cached_input": 0.2,
        "cache_write_input": 2.5,
        "output": 10.0,
    },
    "gpt-5.6-sol": {
        "input": 4.0,
        "cached_input": 0.4,
        "cache_write_input": 5.0,
        "output": 20.0,
    },
    "gpt-5.6-terra": {
        "input": 2.0,
        "cached_input": 0.2,
        "cache_write_input": 2.5,
        "output": 12.0,
    },
    "gpt-5.6-luna": {
        "input": 0.2,
        "cached_input": 0.02,
        "cache_write_input": 0.25,
        "output": 1.2,
    },
}

STANDARD_TABLE = """### Standard pricing data

| Model | Short context input | Short context cached input | Short context cache writes | Short context output | Long context input | Long context cached input | Long context cache writes | Long context output |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-6-astra | $10.00 | $1.00 | $12.50 | $50.00 | $20.00 | $2.00 | $25.00 | $75.00 |
| gpt-6.1-sol | $2.00 | $0.10 | $2.50 | $10.00 | $4.00 | $0.20 | $5.00 | $15.00 |
| gpt-6-luna | $0.10 | $0.01 | $0.125 | $0.50 | $0.20 | $0.02 | $0.25 | $0.75 |
| gpt-6-sol | $2.00 | $0.20 | $2.50 | $10.00 | $4.00 | $0.40 | $5.00 | $15.00 |
| gpt-5.6-sol | $4.00 | $0.40 | $5.00 | $20.00 | $8.00 | $0.80 | $10.00 | $30.00 |
| gpt-5.6-terra | $2.00 | $0.20 | $2.50 | $12.00 | $4.00 | $0.40 | $5.00 | $18.00 |
| gpt-5.6-luna | $0.20 | $0.02 | $0.25 | $1.20 | $0.40 | $0.04 | $0.50 | $1.80 |
| gpt-unselected | not USD | — | changed | n/a | ignored | — | ignored | n/a |

### Batch pricing data

| Model | Short context input | Short context cached input | Short context cache writes | Short context output | Long context input | Long context cached input | Long context cache writes | Long context output |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| gpt-6-astra | $5.00 | $0.50 | $6.25 | $25.00 | $10.00 | $1.00 | $12.50 | $37.50 |
"""


class PricingParserTests(unittest.TestCase):
    def test_standard_short_rows_match_independent_fixed_oracle(self) -> None:
        self.assertEqual(pricing.parse_standard_pricing(STANDARD_TABLE), ORACLE_RATES)

    def test_schema_change_fails_closed(self) -> None:
        changed = STANDARD_TABLE.replace(
            "Short context cache writes", "Short context cache write", 1
        )
        with self.assertRaises(pricing.PricingUpdateError):
            pricing.parse_standard_pricing(changed)

    def test_missing_target_model_fails_closed(self) -> None:
        missing = STANDARD_TABLE.replace(
            "| gpt-5.6-luna | $0.20 | $0.02 | $0.25 | $1.20 | $0.40 | $0.04 | $0.50 | $1.80 |\n",
            "",
        )
        with self.assertRaises(pricing.PricingUpdateError):
            pricing.parse_standard_pricing(missing)

    def test_duplicate_target_model_fails_closed(self) -> None:
        duplicated = STANDARD_TABLE.replace(
            "| gpt-unselected |",
            "| gpt-6-astra | $10.00 | $1.00 | $12.50 | $50.00 | $20.00 | $2.00 | $25.00 | $75.00 |\n| gpt-unselected |",
        )
        with self.assertRaises(pricing.PricingUpdateError):
            pricing.parse_standard_pricing(duplicated)

    def test_non_positive_or_non_finite_target_price_fails_closed(self) -> None:
        for invalid in ("$0", "$-1.00", "$NaN", "$inf", "—"):
            with self.subTest(invalid=invalid):
                bad = STANDARD_TABLE.replace("$12.50", invalid, 1)
                with self.assertRaises(pricing.PricingUpdateError):
                    pricing.parse_standard_pricing(bad)

    def test_duplicate_standard_table_fails_closed(self) -> None:
        with self.assertRaises(pricing.PricingUpdateError):
            pricing.parse_standard_pricing(STANDARD_TABLE + "\n" + STANDARD_TABLE)

    def test_malformed_table_row_after_targets_fails_closed(self) -> None:
        malformed = STANDARD_TABLE.replace(
            "| gpt-unselected |",
            "| malformed | only-two-cells |\n| gpt-unselected |",
        )
        with self.assertRaises(pricing.PricingUpdateError):
            pricing.parse_standard_pricing(malformed)


class SnapshotTests(unittest.TestCase):
    def test_first_write_unchanged_no_write_and_changed_append(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "standard-short.json"
            first = pricing.update_snapshot(path, ORACLE_RATES, observed_at=100)
            self.assertEqual(first["status"], "changed")
            original_bytes = path.read_bytes()
            original = json.loads(original_bytes)
            self.assertEqual(
                set(original),
                {"schema_version", "source", "basis", "currency", "unit_tokens", "revisions"},
            )
            self.assertEqual(original["schema_version"], 1)
            self.assertEqual(original["source"], pricing.SOURCE_PAGE_URL)
            self.assertEqual(original["basis"], "standard-short")
            self.assertEqual(original["currency"], "USD")
            self.assertEqual(original["unit_tokens"], 1_000_000)
            self.assertEqual(len(original["revisions"]), 1)
            self.assertEqual(original["revisions"][0]["observed_at"], 100)
            self.assertEqual(original["revisions"][0]["models"], ORACLE_RATES)
            canonical_rates = json.dumps(
                ORACLE_RATES, ensure_ascii=False, separators=(",", ":")
            ).encode("utf-8")
            expected_hash = hashlib.sha256(canonical_rates).hexdigest()
            self.assertEqual(original["revisions"][0]["id"], f"100-{expected_hash[:16]}")

            before_unchanged_mtime = path.stat().st_mtime_ns
            unchanged = pricing.update_snapshot(path, ORACLE_RATES, observed_at=200)
            self.assertEqual(unchanged["status"], "unchanged")
            self.assertEqual(path.read_bytes(), original_bytes)
            self.assertEqual(path.stat().st_mtime_ns, before_unchanged_mtime)

            changed_rates = copy.deepcopy(ORACLE_RATES)
            changed_rates["gpt-6-sol"]["output"] = 10.25
            changed = pricing.update_snapshot(path, changed_rates, observed_at=300)
            self.assertEqual(changed["status"], "changed")
            updated = json.loads(path.read_bytes())
            self.assertEqual(len(updated["revisions"]), 2)
            self.assertEqual(updated["revisions"][0], original["revisions"][0])
            self.assertEqual(updated["revisions"][1]["observed_at"], 300)
            self.assertEqual(updated["revisions"][1]["models"], changed_rates)
            self.assertGreater(
                updated["revisions"][1]["observed_at"],
                updated["revisions"][0]["observed_at"],
            )

    def test_invalid_markdown_preserves_snapshot_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "standard-short.json"
            pricing.update_snapshot(path, ORACLE_RATES, observed_at=100)
            before = path.read_bytes()
            invalid = STANDARD_TABLE.replace("Short context output", "Short output", 1)
            with self.assertRaises(pricing.PricingUpdateError):
                pricing.update_from_markdown(path, invalid, observed_at=200)
            self.assertEqual(path.read_bytes(), before)

    def test_source_failure_preserves_snapshot_bytes_and_is_not_retried(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "standard-short.json"
            pricing.update_snapshot(path, ORACLE_RATES, observed_at=100)
            before = path.read_bytes()
            failing_fetch = mock.Mock(side_effect=URLError("offline"))
            with self.assertRaises(pricing.PricingUpdateError):
                pricing.update_from_source(path, fetcher=failing_fetch, observed_at=200)
            self.assertEqual(failing_fetch.call_count, 1)
            self.assertEqual(path.read_bytes(), before)

    def test_cli_accepts_an_offline_markdown_fixture(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "pricing.md"
            snapshot = root / "standard-short.json"
            source.write_text(STANDARD_TABLE, encoding="utf-8")
            # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
            completed = subprocess.run(  # noqa: B603  # nosec B603 # Fixed interpreter and private fixture argv.
                [
                    sys.executable,
                    str(Path(pricing.__file__).resolve()),
                    "--input-file",
                    str(source),
                    "--snapshot",
                    str(snapshot),
                    "--observed-at",
                    "400",
                ],
                check=False,
                capture_output=True,
                text=True,
                shell=False,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(json.loads(completed.stdout)["status"], "changed")
            self.assertEqual(
                json.loads(snapshot.read_text(encoding="utf-8"))["revisions"][0]["models"],
                ORACLE_RATES,
            )


class FetchBoundaryTests(unittest.TestCase):
    class Response:
        status = 200
        headers: ClassVar[dict[str, str]] = {"Content-Type": "text/markdown; charset=utf-8"}

        def __init__(self, data: bytes, url: str = pricing.MARKDOWN_URL):
            self.data = data
            self.url = url
            self.read_sizes: list[int] = []

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            return False

        def geturl(self):
            return self.url

        def read(self, size: int) -> bytes:
            self.read_sizes.append(size)
            return self.data[:size]

    def test_fetch_uses_only_official_markdown_url_timeout_and_one_request(self) -> None:
        response = self.Response(STANDARD_TABLE.encode("utf-8"))
        opener = mock.Mock()
        opener.open.return_value = response
        with mock.patch.object(request, "build_opener", return_value=opener):
            result = pricing.fetch_standard_markdown()
        self.assertEqual(result, STANDARD_TABLE)
        opener.open.assert_called_once()
        request_object, = opener.open.call_args.args
        self.assertIsInstance(request_object, Request)
        self.assertEqual(request_object.full_url, pricing.MARKDOWN_URL)
        self.assertEqual(opener.open.call_args.kwargs["timeout"], pricing.FETCH_TIMEOUT_SECONDS)
        self.assertEqual(response.read_sizes, [pricing.MAX_SOURCE_BYTES + 1])

    def test_fetch_rejects_oversized_response(self) -> None:
        response = self.Response(b"x" * (pricing.MAX_SOURCE_BYTES + 1))
        opener = mock.Mock()
        opener.open.return_value = response
        with (
            mock.patch.object(request, "build_opener", return_value=opener),
            self.assertRaises(pricing.PricingUpdateError),
        ):
            pricing.fetch_standard_markdown()

    def test_fetch_rejects_non_official_final_origin(self) -> None:
        response = self.Response(
            STANDARD_TABLE.encode("utf-8"), url="https://pricing.example.invalid/pricing.md"
        )
        opener = mock.Mock()
        opener.open.return_value = response
        with (
            mock.patch.object(request, "build_opener", return_value=opener),
            self.assertRaises(pricing.PricingUpdateError),
        ):
            pricing.fetch_standard_markdown()

    def test_redirect_handler_rejects_non_official_and_allows_official_https(self) -> None:
        handler = pricing.OfficialRedirectHandler()
        request_object = Request(pricing.MARKDOWN_URL)
        with self.assertRaises(pricing.PricingUpdateError):
            handler.redirect_request(
                request_object,
                None,
                302,
                "Found",
                {},
                "https://developers.openai.com.attacker.invalid/x",
            )
        redirected = handler.redirect_request(
            request_object,
            None,
            302,
            "Found",
            {},
            "https://developers.openai.com/api/docs/pricing.md?format=markdown",
        )
        self.assertIsNotNone(redirected)

    def test_fetch_failure_is_not_retried(self) -> None:
        opener = mock.Mock()
        opener.open.side_effect = URLError("offline")
        with (
            mock.patch.object(request, "build_opener", return_value=opener),
            self.assertRaises(pricing.PricingUpdateError),
        ):
            pricing.fetch_standard_markdown()
        opener.open.assert_called_once()


WORKFLOW_PATH = Path(__file__).resolve().parents[1] / ".github/workflows/model-prices.yml"
CATALOG_RELATIVE_PATH = "crates/codex-info-pricing/data/standard-short.json"


def _workflow_run_script() -> str:
    if not WORKFLOW_PATH.is_file():
        raise AssertionError(f"missing requested workflow: {WORKFLOW_PATH}")
    lines = WORKFLOW_PATH.read_text(encoding="utf-8").splitlines()
    candidates = [i for i, line in enumerate(lines) if line.strip() == "run: |"]
    if len(candidates) != 1:
        raise AssertionError(f"expected one workflow shell entrypoint, found {len(candidates)}")
    index = candidates[0]
    parent_indent = len(lines[index]) - len(lines[index].lstrip())
    body: list[str] = []
    for line in lines[index + 1 :]:
        if line.strip():
            indent = len(line) - len(line.lstrip())
            if indent <= parent_indent:
                break
        body.append(line)
    if not body:
        raise AssertionError("workflow shell entrypoint is empty")
    return textwrap.dedent("\n".join(body))


class PriceWorkflowCausalTests(unittest.TestCase):
    """Run the workflow's real shell block with isolated gh/git command stubs."""

    def _run_workflow(
        self,
        source: str,
        *,
        open_prs: str = "[]",
        advance_base: bool = False,
    ) -> dict[str, Any]:
        script = _workflow_run_script()
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        repository = root / "repo"
        repository.mkdir()
        scripts = repository / "scripts"
        scripts.mkdir()
        shutil.copy2(Path(pricing.__file__).resolve(), scripts / "update_model_prices.py")
        snapshot_path = repository / CATALOG_RELATIVE_PATH
        snapshot_path.parent.mkdir(parents=True)
        seed_snapshot = {
            "schema_version": 1,
            "source": pricing.SOURCE_PAGE_URL,
            "basis": "standard-short",
            "currency": "USD",
            "unit_tokens": 1_000_000,
            "revisions": [
                {"id": "fixture-seed", "observed_at": 100, "models": ORACLE_RATES}
            ],
        }
        snapshot_path.write_text(
            json.dumps(seed_snapshot, indent=2) + "\n", encoding="utf-8"
        )
        source_path = root / "pricing.md"
        source_path.write_text(source, encoding="utf-8")

        real_git = shutil.which("git")
        if real_git is None or not Path(real_git).is_absolute():
            raise RuntimeError("workflow fixture requires an absolute Git executable")
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        subprocess.run(  # noqa: B603  # nosec B603 # Fixed Git argv against a private fixture repo.
            [real_git, "init", "-q", str(repository)], check=True, shell=False
        )
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        subprocess.run(  # noqa: B603  # nosec B603 # Fixed Git argv against a private fixture repo.
            [real_git, "-C", str(repository), "add", "--all"], check=True, shell=False
        )
        initial_commit_env = os.environ.copy()
        initial_commit_env.update(
            {
                "GIT_AUTHOR_NAME": "Fixture Seed",
                "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                "GIT_COMMITTER_NAME": "Fixture Seed",
                "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": "/dev/null",
            }
        )
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        subprocess.run(  # noqa: B603  # nosec B603 # Fixed Git commit in a private fixture repo.
            [real_git, "-C", str(repository), "commit", "-qm", "fixture base"],
            check=True,
            env=initial_commit_env,
            shell=False,
        )
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        base_sha = subprocess.check_output(  # noqa: B603  # nosec B603 # Fixed read-only Git argv.
            [real_git, "-C", str(repository), "rev-parse", "HEAD"],
            text=True,
            shell=False,
        ).strip()

        fake_bin = root / "bin"
        fake_bin.mkdir()
        git_log = root / "git.jsonl"
        gh_log = root / "gh.jsonl"
        event_log = root / "events.jsonl"
        price_log = root / "price.jsonl"
        fetch_count = root / "fetch-count"
        git_stub = textwrap.dedent(
            r'''
            #!/__PYTHON__
            import json, os, subprocess, sys
            from pathlib import Path
            args = sys.argv[1:]
            with open(os.environ["GIT_LOG"], "a", encoding="utf-8") as stream:
                stream.write(json.dumps(args) + "\n")
            with open(os.environ["EVENT_LOG"], "a", encoding="utf-8") as stream:
                stream.write(json.dumps(["git", *args]) + "\n")
            real = os.environ["REAL_GIT"]
            repo = os.environ["GIT_FIXTURE_REPO"]
            if args and args[0] == "ls-remote":
                ref = args[-1]
                if ref == "refs/heads/feat/next":
                    count_path = Path(os.environ["FETCH_COUNT"])
                    count = int(count_path.read_text() or "0") + 1 if count_path.exists() else 1
                    count_path.write_text(str(count), encoding="utf-8")
                    sha = os.environ["ADVANCED_SHA"] if (os.environ["ADVANCE_BASE"] == "1" and count > 1) else os.environ["BASE_SHA"]
                    print(f"{sha}\t{ref}")
                raise SystemExit(0)
            if args and args[0] == "fetch":
                fetch_head = subprocess.check_output([real, "-C", repo, "rev-parse", "--git-path", "FETCH_HEAD"], text=True).strip()
                fetch_path = Path(fetch_head)
                if not fetch_path.is_absolute():
                    fetch_path = Path(repo) / fetch_path
                fetch_path.write_text(os.environ["BASE_SHA"] + "\t\tbranch of origin/feat/next\n", encoding="utf-8")
                raise SystemExit(0)
            if args and args[0] == "push":
                raise SystemExit(0)
            raise SystemExit(subprocess.run([real, "-C", repo, *args]).returncode)
            '''
        ).replace("/__PYTHON__", sys.executable).lstrip()
        gh_stub = textwrap.dedent(
            r'''
            #!/__PYTHON__
            import json, os, sys
            from pathlib import Path
            args = sys.argv[1:]
            with open(os.environ["GH_LOG"], "a", encoding="utf-8") as stream:
                stream.write(json.dumps(args) + "\n")
            with open(os.environ["EVENT_LOG"], "a", encoding="utf-8") as stream:
                stream.write(json.dumps(["gh", *args]) + "\n")
            if args[:2] == ["pr", "list"]:
                print(os.environ["OPEN_PRS"])
                raise SystemExit(0)
            if args[:2] == ["pr", "create"]:
                def option(name): return args[args.index(name) + 1]
                record = {
                    "title": option("--title"),
                    "base": option("--base"),
                    "head": option("--head"),
                    "body": Path(option("--body-file")).read_text(encoding="utf-8"),
                    "args": args,
                }
                Path(os.environ["GH_CREATED_PR"]).write_text(json.dumps(record), encoding="utf-8")
                print("https://github.com/salty919/codex_info_v2/pull/999")
                raise SystemExit(0)
            raise SystemExit(91)
            '''
        ).replace("/__PYTHON__", sys.executable).lstrip()
        python_stub = textwrap.dedent(
            """\
            #!/bin/sh
            if [ \"$1\" = \"scripts/update_model_prices.py\" ]; then
              shift
              printf '%s\\n' \"$*\" >> \"$PRICE_LOG\"
              exec \"$REAL_PYTHON\" \"$MODEL_PRICE_SCRIPT\" --input-file \"$PRICE_SOURCE_FILE\" \"$@\"
            fi
            exec \"$REAL_PYTHON\" \"$@\"
            """
        )
        for name, contents in (
            ("git", git_stub),
            ("gh", gh_stub),
            ("python3", python_stub),
        ):
            path = fake_bin / name
            path.write_text(contents, encoding="utf-8")
            path.chmod(0o755)

        advanced_sha = "f" * 40
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{fake_bin}{os.pathsep}{env['PATH']}",
                "REAL_GIT": real_git,
                "REAL_PYTHON": sys.executable,
                "MODEL_PRICE_SCRIPT": str(Path(pricing.__file__).resolve()),
                "GIT_FIXTURE_REPO": str(repository),
                "GIT_LOG": str(git_log),
                "GH_LOG": str(gh_log),
                "EVENT_LOG": str(event_log),
                "PRICE_LOG": str(price_log),
                "FETCH_COUNT": str(fetch_count),
                "BASE_SHA": base_sha,
                "ADVANCED_SHA": advanced_sha,
                "ADVANCE_BASE": "1" if advance_base else "0",
                "OPEN_PRS": open_prs,
                "PRICE_SOURCE_FILE": str(source_path),
                "GH_CREATED_PR": str(root / "created-pr.json"),
                "GITHUB_REPOSITORY": "salty919/codex_info_v2",
                "GITHUB_EVENT_NAME": "workflow_dispatch",
                "GH_TOKEN": "test-token-must-not-be-printed",  # noqa: B105  # nosec B105 # Non-secret sentinel verifies output redaction.
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": "/dev/null",
            }
        )
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        completed = subprocess.run(  # noqa: B603  # nosec B603 # Workflow script, stubbed commands, private repo.
            [BASH, "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", script],
            cwd=repository,
            env=env,
            check=False,
            capture_output=True,
            text=True,
            shell=False,
        )

        def read_json_lines(path: Path) -> list[list[str]]:
            if not path.exists():
                return []
            return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]

        return {
            "completed": completed,
            "base_sha": base_sha,
            "advanced_sha": advanced_sha,
            "git_calls": read_json_lines(git_log),
            "gh_calls": read_json_lines(gh_log),
            "events": read_json_lines(event_log),
            "price_calls": price_log.read_text(encoding="utf-8").splitlines()
            if price_log.exists()
            else [],
            "snapshot": json.loads(snapshot_path.read_text(encoding="utf-8")),
            "snapshot_bytes_before": json.dumps(seed_snapshot, indent=2).encode("utf-8") + b"\n",
            "snapshot_bytes_after": snapshot_path.read_bytes(),
            "created_pr": json.loads((root / "created-pr.json").read_text(encoding="utf-8"))
            if (root / "created-pr.json").exists()
            else None,
            "output": completed.stdout + completed.stderr,
        }

    def test_changed_prices_pushes_one_scoped_branch_and_creates_review_pr(self) -> None:
        changed_source = STANDARD_TABLE.replace("$10.00", "$10.25", 1)
        result = self._run_workflow(changed_source)
        self.assertEqual(result["completed"].returncode, 0, result["output"])
        self.assertEqual(len(result["price_calls"]), 1)
        self.assertEqual(len([call for call in result["git_calls"] if call[0] == "push"]), 1)
        self.assertEqual(len([call for call in result["gh_calls"] if call[:2] == ["pr", "create"]]), 1)
        self.assertEqual(result["created_pr"]["base"], "feat/next")
        self.assertEqual(result["created_pr"]["title"], "[Issue #463] Update official model prices")
        self.assertIn("Refs #463", result["created_pr"]["body"])
        self.assertIn("MODEL-PRICE-UPDATE-463", result["created_pr"]["body"])
        self.assertIn("full `feat/next` checks", result["created_pr"]["body"])
        self.assertNotIn("CI passed", result["created_pr"]["body"])
        self.assertNotIn("test-token-must-not-be-printed", result["output"])
        push_call = next(call for call in result["git_calls"] if call[0] == "push")
        self.assertNotIn("--force", push_call)
        commit_call = next(call for call in result["git_calls"] if "commit" in call)
        self.assertIn("user.name=github-actions[bot]", commit_call)
        self.assertIn(
            "user.email=41898282+github-actions[bot]@users.noreply.github.com",
            commit_call,
        )
        self.assertTrue(
            result["created_pr"]["head"].startswith(
                "codex/issue-463-price-update-" + result["base_sha"] + "-"
            )
        )
        self.assertEqual(
            [call[-1] for call in result["git_calls"] if call[:2] == ["add", "--"]],
            [CATALOG_RELATIVE_PATH],
        )
        staged_path_check = next(
            call for call in result["git_calls"] if call[:3] == ["diff", "--cached", "--name-status"]
        )
        self.assertIn("--find-renames", staged_path_check)
        self.assertIn("--find-copies", staged_path_check)
        self.assertEqual(result["events"][0][:3], ["gh", "pr", "list"])
        first_git_ref_read = next(
            i for i, event in enumerate(result["events"]) if event[:2] == ["git", "ls-remote"]
        )
        self.assertGreater(first_git_ref_read, 0)

    def test_unchanged_prices_make_no_commit_push_or_pr(self) -> None:
        result = self._run_workflow(STANDARD_TABLE)
        self.assertEqual(result["completed"].returncode, 0, result["output"])
        self.assertEqual(len(result["price_calls"]), 1)
        self.assertFalse(any(call[0] == "push" for call in result["git_calls"]))
        self.assertFalse(any("commit" in call for call in result["git_calls"]))
        self.assertFalse(any(call[:2] == ["pr", "create"] for call in result["gh_calls"]))
        self.assertEqual(len(result["snapshot"]["revisions"]), 1)

    def test_existing_autogenerated_pr_skips_price_fetch_and_git(self) -> None:
        result = self._run_workflow(
            "not a pricing table",
            open_prs='[{"number":123,"title":"[Issue #463] Update official model prices"}]',
        )
        self.assertEqual(result["completed"].returncode, 0, result["output"])
        self.assertEqual(result["events"][0][:3], ["gh", "pr", "list"])
        self.assertEqual(result["git_calls"], [])
        self.assertEqual(result["price_calls"], [])

    def test_invalid_pricing_fails_without_catalog_commit_push_or_pr(self) -> None:
        invalid = STANDARD_TABLE.replace("Short context output", "Short output", 1)
        result = self._run_workflow(invalid)
        self.assertNotEqual(result["completed"].returncode, 0)
        self.assertEqual(result["snapshot"]["revisions"][0]["models"], ORACLE_RATES)
        self.assertEqual(result["snapshot_bytes_before"], result["snapshot_bytes_after"])
        self.assertFalse(any(call[:2] == ["add", "--"] for call in result["git_calls"]))
        self.assertFalse(any(call[0] == "push" for call in result["git_calls"]))
        self.assertFalse(any(call[:2] == ["pr", "create"] for call in result["gh_calls"]))

    def test_feat_next_advancing_before_push_fails_closed(self) -> None:
        changed_source = STANDARD_TABLE.replace("$10.00", "$10.25", 1)
        result = self._run_workflow(changed_source, advance_base=True)
        self.assertNotEqual(result["completed"].returncode, 0)
        self.assertFalse(any(call[0] == "push" for call in result["git_calls"]))
        self.assertFalse(any(call[:2] == ["pr", "create"] for call in result["gh_calls"]))

    def test_workflow_schedule_permissions_and_checkout_are_fixed(self) -> None:
        contents = WORKFLOW_PATH.read_text(encoding="utf-8")
        self.assertIn('cron: "15 0 * * *"', contents)
        self.assertIn("workflow_dispatch:", contents)
        self.assertIn("permissions: {}", contents)
        self.assertIn("contents: write", contents)
        self.assertIn("pull-requests: write", contents)
        self.assertIn(
            "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09", contents
        )
        self.assertIn("ref: feat/next", contents)
        self.assertIn("persist-credentials: false", contents)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Execute the finite local control paths of version-prepare.yml.

This harness is intentionally local-only. It executes the shell bodies and
their declared environment from the real workflow while replacing GitHub API
boundaries with deterministic temporary fakes.
"""

from __future__ import annotations

import copy
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from dataclasses import dataclass, replace
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github" / "workflows" / "version-prepare.yml"
SCRIPT_DIR = ROOT / "scripts"
REPOSITORY = "salty919/codex_info_v2"
PR_NUMBER = "42"
BASE_SHA = "1" * 40
HEAD_SHA = "2" * 40
NEW_SHA = "3" * 40
MOVED_SHA = "4" * 40
HEAD_REF = "user/main-change"
RUN_ID = "24680"

STEP_NAMES = (
    "Classify pull request scope",
    "Validate and prepare version data",
    "Select the exact quality head",
    "Register required checks on the immutable final head",
    "Finalize required acceptance on the immutable head",
)
EXPRESSION = re.compile(r"\$\{\{\s*([^{}]+?)\s*\}\}")


@dataclass(frozen=True)
class WorkflowStep:
    name: str
    env: dict[str, str]
    run: str


def extract_step(source: str, name: str) -> WorkflowStep:
    marker = f"      - name: {name}\n"
    if source.count(marker) != 1:
        raise AssertionError(f"expected one workflow step named {name!r}")
    tail = source[source.index(marker) + len(marker) :]
    boundary = re.search(r"^(?:      - name:|  [A-Za-z0-9_-]+:)\s*", tail, re.M)
    block = tail if boundary is None else tail[: boundary.start()]

    env: dict[str, str] = {}
    env_marker = "        env:\n"
    if env_marker in block:
        for line in block.split(env_marker, 1)[1].splitlines():
            match = re.fullmatch(r"          ([A-Z0-9_]+): (.*)", line)
            if match is None:
                break
            key, value = match.groups()
            if key in env:
                raise AssertionError(f"duplicate environment key {key!r} in {name}")
            env[key] = value

    run_marker = "        run: |\n"
    if block.count(run_marker) != 1:
        raise AssertionError(f"expected one shell body in {name!r}")
    body: list[str] = []
    for line in block.split(run_marker, 1)[1].splitlines(keepends=True):
        if line.strip() and not line.startswith("          "):
            break
        body.append(line[10:] if line.startswith("          ") else line)
    run = "".join(body)
    if not run.strip():
        raise AssertionError(f"empty shell body in {name!r}")
    return WorkflowStep(name=name, env=env, run=run)


def render(value: str, context: dict[str, str]) -> str:
    def substitute(match: re.Match[str]) -> str:
        key = match.group(1).strip()
        if key not in context:
            raise AssertionError(f"unbound workflow expression: {key}")
        return context[key]

    return EXPRESSION.sub(substitute, value)


def read_outputs(path: Path) -> dict[str, str]:
    outputs: dict[str, str] = {}
    if not path.exists():
        return outputs
    for line in path.read_text(encoding="utf-8").splitlines():
        if "=" not in line:
            raise AssertionError(f"malformed GITHUB_OUTPUT line: {line!r}")
        key, value = line.split("=", 1)
        outputs[key] = value
    return outputs


def write_evidence(directory: Path, fields: dict[str, str]) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    evidence = directory / "acceptance.txt"
    evidence.write_text(
        "".join(f"{key}: {value}\n" for key, value in fields.items()),
        encoding="utf-8",
    )
    digest = hashlib.sha256(evidence.read_bytes()).hexdigest()
    (directory / "SHA256SUMS").write_text(
        f"{digest}  acceptance.txt\n", encoding="utf-8"
    )


FAKE_GH = r'''#!/usr/bin/env python3
import base64
import json
import os
from pathlib import Path
import sys
from urllib.parse import unquote

state_path = Path(os.environ["HARNESS_STATE"])
state = json.loads(state_path.read_text(encoding="utf-8"))
args = sys.argv[1:]
if not args or args[0] != "api":
    raise SystemExit("fake gh supports only api")

method = "GET"
if "--method" in args:
    method = args[args.index("--method") + 1]
endpoint = next((arg for arg in args if arg.startswith("repos/")), "")
if not endpoint:
    raise SystemExit(f"fake gh could not find endpoint in {args!r}")
if "--input" in args:
    sys.stdin.read()

def save():
    state_path.write_text(json.dumps(state, sort_keys=True), encoding="utf-8")

def emit(value):
    print(value if isinstance(value, str) else json.dumps(value, separators=(",", ":")))
    save()
    raise SystemExit(0)

if "/pulls/" in endpoint and "/files?" not in endpoint:
    count = state.get("pull_reads", 0)
    state["pull_reads"] = count + 1
    emit(state["before"] if count == 0 else state["after"])

if "/pulls/" in endpoint and "/files?" in endpoint:
    emit(state["file_pages"])

if "/contents/" in endpoint:
    suffix = endpoint.split("/contents/", 1)[1]
    path, ref = suffix.rsplit("?ref=", 1)
    path = unquote(path)
    ref = unquote(ref)
    version = state["base_version"] if ref == state["base_sha"] else state["head_version"]
    if path == "Cargo.toml":
        content = f'[package]\nname = "codex_info"\nversion = "{version}"\nedition = "2021"\n'
    elif path == "Cargo.lock":
        content = f'version = 4\n\n[[package]]\nname = "codex_info"\nversion = "{version}"\n'
    elif path == "windows-client/Directory.Build.props":
        content = f'<Project><PropertyGroup><Version>{version}</Version></PropertyGroup></Project>\n'
    else:
        raise SystemExit(f"unexpected contents path: {path}")
    emit({
        "type": "file",
        "encoding": "base64",
        "content": base64.b64encode(content.encode()).decode(),
    })

if "/git/ref/heads/" in endpoint and method == "GET":
    emit(state["ref_sha"] if "--jq" in args else {"object": {"sha": state["ref_sha"]}})

if endpoint.endswith(f'/git/commits/{state["head_sha"]}') and method == "GET":
    emit("5" * 40 if "--jq" in args else {"tree": {"sha": "5" * 40}})

if endpoint.endswith("/git/blobs") and method == "POST":
    state["git_object_writes"] = state.get("git_object_writes", 0) + 1
    emit(str(5 + state["git_object_writes"]) * 40)

if endpoint.endswith("/git/trees") and method == "POST":
    state["git_object_writes"] = state.get("git_object_writes", 0) + 1
    emit("9" * 40)

if endpoint.endswith("/git/commits") and method == "POST":
    state["git_object_writes"] = state.get("git_object_writes", 0) + 1
    emit(state["new_sha"])

if endpoint.endswith(f'/git/commits/{state["new_sha"]}') and method == "GET":
    emit({"tree": {"sha": "9" * 40}, "parents": [{"sha": state["head_sha"]}]})

raise SystemExit(f"unexpected fake gh request: method={method} endpoint={endpoint}")
'''


FAKE_PYTHON = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

real_python = os.environ["REAL_PYTHON"]
arguments = sys.argv[1:]
if not arguments or not arguments[0].endswith("final_head_check_reporter.py"):
    os.execv(real_python, [real_python, *arguments])

sys.path.insert(0, os.environ["HARNESS_SCRIPT_DIR"])
import final_head_check_reporter as reporter
from test_final_head_check_reporter import FakeApi, TransitionFakeApi

reporter_args = arguments[1:]
command = reporter_args[0]
mode = os.environ.get("HARNESS_REPORTER_MODE", "")
log_path = Path(os.environ["HARNESS_LOG"])

def option(name):
    return reporter_args[reporter_args.index(name) + 1]

def identity():
    return reporter.Identity(
        repository=option("--repository"),
        pr_number=int(option("--pr-number")),
        base_repository=option("--base-repository"),
        head_repository=option("--head-repository"),
        base_sha=option("--base-sha"),
        head_sha=option("--head-sha"),
        head_ref=option("--head-ref"),
        run_id=int(option("--run-id")),
        run_url=option("--run-url"),
    )

def append_log(api, ok, error=""):
    mutations = [
        method for method, _path, _payload in api.calls if method in {"POST", "PATCH"}
    ]
    acceptance = ""
    if hasattr(api, "checks") and api.checks.get("acceptance"):
        acceptance = api.checks["acceptance"][0].get("conclusion") or ""
    record = {
        "command": command,
        "mode": mode,
        "ok": ok,
        "error": error,
        "mutations": mutations,
        "acceptance": acceptance,
        "ref_sha": getattr(api, "ref_sha", ""),
        "transition_check": getattr(api, "created_check", None) is not None,
    }
    with log_path.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(record, sort_keys=True) + "\n")

if command == "publish-version":
    api = TransitionFakeApi(updates=["rule", "success"] if mode == "publish_rule" else None)
    original = reporter.publish_version_transition
    reporter.publish_version_transition = lambda api, transition: original(
        api, transition, attempts=4, sleep=lambda _seconds: None
    )
else:
    api = FakeApi()
    current_identity = identity()
    api.pull_request["head"]["sha"] = current_identity.head_sha
    if command == "register" and mode == "register_reuse":
        reporter.register_checks(api, current_identity)
        for name in reporter.CHECK_NAMES:
            api.checks[name][0]["status"] = "completed"
            api.checks[name][0]["conclusion"] = "success"
        api.calls.clear()
    elif command == "finalize":
        reporter.register_checks(api, current_identity)
        api.calls.clear()

reporter.GitHubApi = lambda _token: api
try:
    result = reporter.main(reporter_args)
except reporter.ReporterError as error:
    append_log(api, False, str(error))
    print(f"final-head-check-reporter: FAIL: {error}", file=sys.stderr)
    raise SystemExit(1)
append_log(api, True)
raise SystemExit(result)
'''


class Scenario:
    def __init__(
        self,
        *,
        files: list[dict[str, str]] | None = None,
        base_version: str = "1.0.15",
        head_version: str = "1.0.15",
        after_head_sha: str = HEAD_SHA,
        ref_sha: str = HEAD_SHA,
    ) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="version-prepare-local-")
        self.root = Path(self.temporary.name)
        self.runner_temp = self.root / "runner"
        self.runner_temp.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.state_path = self.root / "state.json"
        self.log_path = self.root / "reporter.jsonl"
        self.run_count = 0

        changed = files or [{"filename": "README.md", "status": "modified"}]
        before = {
            "number": int(PR_NUMBER),
            "state": "open",
            "changed_files": len(changed),
            "base": {
                "repo": {"full_name": REPOSITORY},
                "ref": "main",
                "sha": BASE_SHA,
            },
            "head": {
                "repo": {"full_name": REPOSITORY},
                "ref": HEAD_REF,
                "sha": HEAD_SHA,
            },
        }
        after = copy.deepcopy(before)
        after["head"]["sha"] = after_head_sha
        self.state_path.write_text(
            json.dumps(
                {
                    "before": before,
                    "after": after,
                    "file_pages": [changed],
                    "base_version": base_version,
                    "head_version": head_version,
                    "base_sha": BASE_SHA,
                    "head_sha": HEAD_SHA,
                    "new_sha": NEW_SHA,
                    "ref_sha": ref_sha,
                    "git_object_writes": 0,
                    "pull_reads": 0,
                },
                sort_keys=True,
            ),
            encoding="utf-8",
        )
        self._write_executable("gh", FAKE_GH)
        self._write_executable("python3", FAKE_PYTHON)

    def close(self) -> None:
        self.temporary.cleanup()

    def _write_executable(self, name: str, source: str) -> None:
        path = self.bin / name
        if name == "python3":
            source = source.replace("#!/usr/bin/env python3", f"#!{sys.executable}", 1)
        path.write_text(source, encoding="utf-8")
        path.chmod(0o755)

    def context(self, **overrides: str) -> dict[str, str]:
        values = {
            "github.token": "local-token",
            "github.repository": REPOSITORY,
            "github.event.pull_request.number": PR_NUMBER,
            "github.event.pull_request.base.repo.full_name": REPOSITORY,
            "github.event.pull_request.head.repo.full_name": REPOSITORY,
            "github.event.pull_request.head.ref": HEAD_REF,
            "github.event.pull_request.base.sha": BASE_SHA,
            "github.event.pull_request.head.sha": HEAD_SHA,
            "github.run_id": RUN_ID,
            "github.server_url": "https://github.com",
            "runner.temp": str(self.runner_temp),
        }
        values.update(overrides)
        return values

    def run(
        self,
        step: WorkflowStep,
        *,
        context: dict[str, str] | None = None,
        missing_env: set[str] | None = None,
        reporter_mode: str = "",
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
        self.run_count += 1
        output = self.root / f"output-{self.run_count}.txt"
        values = self.context()
        values.update(context or {})
        environment = os.environ.copy()
        for key in {item for candidate in STEPS.values() for item in candidate.env}:
            environment.pop(key, None)
        environment.update(
            {
                "PATH": f"{self.bin}:{environment['PATH']}",
                "REAL_PYTHON": sys.executable,
                "HARNESS_SCRIPT_DIR": str(SCRIPT_DIR),
                "HARNESS_STATE": str(self.state_path),
                "HARNESS_LOG": str(self.log_path),
                "HARNESS_REPORTER_MODE": reporter_mode,
                "RUNNER_TEMP": str(self.runner_temp),
                "GITHUB_OUTPUT": str(output),
                "GITHUB_RUN_ID": RUN_ID,
                "GITHUB_SERVER_URL": "https://github.com",
                "GITHUB_REPOSITORY": REPOSITORY,
            }
        )
        environment.update(
            {key: render(value, values) for key, value in step.env.items()}
        )
        for key in missing_env or set():
            environment.pop(key, None)
        result = subprocess.run(
            ["bash", "--noprofile", "--norc", "-c", render(step.run, values)],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        return result, read_outputs(output)

    def state(self) -> dict:
        return json.loads(self.state_path.read_text(encoding="utf-8"))

    def logs(self) -> list[dict]:
        if not self.log_path.exists():
            return []
        return [
            json.loads(line)
            for line in self.log_path.read_text(encoding="utf-8").splitlines()
        ]

    def prepare_artifacts(
        self,
        *,
        head_sha: str,
        binary_impact: str,
        windows_impact: str,
        version: str,
    ) -> None:
        write_evidence(
            self.runner_temp / "acceptance-verdict",
            {
                "schema": "codex-info-final-head-v1",
                "pr-number": PR_NUMBER,
                "source-sha": head_sha,
                "binary-impact": binary_impact,
                "windows-impact": windows_impact,
                "version": version,
                "acceptance": "PASS",
            },
        )
        if windows_impact == "true":
            write_evidence(
                self.runner_temp / "release-candidate",
                {
                    "schema": "codex-info-quality-v1",
                    "pr-number": PR_NUMBER,
                    "source-sha": head_sha,
                    "tree-sha": "8" * 40,
                    "version": version,
                    "acceptance": "PASS",
                },
            )


SOURCE = WORKFLOW.read_text(encoding="utf-8")
STEPS = {name: extract_step(SOURCE, name) for name in STEP_NAMES}


def assert_success(
    case: unittest.TestCase, result: subprocess.CompletedProcess[str]
) -> None:
    case.assertEqual(result.returncode, 0, result.stdout + result.stderr)


def final_context(
    scenario: Scenario,
    *,
    head_sha: str,
    binary_impact: str,
    windows_impact: str,
    version: str,
    owner_result: str = "success",
    candidate_result: str = "success",
) -> dict[str, str]:
    scenario.prepare_artifacts(
        head_sha=head_sha,
        binary_impact=binary_impact,
        windows_impact=windows_impact,
        version=version,
    )
    return scenario.context(
        **{
            "needs.prepare.outputs.binary_impact": binary_impact,
            "needs.selected-quality.result": owner_result,
            "needs.quality.outputs.candidate_artifact_digest": (
                "b" * 64 if windows_impact == "true" else ""
            ),
            "needs.quality.outputs.candidate_artifact_id": (
                "1002" if windows_impact == "true" else ""
            ),
            "needs.prepare.outputs.final_head_sha": head_sha,
            "needs.quality.result": candidate_result,
            "needs.quality.outputs.verdict_artifact_digest": "a" * 64,
            "needs.quality.outputs.verdict_artifact_id": "1001",
            "needs.prepare.outputs.version": version,
            "needs.prepare.outputs.windows_impact": windows_impact,
        }
    )


class VersionPrepareWorkflowLocalTests(unittest.TestCase):
    def scenario(self, **arguments: object) -> Scenario:
        scenario = Scenario(**arguments)
        self.addCleanup(scenario.close)
        return scenario

    def classify(
        self, scenario: Scenario
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
        return scenario.run(STEPS["Classify pull request scope"])

    def select(
        self, scenario: Scenario, binary: str, prepared: str
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
        return scenario.run(
            STEPS["Select the exact quality head"],
            context=scenario.context(
                **{
                    "steps.scope.outputs.binary_impact": binary,
                    "steps.prepare.outputs.final_head_sha": prepared,
                }
            ),
        )

    def register(
        self, scenario: Scenario, head_sha: str, *, mode: str = ""
    ) -> tuple[subprocess.CompletedProcess[str], dict[str, str]]:
        return scenario.run(
            STEPS["Register required checks on the immutable final head"],
            context=scenario.context(
                **{"needs.prepare.outputs.final_head_sha": head_sha}
            ),
            reporter_mode=mode,
        )

    def test_workflow_steps_and_pr_number_owner_are_exact(self) -> None:
        self.assertEqual(set(STEPS), set(STEP_NAMES))
        prepare = STEPS["Validate and prepare version data"]
        self.assertEqual(
            prepare.env.get("PR_NUMBER"),
            "$" + "{{ github.event.pull_request.number }}",
        )

    def test_local_harness_is_not_inserted_into_actions(self) -> None:
        for workflow in (ROOT / ".github" / "workflows").glob("*.yml"):
            with self.subTest(workflow=workflow.name):
                self.assertNotIn(
                    "test_version_prepare_workflow.py",
                    workflow.read_text(encoding="utf-8"),
                )

    def test_trusted_classifier_accepts_the_exact_prospective_diff(self) -> None:
        trusted_revision = os.environ.get("LOCAL_TRUSTED_MAIN", "origin/main")
        base_revision = os.environ.get("LOCAL_PR_BASE", "origin/feat/next")
        trusted_source = subprocess.run(
            [
                "git",
                "show",
                f"{trusted_revision}:scripts/ci_change_scope.py",
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout
        base_sha = subprocess.run(
            ["git", "rev-parse", f"{base_revision}^{{commit}}"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        head_sha = subprocess.run(
            ["git", "rev-parse", "HEAD^{commit}"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        changed_paths = subprocess.run(
            ["git", "diff", "--name-only", f"{base_sha}...{head_sha}"],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=True,
        ).stdout.splitlines()
        self.assertGreater(len(changed_paths), 0)

        with tempfile.TemporaryDirectory(prefix="trusted-classifier-") as raw_root:
            fixture_root = Path(raw_root)
            classifier = fixture_root / "ci_change_scope.py"
            classifier.write_text(trusted_source, encoding="utf-8")

            def classify(paths: list[str]) -> subprocess.CompletedProcess[str]:
                pull_request = fixture_root / "pull-request.json"
                files = fixture_root / "files.json"
                pull_request.write_text(
                    json.dumps(
                        {
                            "number": int(PR_NUMBER),
                            "state": "open",
                            "changed_files": len(paths),
                            "base": {
                                "repo": {"full_name": REPOSITORY},
                                "ref": "feat/next",
                                "sha": base_sha,
                            },
                            "head": {
                                "repo": {"full_name": REPOSITORY},
                                "ref": HEAD_REF,
                                "sha": head_sha,
                            },
                        }
                    ),
                    encoding="utf-8",
                )
                files.write_text(
                    json.dumps(
                        [
                            [
                                {"filename": path, "status": "modified"}
                                for path in paths
                            ]
                        ]
                    ),
                    encoding="utf-8",
                )
                return subprocess.run(
                    [
                        sys.executable,
                        str(classifier),
                        "--pull-request",
                        str(pull_request),
                        "--files",
                        str(files),
                        "--expected-repository",
                        REPOSITORY,
                        "--expected-head-repository",
                        REPOSITORY,
                        "--expected-head-ref",
                        HEAD_REF,
                        "--expected-number",
                        PR_NUMBER,
                        "--expected-base-ref",
                        "feat/next",
                        "--expected-base-sha",
                        base_sha,
                        "--expected-head-sha",
                        head_sha,
                        "--expected-state",
                        "open",
                        "--format",
                        "json",
                    ],
                    cwd=ROOT,
                    text=True,
                    capture_output=True,
                    check=False,
                )

            old_layout = classify(["scripts/test_version_prepare_workflow.py"])
            self.assertNotEqual(old_layout.returncode, 0)
            self.assertIn("has no CI owner", old_layout.stderr)
            fixed_layout = classify(
                [".github/tests/test_version_prepare_workflow.py"]
            )
            assert_success(self, fixed_layout)
            exact_diff = classify(changed_paths)
            assert_success(self, exact_diff)

    def test_nonbinary_path_uses_event_head_and_finishes(self) -> None:
        scenario = self.scenario()
        result, scope = self.classify(scenario)
        assert_success(self, result)
        self.assertEqual(scope["binary_impact"], "false")
        self.assertEqual(scope["windows_impact"], "false")

        result, selected = self.select(scenario, "false", "")
        assert_success(self, result)
        self.assertEqual(selected["final_head_sha"], HEAD_SHA)

        result, registered = self.register(scenario, HEAD_SHA)
        assert_success(self, result)
        self.assertEqual(registered["quality_required"], "true")

        context = final_context(
            scenario,
            head_sha=HEAD_SHA,
            binary_impact="false",
            windows_impact="false",
            version="",
        )
        result, _outputs = scenario.run(
            STEPS["Finalize required acceptance on the immutable head"],
            context=context,
        )
        assert_success(self, result)
        self.assertEqual(scenario.logs()[-1]["acceptance"], "success")

    def test_linux_binary_base_version_generates_and_publishes_exact_next(self) -> None:
        scenario = self.scenario(
            files=[{"filename": "src/server.rs", "status": "modified"}]
        )
        result, scope = self.classify(scenario)
        assert_success(self, result)
        self.assertEqual(scope["binary_impact"], "true")
        self.assertEqual(scope["windows_impact"], "false")

        result, prepared = scenario.run(
            STEPS["Validate and prepare version data"],
            reporter_mode="publish_rule",
        )
        assert_success(self, result)
        self.assertEqual(prepared["version"], "1.0.16")
        self.assertEqual(prepared["final_head_sha"], NEW_SHA)
        publication = scenario.logs()[-1]
        self.assertEqual(publication["command"], "publish-version")
        self.assertTrue(publication["transition_check"])
        self.assertEqual(publication["ref_sha"], NEW_SHA)
        self.assertEqual(publication["mutations"].count("PATCH"), 2)

        result, selected = self.select(scenario, "true", NEW_SHA)
        assert_success(self, result)
        self.assertEqual(selected["final_head_sha"], NEW_SHA)
        result, registered = self.register(scenario, NEW_SHA)
        assert_success(self, result)
        self.assertEqual(registered["quality_required"], "true")

        context = final_context(
            scenario,
            head_sha=NEW_SHA,
            binary_impact="true",
            windows_impact="false",
            version="1.0.16",
        )
        result, _outputs = scenario.run(
            STEPS["Finalize required acceptance on the immutable head"],
            context=context,
        )
        assert_success(self, result)
        self.assertEqual(scenario.logs()[-1]["acceptance"], "success")

    def test_windows_exact_next_uses_existing_head_without_version_mutation(self) -> None:
        scenario = self.scenario(
            files=[
                {
                    "filename": "windows-client/src/App.cs",
                    "status": "modified",
                }
            ],
            head_version="1.0.16",
        )
        result, scope = self.classify(scenario)
        assert_success(self, result)
        self.assertEqual(scope["binary_impact"], "true")
        self.assertEqual(scope["windows_impact"], "true")

        result, prepared = scenario.run(
            STEPS["Validate and prepare version data"]
        )
        assert_success(self, result)
        self.assertEqual(prepared["version"], "1.0.16")
        self.assertEqual(prepared["final_head_sha"], HEAD_SHA)
        self.assertEqual(scenario.state()["git_object_writes"], 0)
        self.assertEqual(scenario.logs(), [])

        result, selected = self.select(scenario, "true", HEAD_SHA)
        assert_success(self, result)
        self.assertEqual(selected["final_head_sha"], HEAD_SHA)
        result, registered = self.register(scenario, HEAD_SHA)
        assert_success(self, result)
        self.assertEqual(registered["quality_required"], "true")

        context = final_context(
            scenario,
            head_sha=HEAD_SHA,
            binary_impact="true",
            windows_impact="true",
            version="1.0.16",
        )
        result, _outputs = scenario.run(
            STEPS["Finalize required acceptance on the immutable head"],
            context=context,
        )
        assert_success(self, result)
        self.assertEqual(scenario.logs()[-1]["acceptance"], "success")

    def test_identity_version_and_ref_drift_fail_before_ref_publication(self) -> None:
        cases = (
            (
                {"after_head_sha": MOVED_SHA},
                "Classify pull request scope",
                "pull request moved",
            ),
            (
                {"head_version": "1.0.17"},
                "Validate and prepare version data",
                "must be base 1.0.15 or exact next patch 1.0.16",
            ),
            (
                {"ref_sha": MOVED_SHA},
                "Validate and prepare version data",
                "PR head moved before version preparation",
            ),
        )
        for arguments, step_name, message in cases:
            with self.subTest(step=step_name, arguments=arguments):
                scenario = self.scenario(**arguments)
                result, _outputs = scenario.run(STEPS[step_name])
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertEqual(scenario.logs(), [])
                expected_ref = arguments.get("ref_sha", HEAD_SHA)
                self.assertEqual(scenario.state()["ref_sha"], expected_ref)

    def test_missing_pr_number_reproduces_old_failure_and_fixed_step_passes(self) -> None:
        scenario = self.scenario(
            files=[{"filename": "src/server.rs", "status": "modified"}]
        )
        old_step = replace(
            STEPS["Validate and prepare version data"],
            env={
                key: value
                for key, value in STEPS[
                    "Validate and prepare version data"
                ].env.items()
                if key != "PR_NUMBER"
            },
        )
        result, _outputs = scenario.run(old_step, reporter_mode="publish_rule")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("PR_NUMBER: unbound variable", result.stderr)
        self.assertEqual(scenario.logs(), [])

        fixed = self.scenario(
            files=[{"filename": "src/server.rs", "status": "modified"}]
        )
        result, outputs = fixed.run(
            STEPS["Validate and prepare version data"],
            reporter_mode="publish_rule",
        )
        assert_success(self, result)
        self.assertEqual(outputs["final_head_sha"], NEW_SHA)

    def test_result_rejects_malformed_scope_and_missing_prepared_head(self) -> None:
        cases = (
            ("unexpected", HEAD_SHA, "change scope output is missing or malformed"),
            ("true", "", "final quality head is missing or malformed"),
        )
        for binary, prepared, message in cases:
            with self.subTest(binary=binary, prepared=prepared):
                scenario = self.scenario()
                result, _outputs = self.select(scenario, binary, prepared)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)

    def test_register_reuses_one_successful_pair_without_mutation(self) -> None:
        scenario = self.scenario()
        result, outputs = self.register(
            scenario, HEAD_SHA, mode="register_reuse"
        )
        assert_success(self, result)
        self.assertEqual(outputs["quality_required"], "false")
        self.assertEqual(scenario.logs()[-1]["mutations"], [])

    def test_each_quality_failure_reaches_terminal_acceptance_failure(self) -> None:
        for owner_result, candidate_result in (
            ("failure", "success"),
            ("success", "failure"),
        ):
            with self.subTest(
                owner_result=owner_result, candidate_result=candidate_result
            ):
                scenario = self.scenario()
                context = final_context(
                    scenario,
                    head_sha=HEAD_SHA,
                    binary_impact="false",
                    windows_impact="false",
                    version="",
                    owner_result=owner_result,
                    candidate_result=candidate_result,
                )
                result, _outputs = scenario.run(
                    STEPS["Finalize required acceptance on the immutable head"],
                    context=context,
                )
                self.assertNotEqual(result.returncode, 0)
                finalization = scenario.logs()[-1]
                self.assertEqual(finalization["command"], "finalize")
                self.assertFalse(finalization["ok"])
                self.assertEqual(finalization["acceptance"], "failure")


if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(
        VersionPrepareWorkflowLocalTests
    )
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    print(
        "version-prepare-workflow-local: "
        f"{'PASS' if result.wasSuccessful() else 'FAIL'} cases={result.testsRun}"
    )
    raise SystemExit(0 if result.wasSuccessful() else 1)

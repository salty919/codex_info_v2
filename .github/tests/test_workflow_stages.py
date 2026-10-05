#!/usr/bin/env python3
"""Issue #520: execute the feat caller and verify stage-specific job reachability."""

from __future__ import annotations

import ast
import json
import os
import re
import shutil
import subprocess  # nosec B404 # required tool API; individual execution calls remain reviewed.
import sys
import tempfile
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
GIT = shutil.which("git")
BASH = shutil.which("bash")
if any(path is None or not Path(path).is_absolute() for path in (GIT, BASH)):
    raise RuntimeError("offline workflow fixtures require absolute Git and Bash executables")


def workflow(name):
    return yaml.safe_load((ROOT / ".github/workflows" / name).read_text())


def expression(source, inputs):
    """Interpret the finite Actions boolean grammar, independent of the gate helper."""
    if not isinstance(source, str):
        return source
    source = source.removeprefix("${{").removesuffix("}}").strip()
    source = source.replace("&&", " and ").replace("||", " or ")
    source = re.sub(r"!(?!=)", " not ", source).strip()
    # The old optional native proof is absent in this fresh-run fixture.
    source = re.sub(r"steps\.native-proof\.[a-z_.]+", "''", source)
    names = {"inputs": inputs, "true": True, "false": False}

    def value(node):
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, ast.Name):
            return names[node.id]
        if isinstance(node, ast.Attribute):
            return value(node.value)[node.attr]
        if isinstance(node, ast.BoolOp):
            values = [bool(value(item)) for item in node.values]
            return all(values) if isinstance(node.op, ast.And) else any(values)
        if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.Not):
            return not value(node.operand)
        if isinstance(node, ast.Compare) and len(node.ops) == 1:
            left, right = value(node.left), value(node.comparators[0])
            if isinstance(node.ops[0], ast.Eq):
                return left == right
            if isinstance(node.ops[0], ast.NotEq):
                return left != right
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
            functions = {
                "fromJSON": json.loads,
                "toJSON": lambda item: json.dumps(item, separators=(",", ":")),
                "contains": lambda collection, item: item in collection,
                "always": lambda: True,
            }
            return functions[node.func.id](*(value(arg) for arg in node.args))
        raise AssertionError("unsupported workflow expression: " + source)

    return value(ast.parse(source, mode="eval").body)


class FeatCallerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="ci-stage-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.root / "repo"
        self.repo.mkdir()
        # Include the old planner so RED reaches the incorrect selection rather
        # than failing because the fixture omitted an existing dependency.
        for name in ("Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props",
                     "scripts/product_version.py", "scripts/ci_change_scope.py",
                     ".github/scripts/release_preflight.py"):
            source = ROOT / name
            if source.exists():
                target = self.repo / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
        self.git("init", "-q")
        self.git("config", "user.name", "Stage fixture")
        self.git("config", "user.email", "stage@example.invalid")
        self.write("docs/original.md", "fixed rename fixture\n")
        self.main = self.commit("main")
        self.write("windows-client/src/pending.cs", "pending other PR\n")
        self.base = self.commit("pending feat change")
        self.git("remote", "add", "origin", self.repo.as_uri())
        self.bin = self.root / "bin"
        self.bin.mkdir()
        gh = self.bin / "gh"
        gh.write_text("#!" + sys.executable + "\n" +
                      "import os, sys\n"
                      "endpoint = sys.argv[2]\n"
                      "if endpoint.endswith('/heads/main'): print(os.environ['FIXTURE_MAIN'])\n"
                      "elif endpoint.endswith('/heads/feat/next'): print(os.environ['FIXTURE_FEAT'])\n"
                      "else: raise SystemExit('unexpected GitHub request: ' + endpoint)\n")
        gh.chmod(0o755)

    def git(self, *args):
        # Only this test's local Git repository and literal fixture commands reach this call.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        return subprocess.check_output(  # nosec B603 # absolute Git and fixed offline fixture argv.
            [GIT, "-C", str(self.repo), *args], text=True, shell=False,
            stderr=subprocess.DEVNULL,
        ).strip()

    def write(self, name, content):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def commit(self, message):
        self.git("add", ".")
        self.git("commit", "-qm", message)
        return self.git("rev-parse", "HEAD")

    def classify(self, head, **overrides):
        output = self.root / "output"
        output.write_text("")
        env = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
               "PYTHONDONTWRITEBYTECODE": "1", "RUNNER_TEMP": str(self.root),
               "GITHUB_OUTPUT": str(output), "HEAD_REPOSITORY": "fixture/repo",
               "REPOSITORY": "fixture/repo", "BASE_SHA": self.base, "HEAD_SHA": head,
               "TRUSTED_SHA": self.main, "FIXTURE_MAIN": self.main, "FIXTURE_FEAT": self.base,
               **overrides}
        steps = workflow("feat-integration.yml")["jobs"]["classify"]["steps"]
        script = next(step["run"] for step in steps if step.get("id") == "classify")
        # Execute this checkout's workflow step with local Git and the finite GitHub stub above.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        result = subprocess.run(  # nosec B603 # trusted checked-in script, offline fixtures, no shell=True.
            [BASH, "-euo", "pipefail", "-c", script], cwd=self.repo,
            env=env, text=True, capture_output=True, shell=False, check=False,
        )
        outputs = dict(line.split("=", 1) for line in output.read_text().splitlines())
        return result, outputs

    def test_docs_pr_does_not_recheck_pending_feat_product_changes(self):
        self.write("docs/new.md", "this PR only\n")
        result, outputs = self.classify(self.commit("docs PR"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(outputs["selection_json"]), {
            "owners": ["DOCS"], "codeql_languages": [],
            "binary_impact": False, "distribution_required": False,
        })

    def test_native_pr_does_not_add_windows_or_distribution(self):
        self.write("src/feature.rs", "pub fn feature() {}\n")
        result, outputs = self.classify(self.commit("native PR"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(outputs["selection_json"]), {
            "owners": ["LINUX_BACKEND"], "codeql_languages": ["rust"],
            "binary_impact": True, "distribution_required": False,
        })

    def test_rename_keeps_both_responsibilities_without_pending_owners(self):
        (self.repo / "src").mkdir()
        self.git("mv", "docs/original.md", "src/renamed.rs")
        result, outputs = self.classify(self.commit("rename PR"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(outputs["selection_json"])["owners"],
                         ["DOCS", "LINUX_BACKEND"])

    def test_stale_base_and_source_without_current_base_remain_rejected(self):
        self.write("docs/new.md", "this PR\n")
        head = self.commit("docs PR")
        for source, overrides in ((head, {"FIXTURE_FEAT": head}), (self.main, {})):
            with self.subTest(source=source, overrides=overrides):
                result, outputs = self.classify(source, **overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("selection_json", outputs)


class WorkflowStageTests(unittest.TestCase):
    def inputs(self, release, owners, languages=(), binary=False):
        return {"release_candidate": release, "release_preflight": not release,
                "preflight_plan": "", "source_sha": "a" * 40,
                "selection_json": json.dumps({"owners": owners,
                    "codeql_languages": languages, "binary_impact": binary,
                    "distribution_required": release and binary})}

    def test_selected_jobs_are_stage_specific(self):
        jobs = workflow("selective-quality.yml")["jobs"]
        cases = (
            (False, ["DOCS"], [], False, {"docs-quality"}),
            (False, ["LINUX_BACKEND"], ["rust"], True, {"linux-backend-quality"}),
            (False, ["WINDOWS"], ["csharp"], True, {"windows-quality"}),
            (False, ["GOVERNANCE"], ["actions", "python"], False, {"governance-quality"}),
            (True, ["DOCS"], [], False, {"docs-quality", "selected-quality"}),
            (True, ["GOVERNANCE"], ["actions", "python"], False,
             {"governance-quality", "codeql-quality", "selected-quality"}),
            (True, ["LINUX_BACKEND", "WINDOWS"], ["rust"], True,
             {"linux-backend-quality", "windows-quality", "codeql-quality",
              "linux-distribution", "selected-quality"}),
            (True, ["LINUX_UI", "WINDOWS"], ["rust"], True,
             {"windows-quality", "codeql-quality", "linux-distribution", "selected-quality"}),
            (True, ["LINUX_UI"], [], False, {"linux-ui-quality", "selected-quality"}),
        )
        for release, owners, languages, binary, expected in cases:
            with self.subTest(release=release, owners=owners):
                inputs = self.inputs(release, owners, languages, binary)
                actual = {name for name, job in jobs.items()
                          if expression(job.get("if", True), inputs)}
                self.assertEqual(actual, expected)

    def test_release_behavior_and_candidates_execute_only_at_main(self):
        selective = workflow("selective-quality.yml")["jobs"]
        cases = (
            ("linux-backend-quality", "rust.yml", "native-quality",
             {"Run native unit tests with coverage", "Reject native compiler and Clippy warnings",
              "Verify recorder and REST compile-time boundary"},
             set()),
            ("windows-quality", "windows-client.yml", "windows-quality",
             {"Run Windows unit tests"}, {"Build standard Windows setup wizard",
              "Upgrade latest published Windows release to the exact candidate",
              "Run installed Windows UI Automation E2E", "Upload release candidate"}),
            ("linux-ui-quality", "linux-ui-quality.yml", "linux-ui-quality",
             {"Build native release for UI evaluation", "Run graph UI image acceptance"},
             set()),
        )
        for release in (False, True):
            for caller, file, job_id, ordinary, final in cases:
                with self.subTest(release=release, file=file):
                    inputs = self.inputs(release, ["LINUX_BACKEND", "LINUX_UI", "WINDOWS"], binary=True)
                    bindings = selective[caller]["with"]
                    leaf = {"release_preflight": False, "preflight_plan": ""}
                    for key in ("release_candidate", "release_preflight", "preflight_plan"):
                        if key in bindings:
                            leaf[key] = expression(bindings[key], inputs)
                    steps = workflow(file)["jobs"][job_id]["steps"]
                    reached = {step.get("name") for step in steps
                               if step.get("name") in ordinary | final
                               and expression(step.get("if", True), leaf)}
                    self.assertTrue(ordinary <= reached, ordinary - reached)
                    self.assertEqual(final & reached, final if release else set())

    def test_main_final_head_is_evaluated_without_a_reuse_dependency(self):
        jobs = workflow("version-prepare.yml")["jobs"]
        self.assertEqual(set(jobs), {"version-prepared", "selective-quality", "acceptance"})
        quality = jobs["selective-quality"]
        self.assertEqual(quality["needs"], ["version-prepared"])
        self.assertEqual(quality["with"]["source_sha"], "${{ needs.version-prepared.outputs.quality_sha }}")
        self.assertIs(quality["with"]["release_candidate"], True)
        self.assertEqual(jobs["acceptance"]["needs"], ["version-prepared", "selective-quality"])

    def test_feat_does_not_project_a_release_or_produce_reuse_evidence(self):
        feat = workflow("feat-integration.yml")
        call = feat["jobs"]["selective-quality"]["with"]
        self.assertEqual(call["base_sha"], "${{ github.event.pull_request.base.sha }}")
        self.assertIs(call["release_candidate"], False)
        for file in ("feat-integration.yml", "selective-quality.yml", "version-prepare.yml",
                     "rust.yml", "windows-client.yml", "linux-ui-quality.yml", "linux-distribution.yml"):
            text = (ROOT / ".github/workflows" / file).read_text()
            for obsolete in ("release_preflight", "preflight_plan", "native_reuse_artifact",
                             "native_quality_proof.py"):
                with self.subTest(file=file, obsolete=obsolete):
                    self.assertNotIn(obsolete, text)


class LinuxBuildFlowTests(unittest.TestCase):
    def test_dependency_cache_preserves_parallel_quality_and_excludes_product(self):
        jobs = workflow('selective-quality.yml')['jobs']
        for job in ('linux-backend-quality', 'linux-distribution', 'windows-quality', 'codeql-quality'):
            self.assertNotIn('needs', jobs[job], job)
        for name, target in (('rust.yml', '. -> target'), ('linux-ui-quality.yml', '. -> target'),
                             ('linux-distribution.yml', '. -> target/distribution')):
            with self.subTest(workflow=name):
                steps = next(iter(workflow(name)['jobs'].values()))['steps']
                caches = [step for step in steps if step.get('uses', '').startswith('Swatinem/rust-cache@')]
                self.assertEqual(len(caches), 1, name)
                cache = caches[0]
                self.assertRegex(cache['uses'], r'@[a-f0-9]{40}$')
                self.assertEqual(cache['with']['workspaces'], target)
                self.assertEqual(cache['with']['cache-workspace-crates'], 'false')
                self.assertEqual(cache['with']['cache-bin'], 'false')
                self.assertEqual(cache['with']['env-vars'], 'ImageOS')
                toolchain = next(i for i, step in enumerate(steps)
                                 if step.get('uses', '').startswith('dtolnay/rust-toolchain@'))
                self.assertLess(toolchain, steps.index(cache))
                # A cache hit never replaces compilation or the tests themselves.
                for step in steps:
                    self.assertNotIn('cache-hit', str(step.get('if', '')))

    def test_distribution_build_keeps_cached_host_outputs_away_from_acceptance(self):
        steps = workflow('linux-distribution.yml')['jobs']['linux-distribution']['steps']
        build = next(step['run'] for step in steps if step.get('name') == 'Build the exact Linux target')
        bundle = next(step['run'] for step in steps if step.get('id') == 'bundle')
        self.assertIn('--target-dir target/distribution', build)
        self.assertIn('target/distribution/$LINUX_BUNDLE_TARGET/release/', bundle)
        self.assertIn('candidate_root="$GITHUB_WORKSPACE/target/release"', bundle)

    def inputs(self, release, owners, binary=True):
        return WorkflowStageTests().inputs(release, owners, binary=binary)

    def bindings(self, caller, inputs):
        return {key: expression(value, inputs) for key, value in caller['with'].items()
                if key in ('release_candidate', 'linux_backend_acceptance', 'linux_ui_acceptance')}

    def test_single_release_build_and_ui_only_ordinary_build(self):
        selective = workflow('selective-quality.yml')['jobs']
        for release, owners in ((True, ['LINUX_BACKEND', 'LINUX_UI', 'WINDOWS']),
                                (False, ['LINUX_UI'])):
            with self.subTest(release=release):
                inputs = self.inputs(release, owners)
                builds = []
                for job_id, name in (('linux-backend-quality', 'rust.yml'),
                                     ('linux-ui-quality', 'linux-ui-quality.yml'),
                                     ('linux-distribution', 'linux-distribution.yml')):
                    caller = selective[job_id]
                    if not expression(caller['if'], inputs):
                        continue
                    bindings = self.bindings(caller, inputs)
                    for job in workflow(name)['jobs'].values():
                        for step in job['steps']:
                            if expression(step.get('if', True), bindings):
                                builds.extend(line.strip() for line in step.get('run', '').splitlines()
                                              if 'cargo build ' in line)
                self.assertEqual(len(builds), 1, builds)
                if release:
                    self.assertIn('--target "$LINUX_BUNDLE_TARGET"', builds[0])
                else:
                    self.assertEqual(builds, ['cargo build --release --locked -p codex_info'])

    def run_candidate(self, owners, fail_at=''):
        with tempfile.TemporaryDirectory(prefix='candidate-flow-') as directory:
            root = Path(directory)
            (root / 'scripts').mkdir()
            (root / 'bin').mkdir()
            (root / 'runner').mkdir()
            calls = root / 'calls'

            def script(path, body):
                target = root / path
                target.write_text('#!/bin/bash\nset -euo pipefail\n' + body)
                target.chmod(0o755)

            script('bin/cargo', '''
                printf 'build\\n' >> "$CALLS"
                target=target
                while (($#)); do
                    if [[ "$1" == --target-dir ]]; then target="$2"; shift; fi
                    shift
                done
                mkdir -p "$target/x86_64-unknown-linux-gnu/release"
                for name in codex_info codex_info_recorder codex_info_rest; do
                    printf 'packaged:%s\\n' "$name" > "$target/x86_64-unknown-linux-gnu/release/$name"
                    chmod +x "$target/x86_64-unknown-linux-gnu/release/$name"
                done
''')
            script('bin/xvfb-run', '''
                while [[ "$1" == --* ]]; do shift; done
                exec "$@"
''')
            script('scripts/build_linux_bundle.sh', '''
                printf 'bundle\\n' >> "$CALLS"
                while (($#)); do
                    case "$1" in
                        --output-dir) output="$2"; shift 2;;
                        --ui-binary) payload="$(dirname "$2")"; shift 2;;
                        *) shift 2;;
                    esac
                done
                tar -czf "$output/codex-info-1.0.0-x86_64-unknown-linux-gnu.tar.gz" -C "$payload" .
                printf '{}\\n' > "$output/fixture.sha256"
                printf '{"version":"1.0.0"}\\n' > "$output/fixture.manifest.json"
''')
            script('scripts/test_linux_bundle.sh', 'printf "bundle-check\\n" >> "$CALLS"\n')
            names = {'cli_contract_e2e.sh': 'cli', 'record_daemon_e2e.sh': 'daemon',
                     'x11_startup_visual_gate.sh': 'startup', 'x11_graph_visual_gate.sh': 'graph',
                     'x11_service_recovery_visual_gate.sh': 'recovery'}
            # Preserve the real startup caller's nested recovery invocation.
            recovery_call = next(line for line in (ROOT / 'scripts/x11_startup_visual_gate.sh').read_text().splitlines()
                                 if line.startswith('bash ') and 'x11_service_recovery_visual_gate.sh' in line)
            for name, label in names.items():
                body = 'root_dir="$PWD"\n'
                body += 'payload="${CODEX_INFO_ACCEPTANCE_BINARY:-$PWD/target/release/codex_info}"\n'
                body += 'payload="$(dirname "$payload")"\n'
                body += 'for name in codex_info codex_info_recorder codex_info_rest; do\n'
                body += '  [[ -x "$payload/$name" && "$(cat "$payload/$name")" == "packaged:$name" ]]\ndone\n'
                body += f'printf "{label}\\n" >> "$CALLS"\n'
                body += f'[[ "$FAIL_AT" != "{label}" ]] || exit 23\n'
                if label == 'startup':
                    body += recovery_call + '\n'
                script('scripts/' + name, body)
            caller = workflow('selective-quality.yml')['jobs']['linux-distribution']
            bindings = self.bindings(caller, self.inputs(True, owners))
            job = workflow('linux-distribution.yml')['jobs']['linux-distribution']
            env = {**os.environ, 'PATH': str(root / 'bin') + os.pathsep + os.environ['PATH'],
                   'GITHUB_WORKSPACE': str(root), 'RUNNER_TEMP': str(root / 'runner'),
                   'GITHUB_OUTPUT': str(root / 'output'), 'CALLS': str(calls), 'FAIL_AT': fail_at,
                   'LINUX_BUNDLE_TARGET': 'x86_64-unknown-linux-gnu', 'PR_NUMBER': '1',
                   'SOURCE_SHA': 'a' * 40, 'RUN_ID': '2', 'RUN_ATTEMPT': '1'}
            result = None
            for step in job['steps']:
                if step.get('name') not in ('Build the exact Linux target', 'Create and validate the Linux bundle'):
                    continue
                for key, value in step.get('env', {}).items():
                    if key in ('RUN_BACKEND_ACCEPTANCE', 'RUN_UI_ACCEPTANCE'):
                        env[key] = str(expression(value, bindings)).lower()
                # Execute the checked-in workflow against finite local command fixtures only.
                # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
                result = subprocess.run(  # nosec B603 # absolute Bash, trusted workflow, fixed offline fixtures.
                    [BASH, '-euo', 'pipefail', '-c', step['run']], cwd=root, env=env,
                    text=True, capture_output=True, shell=False, check=False)
                if result.returncode:
                    break
            return result, calls.read_text().splitlines()

    def test_candidate_uses_one_build_and_one_recovery_per_selected_path(self):
        for owners, expected in ((['LINUX_BACKEND', 'LINUX_UI'],
                                  ['cli', 'daemon', 'startup', 'recovery', 'graph']),
                                 (['LINUX_BACKEND'], ['cli', 'daemon', 'recovery']),
                                 (['LINUX_UI'], ['startup', 'recovery', 'graph']),
                                 (['WINDOWS'], ['recovery'])):
            with self.subTest(owners=owners):
                result, calls = self.run_candidate(owners)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, ['build', 'bundle', 'bundle-check', *expected])

    def test_candidate_stops_on_acceptance_failure(self):
        result, calls = self.run_candidate(['LINUX_BACKEND', 'LINUX_UI'], fail_at='daemon')
        self.assertEqual(result.returncode, 23, result.stderr)
        self.assertEqual(calls, ['build', 'bundle', 'bundle-check', 'cli', 'daemon'])


if __name__ == "__main__":
    unittest.main()

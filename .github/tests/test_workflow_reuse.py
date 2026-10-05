#!/usr/bin/env python3
"""Execute real CI caller scripts with finite, offline GitHub/tool fixtures."""
from __future__ import annotations

import copy
import hashlib
import json
import os
import shutil
import subprocess  # nosec B404 # required tool API; individual execution calls remain reviewed.
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
GIT = shutil.which("git")
BASH = shutil.which("bash")
if any(path is None or not Path(path).is_absolute() for path in (GIT, BASH)):
    raise RuntimeError("fixtures require absolute Git and Bash executables")
REPORT = Path("artifacts/codacy-coverage-rust/rust.cobertura.xml")


def step(file, name):
    document = yaml.safe_load((ROOT / ".github/workflows" / file).read_text())
    return next(s for job in document["jobs"].values() for s in job.get("steps", []) if s.get("name") == name)


class WorkflowReuseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.case = Path(self.tmp.name)
        self.repo = self.case / "repo"
        self.repo.mkdir()
        files = ("Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props",
                 ".github/scripts/release_preflight.py", ".github/scripts/native_quality_proof.py",
                 "scripts/product_version.py", "scripts/ci_change_scope.py")
        for name in files:
            destination = self.repo / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)
        self.git("init", "-q"); self.git("config", "user.name", "fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("add", "."); self.git("commit", "-qm", "trusted base")
        self.base = self.git("rev-parse", "HEAD").strip()
        (self.repo / "src").mkdir()
        (self.repo / "src/main.rs").write_text("fn main() {}\n")
        self.git("add", "."); self.git("commit", "-qm", "product")
        self.feature = self.git("rev-parse", "HEAD").strip()
        self.git("remote", "add", "origin", self.repo.as_uri())
        self.bin = self.case / "bin"; self.bin.mkdir()
        self.runner = self.case / "runner"; self.runner.mkdir()
        python_bin = self.runner / "native-proof-venv/bin"
        python_bin.mkdir(parents=True)
        # A symlink in another bin directory loses this venv's pyvenv.cfg.
        # Use the current verified interpreter without downloading per fixture.
        launcher = python_bin / "python"
        launcher.write_text(
            "#!" + sys.executable + "\nimport os, sys\n"
            + f"os.execv({sys.executable!r}, [{sys.executable!r}, *sys.argv[1:]])\n"
        )
        launcher.chmod(0o755)
        self.calls = self.case / "calls.jsonl"
        self.state = self.case / "api.json"
        self.output = self.case / "output"
        self.environment = {**os.environ, "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "PYTHONDONTWRITEBYTECODE": "1", "RUNNER_TEMP": str(self.runner), "RUNNER_OS": "Linux",
            "GITHUB_OUTPUT": str(self.output), "GITHUB_RUN_ID": "20", "GITHUB_RUN_ATTEMPT": "1",
            "ImageOS": "ubuntu24", "ImageVersion": "exact", "REPOSITORY": "owner/repo",
            "SOURCE_SHA": self.feature, "HEAD_SHA": self.feature, "BASE_SHA": self.base,
            "HEAD_REPOSITORY": "owner/repo", "MAIN_BASE_SHA": self.base, "TRUSTED_SHA": self.base,
            "PR_NUMBER": "7", "RELEASE_CANDIDATE": "false",
            "FIXTURE_STATE": str(self.state), "FIXTURE_CALLS": str(self.calls), "FIXTURE_ROOT": str(ROOT)}
        self.executable("gh", '''import base64,json,os,pathlib,sys
endpoint=sys.argv[2]
with open(os.environ['FIXTURE_CALLS'],'a') as f: f.write(json.dumps(['gh',endpoint])+'\\n')
if os.environ.get('FIXTURE_API_FAILURE') == 'true': sys.exit(42)
state=json.loads(pathlib.Path(os.environ['FIXTURE_STATE']).read_text())
if '/contents/' in endpoint:
    name=endpoint.split('/contents/',1)[1].split('?ref=',1)[0]
    if endpoint.split('?ref=',1)[-1] != os.environ['TRUSTED_SHA']: sys.exit(43)
    sys.stdout.write(base64.b64encode((pathlib.Path(os.environ['FIXTURE_ROOT'])/name).read_bytes()).decode())
elif endpoint in state:
    value=state[endpoint]
    if isinstance(value,dict) and 'binary_file' in value:
        sys.stdout.buffer.write(pathlib.Path(value['binary_file']).read_bytes())
    elif '--jq' in sys.argv: print(value['object']['sha'])
    else: print(json.dumps(value))
else: sys.exit(44)
''')
        self.executable("cargo", '''import json,os,pathlib,sys
args=sys.argv[1:]
if '-V' in args:
    print(' '.join(args)+' exact'+os.environ.get('FIXTURE_TOOL_CHANGE','')); sys.exit(0)
with open(os.environ['FIXTURE_CALLS'],'a') as f: f.write(json.dumps(['cargo',*args])+'\\n')
if args[0]=='llvm-cov':
    p=pathlib.Path(args[args.index('--output-path')+1]); p.parent.mkdir(parents=True,exist_ok=True)
    p.write_text('<!DOCTYPE coverage SYSTEM "https://cobertura.sourceforge.net/xml/coverage-04.dtd"><coverage lines-valid="2"><packages><package><classes><class filename="src/main.rs"/></classes></package></packages></coverage>')
if args[0]=='clippy' and os.environ.get('FIXTURE_CLIPPY_FAILURE')=='true': sys.exit(7)
''')
        self.executable("rustc", "print('rustc exact')\n")
        self.executable("dpkg-query", "import os; print('packages exact'+os.environ.get('FIXTURE_PACKAGE_CHANGE',''))\n")
        self.executable("xvfb-run", '''import os,sys
args=sys.argv[1:]
while args and args[0].startswith('-'): args.pop(0)
os.execvp(args[0],args)
''')
        for name in ("cli_contract_e2e.sh", "record_daemon_e2e.sh", "check_recorder_rest_boundary.sh"):
            (self.repo / "scripts" / name).write_text(
                'printf \'["behavior", "%s"]\\n\' "' + name + '" >> "$FIXTURE_CALLS"\n'
                + ('if [[ "${FIXTURE_DAEMON_FAILURE:-}" == true ]]; then exit 9; fi\n' if name == "record_daemon_e2e.sh" else ""))
        self.api = {"repos/owner/repo/git/ref/heads/main": {"object": {"sha": self.base}},
                    "repos/owner/repo/git/ref/heads/feat/next": {"object": {"sha": self.base}}}
        self.save_state()
        self.output.write_text("")

    def executable(self, name, source):
        path = self.bin / name
        path.write_text("#!" + sys.executable + "\n" + source)
        path.chmod(0o755)

    def git(self, *args):
        # Only this test's local Git repository and literal fixture commands reach this call.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        return subprocess.check_output(  # nosec B603 # absolute Git and fixed offline fixture argv.
            [GIT, "-C", str(self.repo), *args], text=True, shell=False, stderr=subprocess.DEVNULL,
        )

    def save_state(self):
        self.state.write_text(json.dumps(self.api))

    def run_script(self, source, env=None, ok=True):
        # source is checked-in workflow text or a literal fixture, run against local tool stubs.
        # Intentionally execute checked-in caller text/literal fixtures against offline tool stubs.
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        result = subprocess.run(  # nosec B603 # finite trusted caller script, offline stubs, no shell=True.
            [BASH, "-euo", "pipefail", "-c", source], cwd=self.repo,
            env={**self.environment, **(env or {})}, text=True, capture_output=True,
            shell=False, check=False,
        )
        if ok: self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def outputs(self):
        return dict(line.split("=", 1) for line in self.output.read_text().splitlines() if "=" in line)

    def prepare_producer(self):
        result = self.run_script("python3 .github/scripts/release_preflight.py plan --source \"$SOURCE_SHA\" --main \"$MAIN_BASE_SHA\" --workflow \"$TRUSTED_SHA\"")
        plan = json.loads(result.stdout)
        self.environment["PLAN"] = json.dumps(plan, separators=(",", ":"))
        self.run_script(step("rust.yml", "Reconstruct the trusted planned release tree")["run"])
        self.environment.update({"GITHUB_RUN_ID": "10", "GITHUB_RUN_ATTEMPT": "2"})
        self.run_script(step("rust.yml", "Match actual native runtime before reuse")["run"])
        self.run_script(step("rust.yml", "Run native unit tests with coverage")["run"])
        self.run_script(step("rust.yml", "Reject native compiler and Clippy warnings")["run"])
        self.run_script(step("rust.yml", "Bind successful native checks to exact inputs")["run"],
                        {"NATIVE_INPUTS": self.outputs().get("inputs_json", ""),
                         "EXPECTED_HELPER_DIGEST": self.outputs().get("helper_digest", "")})
        report = (self.repo / REPORT).read_bytes()
        archive = self.case / "report.zip"
        with zipfile.ZipFile(archive, "w") as package: package.writestr("rust.cobertura.xml", report)
        digest = "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest()
        run = {"id": 10, "run_attempt": 2, "head_sha": self.feature, "status": "completed", "conclusion": "success",
               "event": "pull_request_target", "path": ".github/workflows/feat-integration.yml",
               "repository": {"full_name": "owner/repo"},
               "display_title": f"codex-feat-preflight-v1:pr=7:event_head={self.feature}",
               "referenced_workflows": [{"path": f"owner/repo/.github/workflows/{name}@{self.base}", "sha": self.base}
                                        for name in ("rust.yml", "selective-quality.yml")]}
        job = {"name": "Run selected advisory quality / linux-backend-quality / native-quality",
               "status": "completed", "conclusion": "success",
               "steps": [{"name": name, "status": "completed", "conclusion": "success"} for name in
                         ("Reconstruct the trusted planned release tree", "Run native unit tests with coverage",
                          "Reject native compiler and Clippy warnings", "Bind successful native checks to exact inputs")]}
        artifact = {"id": 77, "expired": False, "size_in_bytes": archive.stat().st_size, "digest": digest,
                    "name": f"codacy-coverage-rust-v1-head-{self.feature}-run-10-attempt-2"}
        self.api.update({
            "repos/owner/repo/actions/workflows/feat-integration.yml/runs?event=pull_request_target&per_page=30": {"workflow_runs": [run]},
            "repos/owner/repo/actions/runs/10/artifacts?per_page=100": {"total_count": 1, "artifacts": [artifact]},
            "repos/owner/repo/actions/artifacts/77/zip": {"binary_file": str(archive)},
            "repos/owner/repo/actions/runs/10/attempts/2/jobs?per_page=100": {"total_count": 1, "jobs": [job]},
        })
        self.save_state()
        self.git("add", "--", "Cargo.toml", "Cargo.lock", "windows-client/Directory.Build.props")
        self.git("commit", "-qm", "actual main version tree")
        self.quality = self.git("rev-parse", "HEAD").strip()
        self.assertEqual(self.git("rev-parse", "HEAD^{tree}").strip(), plan["expected_tree"])
        self.environment.update({"SOURCE_SHA": self.quality, "PLAN": "", "GITHUB_RUN_ID": "20", "GITHUB_RUN_ATTEMPT": "1", "RELEASE_CANDIDATE": "true"})
        self.git("checkout", "-q", "--detach", self.base)  # Actual producer executes trusted checkout, not H1.
        self.output.write_text(""); self.calls.write_text("")
        (self.repo / REPORT).unlink()

    def resolve(self, env=None):
        self.run_script(step("version-prepare.yml", "Fetch exact quality objects without checking out PR code")["run"], env)
        self.run_script(step("version-prepare.yml", "Resolve exact prior native quality evidence")["run"], env)
        return self.outputs().get("artifact", "")

    def leaf(self, bridge=True, env=None, daemon_failure=False, tamper_helper=False):
        self.git("checkout", "-q", "--detach", self.quality)
        if bridge and self.outputs().get("artifact"):
            target = self.runner / "native-reuse-download" / "rust.cobertura.xml"
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(self.runner / "native-reuse/rust.cobertura.xml", target)
        self.output.write_text("")
        result = self.run_script(step("rust.yml", "Match actual native runtime before reuse")["run"], env, ok=False)
        reused = result.returncode == 0 and self.outputs().get("reused") == "true"
        for name in ("Run native unit tests with coverage", "Reject native compiler and Clippy warnings"):
            actual = step("rust.yml", name)
            self.assertEqual(actual["if"], "steps.native-proof.outputs.reused != 'true'")
            if not reused: self.run_script(actual["run"], env)
        if result.returncode == 0:
            if tamper_helper:
                (self.runner / "native_quality_proof.py").write_text("raise SystemExit(91)\n")
            binding = step("rust.yml", "Bind successful native checks to exact inputs")
            bound = self.run_script(binding["run"],
                            {**(env or {}), "NATIVE_INPUTS": self.outputs().get("inputs_json", ""),
                             "EXPECTED_HELPER_DIGEST": self.outputs().get("helper_digest", "")}, ok=False)
            if bound.returncode:
                self.assertIs(binding["continue-on-error"], True)
        self.assertGreater((self.repo / REPORT).stat().st_size, 0)
        behavior_result = None
        for name in ("Build native release", "Verify recorder and REST compile-time boundary", "Run public CLI lifecycle acceptance", "Run recorder daemon live acceptance"):
            behavior_result = self.run_script(step("rust.yml", name)["run"],
                           {**(env or {}), "FIXTURE_DAEMON_FAILURE": "true" if daemon_failure else "false"}, ok=not daemon_failure)
        return reused, behavior_result.returncode

    def native_calls(self):
        return [item[1] for item in map(json.loads, self.calls.read_text().splitlines()) if item[0] == "cargo"]

    def test_parser_setup_failure_keeps_native_checks_fresh(self):
        self.prepare_producer()
        self.assertTrue(self.resolve())
        (self.runner / "native-proof-venv/bin/python").unlink()
        probe = step("rust.yml", "Match actual native runtime before reuse")
        resolver = step("version-prepare.yml", "Resolve exact prior native quality evidence")
        self.assertEqual(probe["if"], "steps.native-parser.outcome == 'success'")
        self.assertEqual(resolver["if"], "steps.native-parser.outcome == 'success'")
        reused, result = self.leaf()
        self.assertFalse(reused)
        self.assertEqual(result, 0)
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_isolated_parser_ignores_product_pythonpath(self):
        self.prepare_producer()
        self.assertTrue(self.resolve())
        shadow = self.case / "shadow/defusedxml"
        shadow.mkdir(parents=True)
        (shadow / "__init__.py").write_text('raise SystemExit("UNTRUSTED_XML_MODULE")\n')
        reused, result = self.leaf(env={"PYTHONPATH": str(shadow.parent)})
        self.assertTrue(reused)
        self.assertEqual(result, 0)

    def test_real_resolver_bridge_leaf_reuses_only_two_checks(self):
        self.prepare_producer()
        self.assertEqual(self.git("rev-parse", "HEAD").strip(), self.base)
        self.assertTrue(self.resolve())
        reused, result = self.leaf(daemon_failure=True)
        self.assertTrue(reused)
        self.assertEqual(result, 9, "daemon failure must still reach the workflow result")
        self.assertEqual(self.native_calls(), ["build"])

    def test_source_helper_never_replaces_the_read_only_resolver(self):
        self.prepare_producer()
        self.git("checkout", "-q", "--detach", self.quality)
        helper = self.repo / ".github/scripts/native_quality_proof.py"
        helper.write_text('raise SystemExit("UNTRUSTED_HELPER_EXECUTED")\n')
        self.git("add", "."); self.git("commit", "-qm", "untrusted helper")
        self.quality = self.git("rev-parse", "HEAD").strip()
        self.environment["SOURCE_SHA"] = self.quality
        self.git("checkout", "-q", "--detach", self.base)
        trusted_helper = helper.read_bytes()
        self.assertEqual(self.resolve(), "", "changed source tree must receive fresh checks")
        self.assertEqual(self.git("rev-parse", "HEAD").strip(), self.base)
        self.assertEqual(helper.read_bytes(), trusted_helper)
        self.assertEqual(self.git("diff", "--name-only"), "")

    def test_resolver_fetch_failure_leaves_native_checks_fresh(self):
        self.prepare_producer()
        fetch = step("version-prepare.yml", "Fetch exact quality objects without checking out PR code")
        result = self.run_script(fetch["run"], {"SOURCE_SHA": "0" * 40}, ok=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.git("rev-parse", "HEAD").strip(), self.base)
        self.assertFalse(self.leaf(bridge=False)[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_api_failure_runs_native_checks_fresh(self):
        self.prepare_producer()
        env = {"FIXTURE_API_FAILURE": "true"}
        self.assertEqual(self.resolve(env), "")
        reused, result = self.leaf(bridge=False, env=env)
        self.assertFalse(reused); self.assertEqual(result, 0)
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_download_failure_runs_native_checks_fresh(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.assertFalse(self.leaf(bridge=False)[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_runtime_change_runs_native_checks_fresh(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.assertFalse(self.leaf(env={"ImageVersion": "new-image"})[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_toolchain_change_runs_native_checks_fresh(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.assertFalse(self.leaf(env={"FIXTURE_TOOL_CHANGE": " updated"})[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_installed_system_package_change_runs_native_checks_fresh(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.assertFalse(self.leaf(env={"FIXTURE_PACKAGE_CHANGE": " updated"})[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_missing_runner_identity_never_creates_reusable_proof(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.assertFalse(self.leaf(env={"ImageVersion": ""})[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])
        self.assertNotIn(b"codex-native-proof", (self.repo / REPORT).read_bytes())

    def test_emit_reacquires_trusted_helper_after_product_tampering(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.assertTrue(self.leaf(tamper_helper=True)[0])
        self.assertNotEqual((self.runner / "native_quality_proof.py").read_text(), "raise SystemExit(91)\n")
        self.assertIn(b"main-quality", (self.repo / REPORT).read_bytes())

    def test_changed_workflow_definition_runs_native_checks_fresh(self):
        self.prepare_producer()
        # A new trusted workflow is actually checked out on the new runner.
        # A fabricated SHA cannot satisfy its HEAD binding.
        workflow = self.repo / ".github/workflows/fixture.yml"
        workflow.parent.mkdir(parents=True, exist_ok=True)
        workflow.write_text("name: updated trusted workflow\n")
        self.git("add", "--", ".github/workflows/fixture.yml"); self.git("commit", "-qm", "trusted workflow update")
        changed = {"TRUSTED_SHA": self.git("rev-parse", "HEAD").strip()}
        self.assertEqual(self.resolve(changed), "")
        self.assertFalse(self.leaf(env=changed)[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_changed_lockfile_tree_runs_native_checks_fresh(self):
        self.prepare_producer()
        self.git("checkout", "-q", "--detach", self.quality)
        with (self.repo / "Cargo.lock").open("a") as stream: stream.write("\n# dependency refresh fixture\n")
        self.git("add", "Cargo.lock"); self.git("commit", "-qm", "dependency input changed")
        self.quality = self.git("rev-parse", "HEAD").strip()
        self.environment["SOURCE_SHA"] = self.quality
        self.git("checkout", "-q", "--detach", self.base)
        self.assertEqual(self.resolve(), "")
        self.assertFalse(self.leaf()[0]); self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_main_advance_after_resolve_still_runs_leaf_checks_fresh(self):
        self.prepare_producer(); self.assertTrue(self.resolve())
        self.api["repos/owner/repo/git/ref/heads/main"]["object"]["sha"] = self.quality
        self.save_state()
        self.assertFalse(self.leaf()[0])
        self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_latest_matching_failed_run_never_uses_older_success(self):
        self.prepare_producer()
        listing = self.api["repos/owner/repo/actions/workflows/feat-integration.yml/runs?event=pull_request_target&per_page=30"]
        latest = copy.deepcopy(listing["workflow_runs"][0]); latest.update({"id": 11, "conclusion": "failure"})
        listing["workflow_runs"].insert(0, latest); self.save_state()
        self.assertEqual(self.resolve(), "")
        self.assertFalse(self.leaf()[0]); self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])
        self.assertNotIn("/artifacts?", self.calls.read_text())

    def test_bad_digest_runs_native_checks_fresh(self):
        self.prepare_producer()
        self.api["repos/owner/repo/actions/runs/10/artifacts?per_page=100"]["artifacts"][0]["digest"] = "sha256:" + "0" * 64
        self.save_state(); self.assertEqual(self.resolve(), "")
        self.assertFalse(self.leaf()[0]); self.assertEqual(self.native_calls(), ["llvm-cov", "clippy", "build"])

    def test_actual_main_advance_discards_old_event_proof(self):
        self.prepare_producer()
        self.api["repos/owner/repo/git/ref/heads/main"]["object"]["sha"] = self.quality
        self.save_state()
        self.assertEqual(self.resolve(), "", "old event base cannot authorize reuse after main advances")

    def test_feat_classifier_selects_pending_release_owners_and_rejects_stale_base(self):
        self.output.write_text("")
        script = step("feat-integration.yml", "Classify the event's exact base and head")["run"]
        self.run_script(script)
        selection = json.loads(self.outputs()["selection_json"])
        self.assertIn("WINDOWS", selection["owners"])
        self.assertTrue(selection["distribution_required"])
        self.assertTrue(json.loads(self.outputs()["preflight_plan"])["main_included"])
        self.api["repos/owner/repo/git/ref/heads/feat/next"]["object"]["sha"] = self.feature
        self.save_state()
        self.assertNotEqual(self.run_script(script, ok=False).returncode, 0,
                            "an event with an advanced feat base must not appear green")
        self.environment["BASE_SHA"] = self.feature
        self.environment["HEAD_SHA"] = self.base
        self.assertNotEqual(self.run_script(script, ok=False).returncode, 0,
                            "a source that lacks the current feat base must not appear green")


if __name__ == "__main__":
    unittest.main()

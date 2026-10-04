"""Offline update-entrypoint regressions for recorder overrides.

The oracle is the requested process path/hash, never UI or health version.
Only an isolated HOME, fake proc tree and trusted fixture bundles are used.
"""

import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "packaging/install_linux_bundle.sh"
BUNDLE_TEST = ROOT / "scripts/test_linux_bundle.sh"
BUILDER = ROOT / "scripts/build_linux_bundle.sh"
RECORDER_PID = 70001
REST_PID = 70002


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def heredoc(name):
    source = BUNDLE_TEST.read_text(encoding="utf-8")
    matches = re.findall(rf"(?ms)^cat > .* <<'{re.escape(name)}'\n(.*?)^{re.escape(name)}$", source)
    if len(matches) != 1:
        raise AssertionError(f"expected one existing offline fixture: {name}")
    return matches[0]


# This adapter models ExecStart resolution independently of installer code.
# The existing offline systemctl fixture still owns enabled/active transitions.
SYSTEMCTL_ADAPTER = r'''#!/usr/bin/python3
import json, os, pathlib, shlex, subprocess, sys
args = sys.argv[1:]
values = [item for item in args if item != "--user"]
action = values[0] if values else ""
units = [item for item in values if item.endswith(".service")]
unit = units[-1] if units else ""
home = pathlib.Path(os.environ["HOME"])
unit_dir = home / ".config/systemd/user"
proc = pathlib.Path(os.environ["FAKE_PROC_ROOT"])
def execution():
    fragment = unit_dir / unit
    dropins = sorted((unit_dir / (unit + ".d")).glob("*.conf"))
    commands = []
    for path in [fragment, *dropins]:
        if not path.exists(): continue
        section = ""
        for line in path.read_text().splitlines():
            line = line.strip()
            if line.startswith("["): section = line
            if section == "[Service]" and line.startswith("ExecStart="):
                value = line.split("=", 1)[1]
                if not value: commands.clear()
                else: commands.append(value.replace("%h", str(home)))
    if len(commands) != 1: raise SystemExit("fixture ExecStart is ambiguous")
    return fragment, dropins, commands[0]
if action == "show" and unit and any("ExecStart" in item or "DropInPaths" in item or "FragmentPath" in item for item in values):
    fragment, dropins, command = execution()
    path = shlex.split(command)[0]
    fields = {"ExecStart": "{ path=" + path + " ; argv[]=" + command + " ; ignore_errors=no ; }",
              "FragmentPath": str(fragment), "DropInPaths": " ".join(map(str, dropins))}
    selected = []
    for index, item in enumerate(values):
        if item.startswith("--property=") or item.startswith("-p="):
            selected.extend(item.split("=", 1)[1].split(","))
        elif item in {"--property", "-p"} and index + 1 < len(values):
            selected.extend(values[index + 1].split(","))
    for name in selected:
        if name in fields: print(fields[name] if "--value" in values else name + "=" + fields[name])
    raise SystemExit(0)
if action in {"start", "restart"} and unit in {"codex-info-recorder.service", "codex-info-rest.service"}:
    _, _, command = execution()
    target = pathlib.Path(shlex.split(command)[0]).resolve(strict=True)
    pid = os.environ["FAKE_MAIN_PID"] if unit == "codex-info-recorder.service" else os.environ["FAKE_REST_PID"]
    executable = proc / pid / "exe"
    if executable.is_symlink(): executable.unlink()
    executable.symlink_to(target)
    if unit == "codex-info-rest.service":
        manifest = json.loads((target.parent / "manifest.json").read_text())
        os.environ["FAKE_HEALTH_VERSION"] = manifest["version"]
    with pathlib.Path(os.environ["FAKE_EFFECTIVE_LOG"]).open("a") as stream:
        stream.write(json.dumps({"unit": unit, "command": command, "path": str(target)}) + "\n")
raise SystemExit(subprocess.run(["/bin/bash", os.environ["FAKE_SYSTEMCTL_BASE"], *args], check=False).returncode)
'''

CURL_ADAPTER = r'''#!/usr/bin/python3
import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
if "http://127.0.0.1:8787/v1/health" in args:
    target = (pathlib.Path(os.environ["HOME"]) / ".local/share/codex-info/current").resolve(strict=True)
    os.environ["FAKE_HEALTH_VERSION"] = json.loads((target / "manifest.json").read_text())["version"]
raise SystemExit(subprocess.run(["/bin/bash", os.environ["FAKE_CURL_BASE"], *args], check=False).returncode)
'''


class EffectiveRecorderUpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codex-info-effective-update-")
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.home = self.root / "home"
        self.proc = self.root / "proc"
        self.bin = self.root / "bin"
        self.payload = self.root / "payload"
        self.assets = self.root / "assets"
        for path in (self.home, self.proc / "net", self.bin, self.payload, self.assets, self.root / "tmp"):
            path.mkdir(parents=True)
        (self.proc / "net/tcp").write_text("")
        self.env = {
            "HOME": str(self.home), "CODEX_HOME": str(self.home / ".codex"),
            "PATH": str(self.bin) + ":/usr/bin:/bin", "LC_ALL": "C", "PYTHONDONTWRITEBYTECODE": "1",
            "TMPDIR": str(self.root / "tmp"), "CODEX_INFO_PROC_ROOT": str(self.proc),
            "SYSTEMCTL_BIN": "systemctl", "CURL_BIN": "curl", "GETCONF_BIN": "getconf", "LDD_BIN": "ldd",
            "FAKE_PROC_ROOT": str(self.proc), "FAKE_LOG": str(self.root / "commands.log"),
            "FAKE_MAIN_PID": str(RECORDER_PID), "FAKE_REST_PID": str(REST_PID),
            "FAKE_MAIN_ACTIVE_FILE": str(self.root / "recorder-active"),
            "FAKE_REST_ACTIVE_FILE": str(self.root / "rest-active"),
            "FAKE_TIMER_ACTIVE_FILE": str(self.root / "timer-active"),
            "FAKE_RECORDER_REFRESH": str(self.home / ".codex/history"),
            "FAKE_HEALTH_VERSION": "1.0.19", "FAKE_DETAILS_STATE": "auth_required",
            "FAKE_RELEASE_JSON": str(self.root / "releases.json"), "FAKE_RELEASE_ASSETS": str(self.assets),
            "FAKE_SYSTEMCTL_BASE": str(self.bin / "systemctl-base"),
            "FAKE_CURL_BASE": str(self.bin / "curl-base"),
            "FAKE_EFFECTIVE_LOG": str(self.root / "effective.log"),
        }
        for name, text in {
            "systemctl-base": heredoc("FAKE_SYSTEMCTL"), "curl-base": heredoc("FAKE_CURL"),
            "systemctl": SYSTEMCTL_ADAPTER, "curl": CURL_ADAPTER,
            "objdump": "#!/bin/sh\nprintf 'fake GLIBC_2.31\\n'\n",
            "getconf": "#!/bin/sh\nprintf 'glibc 2.31\\n'\n",
            "ldd": "#!/bin/sh\nprintf 'ldd (GNU libc) 2.31\\n'\n",
        }.items():
            path = self.bin / name
            path.write_text(text, encoding="utf-8")
            path.chmod(0o755)
        history = self.home / ".codex/history"
        history.mkdir(parents=True)
        self.sentinel = history / "history-preserved.fixture"
        self.sentinel.write_bytes(b"independent history preservation oracle\n")
        self.auth = self.home / ".codex/auth.fixture"
        self.auth.write_bytes(b"independent credential preservation oracle\n")
        self.boot_id = pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip()
        self.control("stopped")
        archive = self.bundle("1.0.19", "1" * 40)
        result = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(result.returncode, 0, "fixture baseline install failed:\n" + result.stderr)
        self.old_generation = self.current().resolve(strict=True)
        self.installed = self.home / ".local/libexec/codex-info-install.sh"
        self.unit_dir = self.home / ".config/systemd/user"
        self.override = self.unit_dir / "codex-info-recorder.service.d/90-issue-134-current-cli.conf"

    def command(self, script, *args):
        return subprocess.run(  # nosec B603 B607 # fixed offline installer/build commands and isolated environment.
            ["/bin/bash", str(script), *map(str, args)], env=self.env,
            capture_output=True, text=True, check=False, timeout=90,
        )

    def current(self):
        return self.home / ".local/share/codex-info/current"

    def control(self, state):
        path = self.home / ".local/share/codex-info/control-state.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        value = {"schema": "codex-info-control-state-v1", "desired_state": state,
                 "boot_id": self.boot_id, "operation_id": "isolated-fixture", "generation_id": "",
                 "updated_at_unix": int(time.time())}
        path.write_text(json.dumps(value) + "\n")
        path.chmod(0o600)

    def bundle(self, version, source, recorder_bytes=None):
        for name in ("codex_info", "codex_info_recorder", "codex_info_rest"):
            path = self.payload / name
            if name == "codex_info_recorder" and recorder_bytes is not None:
                path.write_bytes(recorder_bytes)
            else:
                path.write_text(f"trusted fixture product {name} {version}\n")
            path.chmod(0o755)
        result = self.command(
            BUILDER, "--ui-binary", self.payload / "codex_info", "--recorder-binary", self.payload / "codex_info_recorder",
            "--rest-binary", self.payload / "codex_info_rest", "--version", version, "--source-sha", source,
            "--run-id", "92001", "--run-attempt", "1", "--output-dir", self.root / "output",
        )
        self.assertEqual(result.returncode, 0, "fixture bundle build failed:\n" + result.stderr)
        return self.root / f"output/codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz"

    def release(self, version, source, recorder_bytes=None, include_sidecars=True):
        archive = self.bundle(version, source, recorder_bytes=recorder_bytes)
        paths = [archive]
        if include_sidecars:
            paths.extend((pathlib.Path(str(archive) + ".sha256"), archive.with_suffix("").with_suffix(".manifest.json")))
        for path in paths:
            shutil.copyfile(path, self.assets / path.name)
        (self.assets / "unrelated-release-asset.txt").write_text("ignored by Linux updater\n")
        assets = [{"name": path.name, "browser_download_url": f"https://github.com/salty919/codex_info_v2/releases/download/windows-v{version}/{path.name}",
                   "state": "uploaded", "size": path.stat().st_size, "digest": "sha256:" + digest(path)} for path in self.assets.iterdir()]
        value = {"tag_name": "windows-v" + version, "draft": False, "prerelease": False,
                 "published_at": "2026-09-01T00:00:00Z", "assets": assets}
        pathlib.Path(self.env["FAKE_RELEASE_JSON"]).write_text(json.dumps(value))

    def active_override(self, owned):
        if owned:
            self.hotfix = self.home / ".local/share/codex-info/hotfixes/issue-134/codex_info_recorder"
            self.hotfix.parent.mkdir(parents=True)
            # Product provenance is independent of the migration implementation:
            # these exact bytes already belong to the verified predecessor bundle.
            shutil.copyfile(self.old_generation / "codex_info_recorder", self.hotfix)
            self.assertEqual(digest(self.hotfix), digest(self.old_generation / "codex_info_recorder"))
        else:
            self.hotfix = self.home / ".local/share/custom-recorder/codex_info_recorder"
            self.hotfix.parent.mkdir(parents=True)
            self.hotfix.write_bytes(b"user-owned custom recorder; never migrate automatically\n")
        self.hotfix.chmod(0o755)
        self.override.parent.mkdir(parents=True)
        self.override.write_text("[Service]\nExecStart=\nExecStart=" + str(self.hotfix) + "\n")
        self.override.chmod(0o644)
        self.override_bytes = self.override.read_bytes()
        self.hotfix_bytes = self.hotfix.read_bytes()
        for pid, target in ((RECORDER_PID, self.hotfix), (REST_PID, self.old_generation / "codex_info_rest")):
            directory = self.proc / str(pid)
            (directory / "fd").mkdir(parents=True)
            fields = ["S", *(["0"] * 18), "101"]
            (directory / "stat").write_text(f"{pid} (offline-fixture) " + " ".join(fields) + "\n")
            (directory / "exe").symlink_to(target)
        (self.proc / str(REST_PID) / "fd/3").symlink_to("socket:[599]")
        socket = (
            "  sl  local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n"
            "  0: 0100007F:2253 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 599 1\n"
        )
        (self.proc / "net/tcp").write_text(socket)
        self.env["FAKE_REST_SOCKET_NET_FILE"] = str(self.proc / "net/tcp")
        self.env["FAKE_REST_SOCKET_CONTENT"] = socket
        pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]).touch()
        pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"]).touch()
        self.control("running")
        result = subprocess.run(  # nosec B603 # this offline adapter updates fixture lock identity only.
            ["/usr/bin/python3", str(self.bin / "systemctl"), "--user", "restart", "--no-block", "codex-info-recorder.service"],
            env=self.env, capture_output=True, text=True, check=False, timeout=5,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def preserved_profile(self):
        self.assertEqual(self.sentinel.read_bytes(), b"independent history preservation oracle\n")
        self.assertEqual(self.auth.read_bytes(), b"independent credential preservation oracle\n")

    def test_regular_update_migrates_product_override_and_tracks_next_generation(self):
        self.active_override(owned=True)
        (self.bin / "getconf").write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
        (self.bin / "ldd").write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
        self.release("1.0.20", "2" * 40, include_sidecars=False)
        result = self.command(self.installed, "--update")
        generation = self.current().resolve(strict=True)
        running = (self.proc / str(RECORDER_PID) / "exe").resolve(strict=True)
        # Assert the causal defect before looking at a command's success label.
        self.assertEqual(running, generation / "codex_info_recorder", result.stdout + result.stderr)
        self.assertEqual(digest(running), digest(generation / "codex_info_recorder"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.override.exists(), "the permanent version-pinning override must be retired")
        backups = self.home / ".local/share/codex-info/legacy-backups"
        self.assertTrue(any(path.is_file() and path.read_bytes() == self.override_bytes for path in backups.rglob("*")),
                        "migration must preserve the exact old drop-in bytes")
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.preserved_profile()
        # A second normal update proves migration did not pin another version.
        for path in self.assets.iterdir():
            path.unlink()
        self.release("1.0.21", "3" * 40, include_sidecars=False)
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True),
                         self.current().resolve(strict=True) / "codex_info_recorder")
        self.assertFalse(list(self.override.parent.glob("*.conf")), "no new fixed-version ExecStart override")
        self.preserved_profile()

    def test_regular_update_preserves_unknown_override_and_reports_conflict(self):
        self.active_override(owned=False)
        self.release("1.0.20", "2" * 40)
        result = self.command(self.installed, "--update")
        # A recognized filename alone is deliberately insufficient ownership.
        self.assertEqual(self.current().resolve(strict=True), self.old_generation, result.stdout + result.stderr)
        self.assertNotEqual(result.returncode, 0, "an unresolved user override cannot be reported as updated")
        self.assertIn("SAFE_BLOCKED", result.stderr)
        self.assertRegex(result.stderr.lower(), r"override|execstart")
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), self.hotfix)
        self.preserved_profile()

    def test_failed_activation_restores_override_binary_and_previous_generation(self):
        self.active_override(owned=True)
        self.release("1.0.20", "2" * 40)
        # A finite normal activation failure; no hostile archive or filesystem race.
        self.env["FAKE_FAIL_START_UNIT"] = "codex-info-rest.service"
        self.env["FAKE_FAIL_START_ONCE_FILE"] = str(self.root / "rest-failed-once")
        result = self.command(self.installed, "--update")
        self.assertNotEqual(result.returncode, 0, "failed activation must remain a failed update")
        self.assertEqual(self.current().resolve(strict=True), self.old_generation, result.stdout + result.stderr)
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), self.hotfix)
        self.preserved_profile()

    def test_no_update_migrates_verified_override_without_changing_generation(self):
        self.active_override(owned=True)
        self.release("1.0.19", "1" * 40)
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.current().resolve(strict=True), self.old_generation)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True),
                         self.old_generation / "codex_info_recorder")
        self.assertFalse(self.override.exists())
        backups = self.home / ".local/share/codex-info/legacy-backups"
        self.assertTrue(any(path.is_file() and path.read_bytes() == self.override_bytes for path in backups.rglob("*")))
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.preserved_profile()

    def test_verify_runtime_rejects_noncurrent_binary_with_canonical_execstart(self):
        self.active_override(owned=False)
        # The configured command is now canonical; the admitted PID/lock still
        # describes a non-product executable. Version and heartbeat are valid.
        self.override.unlink()
        result = self.command(self.installed, "--verify-runtime")
        self.assertNotEqual(result.returncode, 0, "fresh lock/health cannot authenticate a different executable")
        self.assertIn("digest", result.stderr.lower())
        self.assertEqual(self.current().resolve(strict=True), self.old_generation)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), self.hotfix)
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.preserved_profile()

    def test_update_preserves_unverified_payload_at_historical_product_path(self):
        self.active_override(owned=True)
        unverified_bytes = b"historical payload without verified product provenance\n"
        self.hotfix.write_bytes(unverified_bytes)
        self.release("1.0.20", "2" * 40)
        result = self.command(self.installed, "--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("provenance", result.stderr.lower())
        self.assertEqual(self.current().resolve(strict=True), self.old_generation)
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.hotfix.read_bytes(), unverified_bytes)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), self.hotfix)
        self.preserved_profile()

    def test_same_recorder_hash_update_and_rest_failure_preserve_running_recorder(self):
        self.active_override(owned=True)
        self.override.unlink()
        recorder_bytes = (self.old_generation / "codex_info_recorder").read_bytes()
        executable = self.proc / str(RECORDER_PID) / "exe"
        original_running = executable.resolve(strict=True)
        events = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"])
        before = len(events.read_text().splitlines())
        self.release("1.0.20", "2" * 40, recorder_bytes=recorder_bytes)
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(executable.resolve(strict=True), original_running)
        self.assertEqual(digest(executable), digest(self.current() / "codex_info_recorder"))
        activations = [json.loads(line)["unit"] for line in events.read_text().splitlines()[before:]]
        self.assertNotIn("codex-info-recorder.service", activations)
        self.assertIn("codex-info-rest.service", activations)
        previous = self.current().resolve(strict=True)
        for path in self.assets.iterdir():
            path.unlink()
        self.release("1.0.21", "3" * 40, recorder_bytes=recorder_bytes)
        self.env["FAKE_FAIL_START_UNIT"] = "codex-info-rest.service"
        self.env["FAKE_FAIL_START_ONCE_FILE"] = str(self.root / "reuse-rest-failed-once")
        before = len(events.read_text().splitlines())
        result = self.command(self.installed, "--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.current().resolve(strict=True), previous, result.stdout + result.stderr)
        self.assertEqual(executable.resolve(strict=True), original_running)
        self.assertEqual(digest(executable), digest(previous / "codex_info_recorder"))
        activations = [json.loads(line)["unit"] for line in events.read_text().splitlines()[before:]]
        self.assertNotIn("codex-info-recorder.service", activations)
        self.assertFalse(self.override.exists())
        self.preserved_profile()
        records = [json.loads(line) for line in (self.home / ".local/share/codex-info/update.log").read_text().splitlines()]
        failed = [record for record in records if record["target"] == "1.0.21"]
        self.assertTrue(any(record["stage"] == "install" for record in failed))
        self.assertTrue(any(record["stage"] == "rollback" for record in failed))
        self.assertTrue(any(record["result"] == "failed" and "candidate activation failed" in record["reason"] for record in failed))
        launcher = self.command(ROOT / "run.sh", "--ui")
        self.assertIn("candidate activation failed", launcher.stderr)


    def unknown_historical_override(self):
        self.active_override(owned=True)
        self.hotfix_bytes = b"unregistered historical recorder; explicit product migration required\n"
        self.hotfix.write_bytes(self.hotfix_bytes)
        self.assertNotEqual(digest(self.hotfix), digest(self.old_generation / "codex_info_recorder"))

    def test_unknown_override_requires_choice_through_normal_launcher(self):
        self.unknown_historical_override()
        self.release("1.0.20", "2" * 40)
        journal = self.home / ".local/share/codex-info/install-transaction.json"
        before_journal = journal.read_bytes()
        before_commands = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_bytes()
        result = self.command(self.home / ".local/bin/codex-info", "--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("RECORDER_OVERRIDE_MIGRATION_REQUIRED", result.stderr)
        self.assertIn("codex-info --update --migrate-recorder-override", result.stderr)
        for destination in (self.override, self.hotfix, self.home / ".local/bin/codex_info_recorder",
                            self.home / ".local/share/codex-info/legacy-backups"):
            self.assertIn(str(destination), result.stderr)
        self.assertEqual(self.current().resolve(strict=True), self.old_generation)
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.assertEqual(journal.read_bytes(), before_journal)
        self.assertEqual(pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_bytes(), before_commands)
        self.preserved_profile()

    def test_explicit_migration_through_normal_launcher_preserves_originals(self):
        # Both release-selection branches must retain the user's explicit choice.
        for index, version in enumerate(("1.0.19", "1.0.20")):
            with self.subTest(version=version):
                if index:
                    self.doCleanups()
                    self.setUp()
                self.unknown_historical_override()
                self.release(version, ("1" if version == "1.0.19" else "2") * 40)
                result = self.command(self.home / ".local/bin/codex-info", "--update", "--migrate-recorder-override")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("RECORDER_OVERRIDE_MIGRATION", result.stderr)
                generation = self.current().resolve(strict=True)
                running = (self.proc / str(RECORDER_PID) / "exe").resolve(strict=True)
                self.assertEqual(running, generation / "codex_info_recorder")
                self.assertEqual(digest(running), digest(generation / "codex_info_recorder"))
                self.assertFalse(self.override.exists())
                self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
                backups = self.home / ".local/share/codex-info/legacy-backups"
                originals = [file for file in backups.rglob("*") if file.is_file()
                             and file.read_bytes() == self.override_bytes]
                self.assertEqual(len(originals), 1)
                self.assertEqual(originals[0].stat().st_mode & 0o777, 0o644)
                self.assertFalse(list(self.override.parent.glob("*.conf")))
                self.preserved_profile()

    def test_failed_explicit_migration_restores_exact_unknown_prestate(self):
        self.unknown_historical_override()
        self.release("1.0.20", "2" * 40)
        self.env["FAKE_FAIL_START_UNIT"] = "codex-info-rest.service"
        self.env["FAKE_FAIL_START_ONCE_FILE"] = str(self.root / "explicit-migration-rest-failed-once")
        before_events = len(pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_text().splitlines())
        result = self.command(self.home / ".local/bin/codex-info", "--update", "--migrate-recorder-override")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("previous generation restored", result.stderr)
        events = [json.loads(line) for line in pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_text().splitlines()[before_events:]]
        self.assertTrue(any(event["unit"] == "codex-info-recorder.service" and event["path"] != str(self.hotfix)
                            for event in events), "activation must be attempted before the rollback oracle")
        self.assertEqual(self.current().resolve(strict=True), self.old_generation)
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.override.stat().st_mode & 0o777, 0o644)
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), self.hotfix)
        journal = json.loads((self.home / ".local/share/codex-info/install-transaction.json").read_text())
        self.assertEqual(journal["phase"], "committed", "rollback must finish its existing verification gate")
        self.preserved_profile()

    def test_explicit_migration_preserves_mixed_or_unsafe_dropin(self):
        for index, kind in enumerate(("mixed", "unsafe_mode")):
            with self.subTest(kind=kind):
                if index:
                    self.doCleanups()
                    self.setUp()
                self.unknown_historical_override()
                if kind == "mixed":
                    self.override.write_bytes(self.override_bytes + b"Environment=USER_SETTING=preserve\n")
                else:
                    self.override.chmod(0o666)
                expected_bytes = self.override.read_bytes()
                expected_mode = self.override.stat().st_mode & 0o777
                self.release("1.0.20", "2" * 40)
                before_commands = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_bytes()
                result = self.command(self.home / ".local/bin/codex-info", "--update", "--migrate-recorder-override")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("SAFE_BLOCKED", result.stderr)
                self.assertEqual(self.current().resolve(strict=True), self.old_generation)
                self.assertEqual(self.override.read_bytes(), expected_bytes)
                self.assertEqual(self.override.stat().st_mode & 0o777, expected_mode)
                self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
                self.assertEqual(pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_bytes(), before_commands)
                self.preserved_profile()


class RecorderRollbackIdentityTests(unittest.TestCase):
    def test_rollback_binds_saved_file_identity_and_keeps_canonical_reuse(self):
        function = re.findall(r"(?ms)^recorder_binary_identity_check\(\) \{\n.*?^\}\n", INSTALLER.read_text())
        self.assertEqual(len(function), 1)
        with tempfile.TemporaryDirectory(prefix="recorder-rollback-file-identity-") as directory:
            root = pathlib.Path(directory)
            original = root / "original-recorder"
            identical = root / "other-recorder"
            original.write_bytes(b"independent original recorder file identity oracle\n")
            identical.write_bytes(original.read_bytes())
            original.chmod(0o755)
            identical.chmod(0o755)
            metadata = original.stat()
            self.assertNotEqual(metadata.st_ino, identical.stat().st_ino)
            self.assertEqual(digest(original), digest(identical))
            receipt = root / "receipt.json"
            receipt.write_text(json.dumps({"executable":str(original), "executable_record":{
                "device":metadata.st_dev, "inode":metadata.st_ino, "sha256":digest(original)}}))
            receipt.chmod(0o600)
            process = root / "proc/70"
            process.mkdir(parents=True)
            executable = process / "exe"
            prefix = """set -euo pipefail
allow_legacy_recorder_override=1
proc_root="$FIXTURE_ROOT/proc"
generations_dir="$FIXTURE_ROOT/generations"
rollback_recorder_receipt="$FIXTURE_ROOT/receipt.json"
safe_blocked() { printf '%s\\n' "$*" >&2; return 1; }
proc_starttime() { printf '42\\n'; }
recorder_systemd_pid() { printf '70\\n'; }
# Classification and generation manifest are already verified caller inputs.
recorder_execution_record() { printf '%s\\t%s\\n' "$EXECUTION_KIND" "$FIXTURE_SHA"; }
manifest_record() { printf '1.0.19\\tsource\\tmanifest\\t%s\\n' "$FIXTURE_SHA"; }
"""
            for kind, target, expected in (("restored", original, 0), ("restored", identical, 1),
                                          ("canonical", identical, 0)):
                with self.subTest(kind=kind, executable=target.name):
                    if executable.is_symlink():
                        executable.unlink()
                    executable.symlink_to(target)
                    result = subprocess.run(
                        ["/bin/bash", "--noprofile", "--norc", "-s"],
                        input=prefix + function[0] + "\nrecorder_binary_identity_check 70 verified-old-generation\n",
                        env={"PATH":"/usr/bin:/bin", "LC_ALL":"C", "FIXTURE_ROOT":str(root),
                             "FIXTURE_SHA":digest(original), "EXECUTION_KIND":kind},
                        capture_output=True, text=True, check=False, timeout=3,
                    )
                    self.assertEqual(result.returncode, expected, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()

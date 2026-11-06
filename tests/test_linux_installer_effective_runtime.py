"""Offline update-entrypoint regressions for recorder overrides.

The oracle is the requested process path/hash, never UI or health version.
Only an isolated HOME, fake proc tree and trusted fixture bundles are used.
"""

import hashlib
import io
import json
import os
import pathlib
import re
import shutil
import sqlite3
import subprocess
import sys
import tarfile
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
    argv = shlex.split(command)
    pid = os.environ["FAKE_MAIN_PID"] if unit == "codex-info-recorder.service" else os.environ["FAKE_REST_PID"]
    executable = proc / pid / "exe"
    executable.parent.mkdir(parents=True, exist_ok=True)
    if executable.is_symlink(): executable.unlink()
    executable.symlink_to(target)
    (proc / pid / "cmdline").write_bytes(b"\0".join(item.encode() for item in argv) + b"\0")
    manifest_path = target.parent / "manifest.json"
    version = json.loads(manifest_path.read_text())["version"] if manifest_path.is_file() else ""
    if unit == "codex-info-rest.service":
        if manifest_path.is_file():
            os.environ["FAKE_HEALTH_VERSION"] = version
    with pathlib.Path(os.environ["FAKE_EFFECTIVE_LOG"]).open("a") as stream:
        stream.write(json.dumps({"unit": unit, "command": command, "path": str(target)}) + "\n")
result = subprocess.run(["/bin/bash", os.environ["FAKE_SYSTEMCTL_BASE"], *args], check=False)
if (result.returncode == 0 and action in {"start", "restart"} and unit == "codex-info-recorder.service" and
        os.environ.get("FAKE_RECORDER_FAIL_READY_VERSION") == version):
    history = pathlib.Path(os.environ["FAKE_RECORDER_REFRESH"])
    (history / "recorder-state.json").write_text("{}\n")
    marker = pathlib.Path(os.environ["FAKE_RECORDER_READINESS_MARKER"])
    marker.write_text(json.dumps({"version": version, "injection": "recorder-readiness"}) + "\n")
raise SystemExit(result.returncode)
'''

CURL_ADAPTER = r'''#!/usr/bin/python3
import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
url = args[-1] if args else ""
if url.endswith("/v4/current") and "FAKE_CURRENT_STATUS" in os.environ:
    status = os.environ["FAKE_CURRENT_STATUS"]
    body = os.environ.get("FAKE_CURRENT_BODY", "{}")
    with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as stream:
        stream.write("current-v4 " + status + "\n")
    if "--output" in args:
        pathlib.Path(args[args.index("--output") + 1]).write_text(body)
    else:
        sys.stdout.write(body)
    if "--write-out" in args:
        sys.stdout.write(args[args.index("--write-out") + 1].replace("%{http_code}", status))
    raise SystemExit(22 if "--fail" in args and int(status) >= 400 else 0)
if url.endswith("/v1/details") and os.environ.get("FAKE_REJECT_LEGACY_DETAILS") == "1":
    with pathlib.Path(os.environ["FAKE_LOG"]).open("a") as stream:
        stream.write("rejected-legacy-details\n")
    raise SystemExit(22)
if "http://127.0.0.1:8787/v1/health" in args:
    target = (pathlib.Path(os.environ["HOME"]) / ".local/share/codex-info/current").resolve(strict=True)
    version = json.loads((target / "manifest.json").read_text())["version"]
    if os.environ.get("FAKE_HEALTH_FAIL_VERSION") == version:
        raise SystemExit(22)
    os.environ["FAKE_HEALTH_VERSION"] = version
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
        return subprocess.run(  # noqa: S603 -- fixed offline installer/build commands and isolated environment.  # nosec B603 B607
            ["/bin/bash", str(script), *map(str, args)], env=self.env,
            capture_output=True, text=True, check=False, timeout=90,
        )

    def current_readiness(self):
        # Exercise the real route/schema consumer without an update or its
        # rollback loop. Process identity has its own existing fixture owner.
        source = INSTALLER.read_text()
        function = source.split("health_readback() {\n", 1)[1].split("\n}\n", 1)[0]
        urls = "\n".join(re.findall(r"^(?:HEALTH|CURRENT|DETAILS)_URL=.*$", source, re.M))
        shell = """set -euo pipefail
generations_dir=/unused
HEALTH_TIMEOUT=30
readiness_deadline=0
CURL_BIN=curl
proc_identity_check() { :; }
recorder_identity_check() { :; }
now_unix() { printf 1000; }
proc_starttime() { printf 101; }
systemd_pid() { printf 7; }
recorder_systemd_pid() { printf 8; }
safe_blocked() { printf '%s\\n' "$*" >&2; exit 3; }
""" + urls + "\nhealth_readback() {\n" + function + "\n}\nhealth_readback 7 101 1.0.19 source manifest hash\n"
        return subprocess.run(  # noqa: S603 -- repository function and isolated fake HTTP environment.  # nosec B603 B607
            ["/bin/bash", "--noprofile", "--norc", "-c", shell], env=self.env,
            capture_output=True, text=True, check=False, timeout=5,
        )

    def current(self):
        return self.home / ".local/share/codex-info/current"

    def control(self, state):
        path = self.home / ".local/share/codex-info/control-state.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.parent.chmod(0o700)
        value = {"schema": "codex-info-control-state-v1", "desired_state": state,
                 "boot_id": self.boot_id, "operation_id": "isolated-fixture", "generation_id": "",
                 "updated_at_unix": int(time.time())}
        path.write_text(json.dumps(value) + "\n")
        path.chmod(0o600)

    def bundle(self, version, source, recorder_bytes=None, ui_bytes=None):
        for name in ("codex_info", "codex_info_recorder", "codex_info_rest"):
            path = self.payload / name
            if name == "codex_info_recorder" and recorder_bytes is not None:
                path.write_bytes(recorder_bytes)
            elif name == "codex_info" and ui_bytes is not None:
                path.write_bytes(ui_bytes)
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

    def local_recovery_bundle(self, version="1.0.19", ui_bytes=None):
        archive = self.bundle(version, "2" * 40, recorder_bytes=b"local repaired recorder\n",
                              ui_bytes=ui_bytes or b"#!/usr/bin/python3\nimport os, pathlib\npathlib.Path(os.environ['HOME'], 'ui-started').write_text('verified local UI client\\n')\nif path := os.environ.get('CODEX_INFO_UI_STARTUP_FIFO'):\n    with open(path, 'w') as ready: ready.write('READY\\n')\n")
        with tarfile.open(archive, "r:gz") as source:
            members = {item.name: (source.extractfile(item).read(), item.mode)
                       for item in source.getmembers()}
        document = json.loads(members["manifest.json"][0])
        provenance = json.dumps({"kind": "local-recovery", "base_source_sha": "2" * 40,
                                 "source_diff_sha256": "a" * 64}, sort_keys=True).encode()
        proof_hash = hashlib.sha256(provenance).hexdigest()
        document["run_id"] = "local-" + proof_hash
        document["files"].append({"path": "LOCAL_RECOVERY.json", "size": len(provenance),
                                  "sha256": proof_hash, "mode": 0o644})
        document["files"].sort(key=lambda entry: entry["path"])
        raw = json.dumps(document, indent=2).encode() + b"\n"
        members["LOCAL_RECOVERY.json"] = (provenance, 0o644)
        members["manifest.json"] = (raw, 0o644)
        members["SHA256SUMS"] = ("".join(
            hashlib.sha256(data).hexdigest() + "  " + name + "\n"
            for name, (data, mode) in sorted(members.items()) if name != "SHA256SUMS").encode(), 0o644)
        with tarfile.open(archive, "w:gz") as output:
            for name, (data, mode) in sorted(members.items()):
                entry = tarfile.TarInfo(name)
                entry.size, entry.mode = len(data), mode
                output.addfile(entry, io.BytesIO(data))
        archive.with_suffix("").with_suffix(".manifest.json").write_bytes(raw)
        pathlib.Path(str(archive) + ".sha256").write_text(digest(archive) + "  " + archive.name + "\n")
        return archive


    def canonical_process_fixture(self, rest_active=False):
        for pid in (RECORDER_PID, REST_PID):
            directory = self.proc / str(pid)
            (directory / "fd").mkdir(parents=True, exist_ok=True)
            fields = ["S", *("0" for _ in range(18)), "101"]
            (directory / "stat").write_text(f"{pid} (canonical-fixture) " + " ".join(fields) + "\n")
        (self.proc / str(REST_PID) / "exe").symlink_to(self.old_generation / "codex_info_rest")
        (self.proc / str(REST_PID) / "fd/3").symlink_to("socket:[599]")
        socket = (
            "  sl  local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n"
            "  0: 0100007F:2253 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 599 1\n"
        )
        self.env["FAKE_REST_SOCKET_NET_FILE"] = str(self.proc / "net/tcp")
        self.env["FAKE_REST_SOCKET_CONTENT"] = socket
        if rest_active:
            pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"]).touch()
            (self.proc / "net/tcp").write_text(socket)


    def ui_startup_fixture(self, *, fail=False):
        payload = b'''#!/usr/bin/python3
import os, pathlib, sys
root = pathlib.Path(os.environ["HOME"])
(root / "ui-pid").write_text(str(os.getpid()))
if os.environ.get("FAKE_UI_INIT_FAILURE") == "1": sys.exit(23)
if path := os.environ.get("CODEX_INFO_UI_STARTUP_FIFO"):
    with open(path, "w") as ready: ready.write("READY\\n")
(root / "ui-displayed").touch()
with open(root / "ui-exit-permission", "rb") as permission: permission.read(1)
(root / "ui-exited").touch()
'''
        installed = self.command(INSTALLER, "--bundle", self.local_recovery_bundle(ui_bytes=payload))
        self.assertEqual(installed.returncode, 0, installed.stderr)
        self.canonical_process_fixture()
        started = self.command(self.installed, "--start")
        self.assertEqual(started.returncode, 0, started.stderr)
        os.mkfifo(self.home / "ui-exit-permission", 0o600)
        if fail:
            self.env["FAKE_UI_INIT_FAILURE"] = "1"

    def test_issue669_installed_ui_returns_after_ready_and_keeps_ui_alive(self):
        self.ui_startup_fixture()
        launcher = subprocess.Popen(  # installed fixture launcher in isolated HOME.  # nosec B603
            ["/bin/bash", ".local/bin/codex-info", "--ui"],
            cwd=self.home,
            env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            deadline = time.monotonic() + 30
            while not (self.home / "ui-displayed").exists():
                self.assertLess(time.monotonic(), deadline, "fixture UI did not reach its display oracle")
                time.sleep(0.02)
            try:
                stdout, stderr = launcher.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                self.fail("launcher still waits for UI exit after initial display")
            self.assertEqual(launcher.returncode, 0, stdout + stderr)
            pid = int((self.home / "ui-pid").read_text())
            os.kill(pid, 0)
            self.assertFalse((self.home / "ui-exited").exists())
            self.preserved_profile()
        finally:
            try:
                permission = os.open(self.home / "ui-exit-permission", os.O_WRONLY | os.O_NONBLOCK)
                os.write(permission, b"x")
                os.close(permission)
                deadline = time.monotonic() + 5
                while not (self.home / "ui-exited").exists():
                    self.assertLess(time.monotonic(), deadline, "fixture UI did not acknowledge exit")
                    time.sleep(0.02)
            except OSError:
                launcher.terminate()
            launcher.communicate(timeout=5)

    def test_issue669_installed_ui_initialization_failure_returns_nonzero(self):
        self.ui_startup_fixture(fail=True)
        result = self.command(self.home / ".local/bin/codex-info", "--ui")
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.home / "ui-displayed").exists())
        self.preserved_profile()

    def test_absolute_current_recovery_start_and_ui_keep_verified_identity(self):
        archive = self.local_recovery_bundle()
        installed = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(installed.returncode, 0, installed.stderr)
        self.canonical_process_fixture()
        self.env["FAKE_RELEASE_FAILURE"] = "1"
        normal = self.command(self.installed, "--start")
        self.assertEqual(normal.returncode, 0, normal.stderr)
        recovered = self.current().resolve(strict=True)
        effective = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"])
        owners_before = effective.read_bytes()
        self.current().unlink()
        self.current().symlink_to(recovered)

        for operation in ("--start", "--verify-runtime", "--verify-ui", "--status"):
            with self.subTest(operation=operation):
                result = self.command(self.installed, operation)
                self.assertEqual(result.returncode, 0, result.stderr)
        ui = self.command(self.home / ".local/bin/codex-info", "--ui")
        self.assertEqual(ui.returncode, 0, ui.stderr)
        self.assertEqual((self.home / "ui-started").read_text(), "verified local UI client\n")
        self.assertEqual(self.current().resolve(strict=True), recovered)
        self.assertEqual(effective.read_bytes(), owners_before)
        self.assertNotIn("/releases/", pathlib.Path(self.env["FAKE_LOG"]).read_text())
        self.preserved_profile()

    def test_absolute_current_no_update_keeps_verified_generation(self):
        original = self.current().resolve(strict=True)
        self.current().unlink()
        self.current().symlink_to(original)
        self.release("1.0.19", "1" * 40)
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.current().resolve(strict=True), original)
        self.assertTrue(original.is_dir())
        self.preserved_profile()

    def test_absolute_current_can_transition_to_published_generation(self):
        original = self.current().resolve(strict=True)
        self.current().unlink()
        self.current().symlink_to(original)
        self.release("1.0.20", "3" * 40)
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        final = self.current().resolve(strict=True)
        self.assertNotEqual(final, original)
        self.assertEqual(json.loads((final / "manifest.json").read_bytes())["version"], "1.0.20")
        self.assertTrue(original.is_dir(), "verified rollback generation was removed")
        self.preserved_profile()

    def test_current_outside_managed_generations_is_not_an_owned_generation(self):
        installed = self.command(INSTALLER, "--bundle", self.local_recovery_bundle())
        self.assertEqual(installed.returncode, 0, installed.stderr)
        recovered = self.current().resolve(strict=True)
        outside = self.root / "outside" / recovered.name
        shutil.copytree(recovered, outside)
        self.current().unlink()
        self.current().symlink_to(outside)
        self.canonical_process_fixture()
        result = self.command(self.installed, "--start")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("SAFE_BLOCKED", result.stderr)
        self.assertFalse((self.home / "ui-started").exists())
        self.preserved_profile()



    def license_recovery_bundle(self):
        archive = self.local_recovery_bundle()
        with tarfile.open(archive, "r:gz") as source:
            members = {
                item.name: (source.extractfile(item).read(), item.mode)
                for item in source.getmembers()
            }
        name = "LICENSES/public-fixture.txt"
        data = b"independent public license fixture\n"
        members[name] = (data, 0o644)
        document = json.loads(members["manifest.json"][0])
        document["files"].append({
            "path": name, "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(), "mode": 0o644,
        })
        document["files"].sort(key=lambda entry: entry["path"])
        raw = json.dumps(document, sort_keys=True).encode() + b"\n"
        members["manifest.json"] = (raw, 0o644)
        members["SHA256SUMS"] = ("".join(
            hashlib.sha256(data).hexdigest() + "  " + name + "\n"
            for name, (data, _) in sorted(members.items())
            if name != "SHA256SUMS"
        ).encode(), 0o644)
        with tarfile.open(archive, "w:gz") as output:
            for name, (data, mode) in sorted(members.items()):
                item = tarfile.TarInfo(name)
                item.size, item.mode, item.mtime = len(data), mode, 0
                output.addfile(item, io.BytesIO(data))
        archive.with_suffix("").with_suffix(".manifest.json").write_bytes(raw)
        pathlib.Path(str(archive) + ".sha256").write_text(
            digest(archive) + "  " + archive.name + "\n"
        )
        return archive

    def test_public_license_directory_is_valid_for_verify_and_prune(self):
        installed = self.command(INSTALLER, "--bundle", self.license_recovery_bundle())
        self.assertEqual(installed.returncode, 0, installed.stderr)
        baseline = self.command(self.installed, "--verify-ui", "--quiet")
        self.assertEqual(baseline.returncode, 0, baseline.stderr)
        generation = self.current().resolve(strict=True)
        (generation / "LICENSES").chmod(0o755)

        verified = self.command(self.installed, "--verify-ui", "--quiet")
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.release("1.0.19", "2" * 40)
        updated = self.command(self.installed, "--update")
        self.assertEqual(updated.returncode, 0, updated.stderr)
        self.assertEqual(self.current().resolve(strict=True), generation)
        self.assertEqual(
            (generation / "LICENSES/public-fixture.txt").read_bytes(),
            b"independent public license fixture\n",
        )
        self.preserved_profile()

    def test_foreign_writable_license_directory_is_rejected(self):
        installed = self.command(INSTALLER, "--bundle", self.license_recovery_bundle())
        self.assertEqual(installed.returncode, 0, installed.stderr)
        generation = self.current().resolve(strict=True)
        (generation / "LICENSES").chmod(0o775)
        verified = self.command(self.installed, "--verify-ui", "--quiet")
        self.assertNotEqual(verified.returncode, 0, verified.stdout)
        self.assertIn("generation subdirectory", verified.stderr)
        self.assertEqual(self.current().resolve(strict=True), generation)
        self.preserved_profile()

    def test_issue604_local_recovery_start_ui_preserves_disabled_update_timer(self):
        installed = self.command(INSTALLER, "--bundle", self.local_recovery_bundle())
        self.assertEqual(installed.returncode, 0, installed.stderr)
        self.canonical_process_fixture()
        started = self.command(self.installed, "--start")
        self.assertEqual(started.returncode, 0, started.stderr)
        recovered = self.current().resolve(strict=True)
        effective = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"])
        owners_before = effective.read_bytes()

        # systemctl disable removes both the external unit registration and
        # its wants link; the manifested timer inside current is still valid.
        timer = self.unit_dir / "codex-info-update.timer"
        enabled = self.unit_dir / "timers.target.wants/codex-info-update.timer"
        timer.unlink()
        enabled.unlink()
        pathlib.Path(self.env["FAKE_TIMER_ACTIVE_FILE"]).unlink(missing_ok=True)
        commands = pathlib.Path(self.env["FAKE_LOG"])
        before = commands.read_text()
        self.env["FAKE_RELEASE_FAILURE"] = "1"

        restarted = self.command(self.installed, "--start")
        self.assertEqual(restarted.returncode, 0, restarted.stderr)
        verified = self.command(self.installed, "--verify-ui", "--quiet")
        self.assertEqual(verified.returncode, 0, verified.stderr)
        ui = self.command(self.home / ".local/bin/codex-info", "--ui")
        self.assertEqual(ui.returncode, 0, ui.stderr)
        self.assertEqual((self.home / "ui-started").read_text(), "verified local UI client\n")
        self.assertEqual(self.current().resolve(strict=True), recovered)
        self.assertEqual(effective.read_bytes(), owners_before)
        self.assertFalse(timer.exists() or timer.is_symlink())
        self.assertFalse(enabled.exists() or enabled.is_symlink())
        self.assertFalse(pathlib.Path(self.env["FAKE_TIMER_ACTIVE_FILE"]).exists())
        added = commands.read_text()[len(before):]
        self.assertNotRegex(added, r"systemctl .*\b(?:enable|start|restart)\b.*codex-info-update.timer")
        self.assertNotIn("/releases/", added)
        self.preserved_profile()

    def test_issue604_local_recovery_start_does_not_discover_or_replace(self):
        archive = self.local_recovery_bundle()
        pathlib.Path(self.env["FAKE_TIMER_ACTIVE_FILE"]).unlink(missing_ok=True)
        installed = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(installed.returncode, 0, installed.stderr)
        recovered = self.current().resolve(strict=True)
        self.canonical_process_fixture()
        # A newer published version must not replace this explicitly started
        # repair. Discovery itself fails, so a hidden network dependency fails.
        self.release("1.0.20", "3" * 40)
        self.env["FAKE_RELEASE_FAILURE"] = "1"
        result = self.command(self.installed, "--start")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.current().resolve(strict=True), recovered)
        commands = pathlib.Path(self.env["FAKE_LOG"]).read_text()
        self.assertNotIn("/releases/", commands)
        executions = [json.loads(line) for line in pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_text().splitlines()]
        self.assertEqual({item["unit"] for item in executions},
                         {"codex-info-recorder.service", "codex-info-rest.service"})
        self.assertTrue(all(pathlib.Path(item["path"]).parent == recovered for item in executions))
        self.assertFalse(pathlib.Path(self.env["FAKE_TIMER_ACTIVE_FILE"]).exists())
        self.preserved_profile()


    def test_issue604_local_recovery_keeps_canonical_ui_and_update_entrypoints(self):
        archive = self.local_recovery_bundle()
        result = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(result.returncode, 0, result.stderr)
        recovered = self.current().resolve(strict=True)
        self.assertNotEqual(recovered, self.old_generation)
        self.assertTrue(self.old_generation.is_dir())
        manifest = json.loads((recovered / "manifest.json").read_bytes())
        self.assertEqual(manifest["run_id"], "local-" + digest(recovered / "LOCAL_RECOVERY.json"))
        self.release("1.0.19", "1" * 40)
        # The fake service manager creates exe/cmdline on start; proc stat and
        # the listener fixture are independent OS inputs supplied by this test.
        for pid in (RECORDER_PID, REST_PID):
            directory = self.proc / str(pid)
            (directory / "fd").mkdir(parents=True)
            fields = ["S", *("0" for _ in range(18)), "101"]
            (directory / "stat").write_text(f"{pid} (local-recovery-fixture) " + " ".join(fields) + "\n")
        (self.proc / str(REST_PID) / "fd/3").symlink_to("socket:[599]")
        self.env["FAKE_REST_SOCKET_NET_FILE"] = str(self.proc / "net/tcp")
        self.env["FAKE_REST_SOCKET_CONTENT"] = (
            "  sl  local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n"
            "  0: 0100007F:2253 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 599 1\n"
        )
        self.control("running")
        ui = self.command(self.home / ".local/bin/codex-info", "--ui")
        self.assertEqual(ui.returncode, 0, ui.stderr)
        self.assertEqual((self.home / "ui-started").read_text(), "verified local UI client\n")
        self.assertNotIn("E_UI_SOURCE_UNAVAILABLE", ui.stderr)
        self.assertEqual(self.current().resolve(strict=True), recovered)
        self.assertFalse(list(self.unit_dir.glob("codex-info-*.service.d/*.conf")))
        update = self.command(self.installed, "--update")
        self.assertEqual(update.returncode, 0, update.stderr)
        self.assertEqual(self.current().resolve(strict=True), recovered)
        self.assertEqual(self.sentinel.read_bytes(), b"independent history preservation oracle\n")


    def test_issue604_local_recovery_allows_later_published_generation(self):
        archive = self.local_recovery_bundle()
        installed = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(installed.returncode, 0, installed.stderr)
        local_generation = self.current().resolve(strict=True)
        self.release("1.0.20", "3" * 40)
        updated = self.command(self.installed, "--update")
        self.assertEqual(updated.returncode, 0, updated.stderr)
        final = self.current().resolve(strict=True)
        self.assertNotEqual(final, local_generation)
        manifest = json.loads((final / "manifest.json").read_bytes())
        self.assertEqual((manifest["version"], manifest["run_id"]), ("1.0.20", "92001"))
        self.assertEqual(self.sentinel.read_bytes(), b"independent history preservation oracle\n")


    def test_issue604_public_release_rejects_local_recovery_producer(self):
        self.release("1.0.20", "2" * 40)
        archive = self.local_recovery_bundle("1.0.20")
        for path in (archive, pathlib.Path(str(archive) + ".sha256"),
                     archive.with_suffix("").with_suffix(".manifest.json")):
            shutil.copyfile(path, self.assets / path.name)
        metadata_path = pathlib.Path(self.env["FAKE_RELEASE_JSON"])
        metadata = json.loads(metadata_path.read_text())
        for asset in metadata["assets"]:
            path = self.assets / asset["name"]
            asset["size"], asset["digest"] = path.stat().st_size, "sha256:" + digest(path)
        metadata_path.write_text(json.dumps(metadata))
        result = self.command(self.installed, "--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("local recovery is not a published Release producer", result.stderr)
        self.assertEqual(self.current().resolve(strict=True), self.old_generation)
        self.assertEqual(self.sentinel.read_bytes(), b"independent history preservation oracle\n")



    def test_manual_digest_environment_cannot_replace_checksum_authority(self):
        archive = self.bundle("1.0.20", "3" * 40)
        checksum = pathlib.Path(str(archive) + ".sha256")
        checksum.write_text("0" * 64 + "  " + archive.name + "\n")
        predecessor = self.current().resolve(strict=True)
        control = self.home / ".local/share/codex-info/control-state.json"
        control_bytes = control.read_bytes()
        manifest_bytes = (predecessor / "manifest.json").read_bytes()

        # A malformed manual checksum must reject this otherwise valid bundle.
        baseline = self.command(INSTALLER, "--bundle", archive)
        self.assertNotEqual(baseline.returncode, 0, baseline.stdout + baseline.stderr)
        self.assertIn("archive digest differs", baseline.stderr)
        self.assertEqual(self.current().resolve(strict=True), predecessor)
        self.assertEqual(control.read_bytes(), control_bytes)
        self.preserved_profile()

        # subprocess closes unpassed descriptors. There is no inherited FD9
        # and no inherited L1 marker: the only changed input is this digest.
        self.assertNotIn("CODEX_INFO_INSTALL_LOCKED", self.env)
        self.env["CODEX_INFO_RELEASE_ARCHIVE_DIGEST"] = "sha256:" + digest(archive)
        untrusted = self.command(INSTALLER, "--bundle", archive)
        self.assertNotEqual(
            untrusted.returncode, 0, untrusted.stdout + untrusted.stderr
        )
        self.assertEqual(self.current().resolve(strict=True), predecessor)
        self.assertEqual((predecessor / "manifest.json").read_bytes(), manifest_bytes)
        self.assertEqual(control.read_bytes(), control_bytes)
        self.preserved_profile()


    def test_reordered_manifest_supports_installed_identity_and_prune(self):
        archive = self.bundle("1.0.20", "4" * 40)
        with tarfile.open(archive, "r:gz") as reader:
            members = {
                item.name: (reader.extractfile(item).read(), item.mode)
                for item in reader.getmembers()
            }

        def bind_metadata(payload, document):
            raw = json.dumps(document, indent=2).encode() + b"\n"
            payload["manifest.json"] = (raw, 0o644)
            sums = "".join(
                hashlib.sha256(data).hexdigest() + "  " + name + "\n"
                for name, (data, _) in sorted(payload.items())
                if name != "SHA256SUMS"
            ).encode()
            payload["SHA256SUMS"] = (sums, 0o644)
            return raw

        document = json.loads(members["manifest.json"][0])
        document["files"].reverse()
        raw = bind_metadata(members, document)
        with tarfile.open(archive, "w:gz") as writer:
            for name, (data, mode) in sorted(members.items()):
                item = tarfile.TarInfo(name)
                item.size, item.mode, item.mtime = len(data), mode, 0
                writer.addfile(item, io.BytesIO(data))
        archive.with_suffix("").with_suffix(".manifest.json").write_bytes(raw)
        pathlib.Path(str(archive) + ".sha256").write_text(
            digest(archive) + "  " + archive.name + "\n"
        )

        installed = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(installed.returncode, 0, installed.stdout + installed.stderr)
        current = self.current().resolve(strict=True)
        self.assertEqual(
            current.name,
            "1.0.20-" + "4" * 40 + "-" + hashlib.sha256(raw).hexdigest(),
        )
        self.assertEqual((current / "manifest.json").read_bytes(), raw)
        status = self.command(self.installed, "--status")
        self.assertEqual(status.returncode, 0, status.stdout + status.stderr)
        self.assertTrue(self.old_generation.is_dir())
        self.preserved_profile()

        # A separate unreferenced fixture generation has valid bytes/modes,
        # its own manifest identity, and the same non-sorted entry order.
        obsolete_document = dict(document)
        obsolete_document["version"] = "1.0.18"
        obsolete_members = dict(members)
        obsolete_raw = bind_metadata(obsolete_members, obsolete_document)
        obsolete = current.parent / (
            "1.0.18-" + "4" * 40 + "-"
            + hashlib.sha256(obsolete_raw).hexdigest()
        )
        obsolete.mkdir(mode=0o700)
        for name, (data, mode) in obsolete_members.items():
            path = obsolete / name
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path.write_bytes(data)
            path.chmod(mode)

        # Equal-version coherent no-update invokes the real reference-aware
        # pruner. It must accept and delete this obsolete generation.
        self.release("1.0.20", "4" * 40)
        updated = self.command(self.installed, "--update")
        self.assertEqual(updated.returncode, 0, updated.stdout + updated.stderr)
        self.assertFalse(obsolete.exists())
        self.assertEqual(self.current().resolve(strict=True), current)
        self.assertEqual((current / "manifest.json").read_bytes(), raw)
        self.assertTrue(self.old_generation.is_dir())
        self.preserved_profile()

    def release(self, version, source, recorder_bytes=None, include_sidecars=True, ui_bytes=None):
        archive = self.bundle(version, source, recorder_bytes=recorder_bytes, ui_bytes=ui_bytes)
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
        result = subprocess.run(  # noqa: S603 -- this offline adapter updates fixture lock identity only.  # nosec B603
            ["/usr/bin/python3", str(self.bin / "systemctl"), "--user", "restart", "--no-block", "codex-info-recorder.service"],
            env=self.env, capture_output=True, text=True, check=False, timeout=5,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def preserved_profile(self):
        self.assertEqual(self.sentinel.read_bytes(), b"independent history preservation oracle\n")
        self.assertEqual(self.auth.read_bytes(), b"independent credential preservation oracle\n")

    def incident_override_pair(self):
        incident = self.home / ".local/share/codex-info/incident-604-hotfix-20261018T1440JST"
        incident.mkdir(parents=True)
        self.hotfix = incident / "codex_info_recorder-v3"
        self.rest_hotfix = incident / "codex_info_rest"
        self.hotfix.write_bytes(b"isolated issue 604 recorder hotfix fixture\n")
        self.rest_hotfix.write_bytes(b"isolated issue 604 REST hotfix fixture\n")
        self.hotfix.chmod(0o700)
        self.rest_hotfix.chmod(0o700)

        self.override = self.unit_dir / "codex-info-recorder.service.d/604-hotfix.conf"
        self.rest_override = self.unit_dir / "codex-info-rest.service.d/604-hotfix.conf"
        self.override.parent.mkdir(parents=True)
        self.rest_override.parent.mkdir(parents=True)
        self.override_bytes = ("[Service]\nExecStart=\nExecStart=" + str(self.hotfix) + "\n").encode()
        self.rest_override_bytes = (
            "[Service]\nExecStart=\nExecStart=" + str(self.rest_hotfix) + " --port 8787\n"
        ).encode()
        self.override.write_bytes(self.override_bytes)
        self.rest_override.write_bytes(self.rest_override_bytes)
        self.override.chmod(0o600)
        self.rest_override.chmod(0o600)

        for pid, target in ((RECORDER_PID, self.hotfix), (REST_PID, self.rest_hotfix)):
            directory = self.proc / str(pid)
            (directory / "fd").mkdir(parents=True)
            fields = ["S", *("0" for _ in range(18)), "101"]
            (directory / "stat").write_text(f"{pid} (issue-604-hotfix-fixture) " + " ".join(fields) + "\n")
            (directory / "exe").symlink_to(target)
            command = [str(target)] + (["--port", "8787"] if pid == REST_PID else [])
            (directory / "cmdline").write_bytes(b"\0".join(part.encode() for part in command) + b"\0")
        (self.proc / str(REST_PID) / "fd/3").symlink_to("socket:[599]")
        socket = (
            "  sl  local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n"
            "  0: 0100007F:2253 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 599 1\n"
        )
        fields = socket.splitlines()[1].split()
        self.assertEqual(fields[7], "1000")
        self.assertEqual(fields[9], "599")
        self.assertEqual(
            os.readlink(self.proc / str(REST_PID) / "fd/3"),
            "socket:[599]",
        )
        (self.proc / "net/tcp").write_text(socket)
        self.env["FAKE_REST_SOCKET_NET_FILE"] = str(self.proc / "net/tcp")
        self.env["FAKE_REST_SOCKET_CONTENT"] = socket
        pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]).touch()
        pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"]).touch()
        self.control("running")
        # Literal argv, fixed interpreter, test-owned adapter cwd, shell=False.
        recorder = subprocess.run(  # nosec B603
            [
                "/usr/bin/python3", "systemctl", "--user", "restart", "--no-block",
                "codex-info-recorder.service",
            ],
            cwd=self.bin, env=self.env, capture_output=True, text=True,
            check=False, timeout=5, shell=False,
        )
        self.assertEqual(recorder.returncode, 0, recorder.stderr)
        # Literal argv, fixed interpreter, test-owned adapter cwd, shell=False.
        rest = subprocess.run(  # nosec B603
            [
                "/usr/bin/python3", "systemctl", "--user", "restart", "--no-block",
                "codex-info-rest.service",
            ],
            cwd=self.bin, env=self.env, capture_output=True, text=True,
            check=False, timeout=5, shell=False,
        )
        self.assertEqual(rest.returncode, 0, rest.stderr)

    def incident_database(self):
        db = self.home / ".local/share/codex-info/history/accounts/fixture-account/epoch-one/usage_history.sqlite3"
        db.parent.mkdir(parents=True)
        with sqlite3.connect(db) as connection:
            connection.execute("CREATE TABLE preserved_usage (observed_at INTEGER NOT NULL, tokens INTEGER NOT NULL)")
            connection.execute("INSERT INTO preserved_usage VALUES (1792478542, 123456)")
            rows = connection.execute("SELECT observed_at, tokens FROM preserved_usage").fetchall()
        return db, db.read_bytes(), rows

    def incident_pair_prestate(self, db, db_bytes, rows):
        enabled_paths = (
            self.unit_dir / "default.target.wants/codex-info-recorder.service",
            self.unit_dir / "default.target.wants/codex-info-rest.service",
        )
        process_state = {}
        active_files = {
            RECORDER_PID: pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]),
            REST_PID: pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"]),
        }
        for pid, active_file in active_files.items():
            if not active_file.is_file():
                continue
            directory = self.proc / str(pid)
            executable = directory / "exe"
            target = executable.resolve(strict=True)
            metadata = target.stat()
            process_state[pid] = {
                "target": target,
                "device": metadata.st_dev,
                "inode": metadata.st_ino,
                "sha256": digest(target),
                "cmdline": (directory / "cmdline").read_bytes(),
            }
        payload_state = {}
        for path in (self.hotfix, self.rest_hotfix):
            metadata = path.stat()
            payload_state[path] = (metadata.st_dev, metadata.st_ino, digest(path), path.read_bytes())
        return {
            "generation": self.current().resolve(strict=True),
            "recorder_override": (self.override.read_bytes(), self.override.stat().st_mode & 0o777),
            "rest_override": (self.rest_override.read_bytes(), self.rest_override.stat().st_mode & 0o777),
            "payloads": payload_state,
            "processes": process_state,
            "active": tuple(path.exists() for path in (
                pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]), pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"])
            )),
            "enabled": tuple((path.is_symlink(), os.readlink(path) if path.is_symlink() else None) for path in enabled_paths),
            "db": (db, db_bytes, rows),
        }

    def assert_incident_pair_prestate(self, before):
        self.assertEqual(self.current().resolve(strict=True), before["generation"])
        for path, expected in ((self.override, before["recorder_override"]), (self.rest_override, before["rest_override"])):
            self.assertEqual((path.read_bytes(), path.stat().st_mode & 0o777), expected)
        for path, expected in before["payloads"].items():
            metadata = path.stat()
            self.assertEqual((metadata.st_dev, metadata.st_ino, digest(path), path.read_bytes()), expected)
        for pid, expected in before["processes"].items():
            directory = self.proc / str(pid)
            actual = directory / "exe"
            metadata = actual.stat()
            self.assertEqual(actual.resolve(strict=True), expected["target"])
            self.assertEqual((metadata.st_dev, metadata.st_ino, digest(actual)), (
                expected["device"], expected["inode"], expected["sha256"]
            ))
            self.assertEqual((directory / "cmdline").read_bytes(), expected["cmdline"])
        active = tuple(path.exists() for path in (
            pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]), pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"])
        ))
        self.assertEqual(active, before["active"])
        enabled_paths = (
            self.unit_dir / "default.target.wants/codex-info-recorder.service",
            self.unit_dir / "default.target.wants/codex-info-rest.service",
        )
        enabled = tuple((path.is_symlink(), os.readlink(path) if path.is_symlink() else None) for path in enabled_paths)
        self.assertEqual(enabled, before["enabled"])
        db, db_bytes, rows = before["db"]
        self.assertEqual(db.read_bytes(), db_bytes)
        with sqlite3.connect(db) as connection:
            self.assertEqual(connection.execute("SELECT observed_at, tokens FROM preserved_usage").fetchall(), rows)
        self.preserved_profile()

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



    def wire_error_model_frame(self):
        return {
            "api_version": "v4", "state": "error", "observed_at": 1792228061,
            "authenticated": True, "plan_label": None,
            "quota": {
                "remaining_percent": 44.0, "reset_at": 1792823932,
                "window_seconds": 604800, "monthly": False,
            },
            "models_status": "partial",
            "models": [{
                "model": "gpt-6.1-sol", "total_tokens": 15,
                "input_tokens": 12, "cached_input_tokens": 3,
                "cache_write_input_tokens": None, "output_tokens": 3,
                "estimated_cost": None,
            }],
            "active_thread_count": 0, "open_session_thread_count": 0,
        }

    def test_issue659_expired_owner_with_retained_error_current_is_available(self):
        installed = self.command(INSTALLER, "--bundle", self.local_recovery_bundle())
        self.assertEqual(installed.returncode, 0, installed.stdout + installed.stderr)
        self.canonical_process_fixture()
        started = self.command(self.installed, "--start")
        self.assertEqual(started.returncode, 0, started.stdout + started.stderr)

        state_path = self.home / ".codex/history/recorder-state.json"
        state = json.loads(state_path.read_text())
        last_commit = int(time.time()) - 151
        state.update(
            write_state="degraded", partition_id_hash="a" * 64,
            data_generation=1, collector_epoch="b" * 32, cycle_seq=1,
            last_commit_unix=last_commit, updated_at_unix=last_commit,
        )
        state_path.write_text(json.dumps(state) + "\n")
        before = state_path.read_bytes()
        self.env.update(
            FAKE_CURRENT_STATUS="200",
            FAKE_CURRENT_BODY=json.dumps(self.wire_error_model_frame()),
            FAKE_REJECT_LEGACY_DETAILS="1",
        )
        result = self.command(self.installed, "--verify-runtime")
        self.assertEqual(
            result.returncode, 0,
            "RETAINED_ERROR_CURRENT_AGE_WAS_REJECTED: " + result.stdout + result.stderr,
        )
        self.assertEqual(state_path.read_bytes(), before, "readiness must not forge progress")
        self.preserved_profile()

    def probe_wire_model_frame(self, frame):
        self.env.update(
            FAKE_CURRENT_STATUS="200", FAKE_CURRENT_BODY=json.dumps(frame),
            FAKE_REJECT_LEGACY_DETAILS="1",
        )
        return self.current_readiness()

    def test_current_readiness_rejects_null_required_model_components(self):
        valid = self.probe_wire_model_frame(self.wire_error_model_frame())
        self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
        for component in ("input_tokens", "cached_input_tokens", "output_tokens"):
            with self.subTest(component=component):
                frame = self.wire_error_model_frame()
                frame["models"][0][component] = None
                result = self.probe_wire_model_frame(frame)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.preserved_profile()

    def test_current_readiness_rejects_inconsistent_model_tokens(self):
        valid = self.probe_wire_model_frame(self.wire_error_model_frame())
        self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
        invalid = (
            {"total_tokens": 16},
            {"cached_input_tokens": 13},
            {"cache_write_input_tokens": 10},
        )
        for changes in invalid:
            with self.subTest(changes=changes):
                frame = self.wire_error_model_frame()
                frame["models"][0].update(changes)
                result = self.probe_wire_model_frame(frame)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.preserved_profile()


    def test_current_readiness_preserves_known_price_without_repricing(self):
        frame = CurrentFrameDomainTests.priced_frame(self)
        before = json.dumps(frame)
        result = self.probe_wire_model_frame(frame)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.env["FAKE_CURRENT_BODY"], before)
        self.assertEqual(
            frame["models"][0]["estimated_cost"]["total_dollars"].hex(),
            float(119.20181600000001).hex(),
        )
        self.preserved_profile()


    def test_absolute_fixed_entrypoints_keep_current_indirection(self):
        ui = b"""#!/usr/bin/python3
import os, pathlib, sys
if "--help" in sys.argv:
    print("fixture launcher help")
    raise SystemExit(0)
pathlib.Path(os.environ["HOME"], "ui-started").write_text("verified fixed UI\\n")
if fifo := os.environ.get("CODEX_INFO_UI_STARTUP_FIFO"):
    with open(fifo, "w") as ready:
        ready.write("READY\\n")
"""
        archive = self.local_recovery_bundle(ui_bytes=ui)
        installed = self.command(INSTALLER, "--bundle", archive)
        self.assertEqual(installed.returncode, 0, installed.stderr)
        self.canonical_process_fixture()
        self.env["FAKE_RELEASE_FAILURE"] = "1"
        normal = self.command(self.installed, "--start")
        self.assertEqual(normal.returncode, 0, normal.stderr)
        generation = self.current().resolve(strict=True)
        manifest_before = (generation / "manifest.json").read_bytes()
        document = json.loads(manifest_before)
        members_before = {
            entry["path"]: digest(generation / entry["path"])
            for entry in document["files"]
        }
        effective = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"])
        owners_before = effective.read_bytes()
        current_path = self.home / ".local/share/codex-info/current"
        links = {
            self.home / ".local/bin/codex_info": "codex_info",
            self.home / ".local/bin/codex_info_recorder": "codex_info_recorder",
            self.home / ".local/bin/codex_info_rest": "codex_info_rest",
            self.home / ".local/bin/codex-info": "run.sh",
            self.home / ".local/libexec/codex-info-install.sh": "install.sh",
            self.home / ".local/share/codex-info/manifest.json": "manifest.json",
        }
        for unit in (
            "codex-info-recorder.service", "codex-info-rest.service",
            "codex-info-update.service", "codex-info-update.timer",
        ):
            links[self.unit_dir / unit] = unit
        for link, member in links.items():
            self.assertTrue(link.is_symlink(), str(link))
            expected = current_path / member
            self.assertEqual(
                os.path.abspath(os.path.join(link.parent, os.readlink(link))),
                str(expected),
            )
            link.unlink()
            link.symlink_to(expected)
        checks = (
            ("start", self.installed, "--start"),
            ("verify-runtime", self.installed, "--verify-runtime"),
            ("verify-ui", self.installed, "--verify-ui"),
            ("status", self.installed, "--status"),
            ("launcher-ui", self.home / ".local/bin/codex-info", "--ui"),
            ("launcher-help", self.home / ".local/bin/codex-info", "--help"),
        )
        for label, executable, argument in checks:
            with self.subTest(operation=label):
                result = self.command(executable, argument)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                if label == "launcher-ui":
                    self.assertEqual(
                        (self.home / "ui-started").read_text(), "verified fixed UI\n"
                    )
                if label == "launcher-help":
                    self.assertIn("fixture launcher help", result.stdout)
        self.assertEqual(self.current().resolve(strict=True), generation)
        self.assertEqual((generation / "manifest.json").read_bytes(), manifest_before)
        self.assertEqual(
            {
                entry["path"]: digest(generation / entry["path"])
                for entry in document["files"]
            },
            members_before,
        )
        self.assertEqual(effective.read_bytes(), owners_before)
        self.assertNotIn("/releases/", pathlib.Path(self.env["FAKE_LOG"]).read_text())
        self.preserved_profile()


    def test_absolute_fixed_entrypoints_update_and_keep_verified_rollback(self):
        original = self.current().resolve(strict=True)
        links = {
            self.home / ".local/bin/codex_info": "codex_info",
            self.home / ".local/bin/codex_info_recorder": "codex_info_recorder",
            self.home / ".local/bin/codex_info_rest": "codex_info_rest",
            self.home / ".local/bin/codex-info": "run.sh",
            self.home / ".local/libexec/codex-info-install.sh": "install.sh",
            self.home / ".local/share/codex-info/manifest.json": "manifest.json",
        }
        for unit in (
            "codex-info-recorder.service", "codex-info-rest.service",
            "codex-info-update.service", "codex-info-update.timer",
        ):
            links[self.unit_dir / unit] = unit
        for path, member in links.items():
            self.assertTrue(path.is_symlink(), str(path))
            path.unlink()
            path.symlink_to(self.current() / member)
        self.release("1.0.20", "3" * 40)
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        final = self.current().resolve(strict=True)
        self.assertNotEqual(final, original)
        self.assertEqual(
            json.loads((final / "manifest.json").read_bytes())["version"],
            "1.0.20",
        )
        self.assertTrue(original.is_dir(), "verified rollback generation was removed")
        self.preserved_profile()





    def test_absent_control_is_restored_after_late_rollback(self):
        self.active_override(owned=True)
        control = self.home / ".local/share/codex-info/control-state.json"
        control.unlink()
        original = self.current().resolve(strict=True)
        original_manifest = (original / "manifest.json").read_bytes()
        self.env["FAKE_CURRENT_STATUS"] = "200"
        self.env["FAKE_CURRENT_BODY"] = json.dumps({
            "api_version": "v4", "state": "auth_required",
            "observed_at": None, "authenticated": False, "plan_label": None,
            "quota": None, "models_status": "unavailable", "models": [],
            "active_thread_count": 0, "open_session_thread_count": 0,
        })
        self.release("1.0.20", "3" * 40)
        self.prepared_failure_environment("committed")
        result = self.command(self.installed, "--update")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        marker_path = self.home / "prepared-io-fault.json"
        self.assertTrue(
            marker_path.is_file(),
            "late control fault was not reached:\n" + result.stdout + result.stderr,
        )
        marker = json.loads(marker_path.read_text())
        self.assertEqual(marker["kind"], "committed")
        self.assertEqual(self.current().resolve(strict=True), original)
        self.assertEqual((original / "manifest.json").read_bytes(), original_manifest)
        self.assertFalse(control.exists() or control.is_symlink())
        journal = json.loads(
            (self.home / ".local/share/codex-info/install-transaction.json").read_text()
        )
        self.assertEqual(journal["phase"], "committed")
        self.assertEqual(journal["operation_id"], marker["operation_id"])
        self.assertEqual(journal["old_generation"], original.name)
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)
        self.assertTrue(pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]).is_file())
        self.assertTrue(pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"]).is_file())
        undo = self.home / ".local/share/codex-info/legacy-backups" / (
            marker["operation_id"] + "-control-state.json"
        )
        self.assertFalse(undo.exists() or undo.is_symlink())
        self.preserved_profile()


    def test_resumed_and_no_update_journal_io_restore_the_same_operation(self):
        shim = r"""
import errno, json, os, pathlib, sys
assert sys.argv[1] == "-"
sys.argv = ["-", *sys.argv[2:]]
program = sys.stdin.read()
home = pathlib.Path(os.environ["HOME"])
journal = home / ".local/share/codex-info/install-transaction.json"
marker = home / "resume-journal-fault.json"
trace = home / "resume-prepared-trace.jsonl"
expected = os.environ["FAKE_RESUME_JOURNAL_PHASE"]
original_replace = os.replace
def controlled_replace(source, destination):
    if pathlib.Path(destination) == journal:
        pending = json.loads(pathlib.Path(source).read_text())
        if pending["phase"] == "prepared":
            with trace.open("a") as output:
                output.write(json.dumps(pending) + "\n")
        if pending["phase"] == expected and not marker.exists():
            prior = json.loads(journal.read_text())
            marker.write_text(json.dumps({
                "phase": prior["phase"],
                "operation_id": prior["operation_id"],
                "pending_operation_id": pending["operation_id"],
                "old_generation": prior["old_generation"],
                "new_generation": prior["new_generation"],
            }))
            raise OSError(errno.EIO, "controlled resumed transaction journal failure")
    return original_replace(source, destination)
os.replace = controlled_replace
exec(compile(program, "actual-installer-inline-python", "exec"))
"""
        wrapper = """#!/bin/bash
if [[ "${1:-}" == -B ]]; then shift; fi
if [[ "${1:-}" == - ]]; then
    exec /usr/bin/python3 -B "$FAKE_RESUME_JOURNAL_SHIM" "$@"
fi
exec /usr/bin/python3 -B "$@"
"""
        for index, entry in enumerate(("resume", "no-update")):
            with self.subTest(entry=entry):
                if index:
                    self.doCleanups()
                    self.setUp()
                before = None
                if entry == "no-update":
                    self.incident_override_pair()
                db, db_bytes, rows = self.incident_database()
                if entry == "no-update":
                    before = self.incident_pair_prestate(db, db_bytes, rows)
                original = self.current().resolve(strict=True)
                manifest_before = (original / "manifest.json").read_bytes()
                document = json.loads(manifest_before)
                members_before = {
                    item["path"]: digest(original / item["path"])
                    for item in document["files"]
                }
                control = self.home / ".local/share/codex-info/control-state.json"
                control_before = (control.read_bytes(), control.stat().st_mode & 0o777)
                journal_path = self.home / ".local/share/codex-info/install-transaction.json"

                def runtime_intent():
                    result = {}
                    for unit in (
                        "codex-info-recorder.service",
                        "codex-info-rest.service",
                        "codex-info-update.timer",
                    ):
                        for query in ("is-active", "is-enabled"):
                            probe = subprocess.run(  # noqa: S603 -- test-owned fake systemctl and literal query/unit arguments.
                                [str(self.bin / "systemctl"), "--user",
                                 query, "--quiet", unit],
                                env=self.env, capture_output=True, text=True,
                                check=False, timeout=5,
                            )
                            allowed = {0, 3, 4} if query == "is-active" else {0, 1, 4}
                            self.assertIn(probe.returncode, allowed, probe.stderr)
                            result[(unit, query)] = probe.returncode
                    return result

                intent_before = runtime_intent()
                if entry == "resume":
                    self.release("1.0.20", "3" * 40)
                    self.env["CODEX_INFO_INTERRUPT_PHASE"] = "current_switched"
                    interrupted = self.command(self.installed, "--update")
                    self.assertNotEqual(
                        interrupted.returncode, 0,
                        interrupted.stdout + interrupted.stderr,
                    )
                    pending = json.loads(journal_path.read_text())
                    self.assertEqual(pending["phase"], "current_switched")
                    self.assertEqual(pending["old_generation"], original.name)
                    self.assertTrue(
                        self.current().resolve(strict=True).name.startswith("1.0.20-")
                    )
                    del self.env["CODEX_INFO_INTERRUPT_PHASE"]
                else:
                    self.release("1.0.19", "1" * 40)

                private_bin = self.root / "resume-journal-python"
                private_bin.mkdir()
                shim_path = private_bin / "shim.py"
                shim_path.write_text(shim)
                executable = private_bin / "python3"
                executable.write_text(wrapper)
                executable.chmod(0o755)
                self.env["PATH"] = str(private_bin) + os.pathsep + self.env["PATH"]
                self.env["FAKE_RESUME_JOURNAL_SHIM"] = str(shim_path)
                self.env["FAKE_RESUME_JOURNAL_PHASE"] = (
                    "candidate_verified" if entry == "resume" else "current_switched"
                )
                arguments = ["--update"]
                if entry == "no-update":
                    arguments.append("--migrate-recorder-override")
                result = self.command(self.installed, *arguments)
                diagnostic = result.stdout + result.stderr
                self.assertNotEqual(result.returncode, 0, diagnostic)
                marker_path = self.home / "resume-journal-fault.json"
                self.assertTrue(
                    marker_path.is_file(),
                    "actual journal fault was not reached:\n" + diagnostic,
                )
                marker = json.loads(marker_path.read_text())
                self.assertEqual(
                    marker["phase"],
                    "current_switched" if entry == "resume" else "prepared",
                )
                self.assertEqual(
                    marker["operation_id"], marker["pending_operation_id"]
                )
                self.assertEqual(marker["old_generation"], original.name)
                self.assertNotIn("command not found", diagnostic)
                self.assertNotIn("NameError", diagnostic)
                self.assertEqual(
                    (original / "manifest.json").read_bytes(), manifest_before
                )
                self.assertEqual({
                    item["path"]: digest(original / item["path"])
                    for item in document["files"]
                }, members_before)
                self.assertEqual(db.read_bytes(), db_bytes)
                with sqlite3.connect(db) as connection:
                    self.assertEqual(
                        connection.execute(
                            "SELECT observed_at, tokens FROM preserved_usage"
                        ).fetchall(), rows,
                    )
                self.preserved_profile()

                if entry == "no-update":
                    events = [
                        json.loads(line)
                        for line in (self.home / "resume-prepared-trace.jsonl")
                        .read_text().splitlines()
                    ]
                    self.assertTrue(events)
                    comparable = [
                        {key: value for key, value in event.items()
                         if key != "updated_at_unix"}
                        for event in events
                    ]
                    self.assertTrue(
                        all(value == comparable[0] for value in comparable),
                        "no-update prepared payload changed beyond timestamp",
                    )
                    self.assertTrue(all(
                        event["operation_id"] == marker["operation_id"]
                        for event in events
                    ))
                    print(
                        "NO_UPDATE_PREPARED_IDENTICAL=True count=" + str(len(events)),
                        flush=True,
                    )
                    self.assertTrue(
                        self.override.is_file(),
                        "no-update journal I/O left Recorder configuration removed",
                    )
                    self.assert_incident_pair_prestate(before)
                    self.assertEqual(len(events), 1)
                else:
                    self.assertEqual(
                        self.current().resolve(strict=True), original,
                        "resumed journal I/O left candidate current",
                    )
                self.assertEqual(self.current().resolve(strict=True), original)
                restored = json.loads(journal_path.read_text())
                self.assertEqual(restored["phase"], "committed")
                self.assertEqual(restored["operation_id"], marker["operation_id"])
                self.assertEqual(restored["old_generation"], original.name)
                self.assertEqual(
                    (control.read_bytes(), control.stat().st_mode & 0o777),
                    control_before,
                )
                self.assertEqual(runtime_intent(), intent_before)
                undo = self.home / ".local/share/codex-info/legacy-backups" / (
                    marker["operation_id"] + "-control-state.json"
                )
                self.assertFalse(undo.exists() or undo.is_symlink())
                print(
                    "RESTORED_JOURNAL_IO entry=" + entry
                    + " same_operation=True control=True runtime_intent=True",
                    flush=True,
                )

    def test_committed_resume_cleans_only_its_control_undo(self):
        control = self.home / ".local/share/codex-info/control-state.json"
        control_before = control.read_bytes()
        self.release("1.0.20", "3" * 40)
        self.env["CODEX_INFO_INTERRUPT_PHASE"] = "committed"
        interrupted = self.command(self.installed, "--update")
        self.assertNotEqual(
            interrupted.returncode, 0, interrupted.stdout + interrupted.stderr
        )
        generation = self.current().resolve(strict=True)
        self.assertTrue(generation.name.startswith("1.0.20-"))
        journal_path = self.home / ".local/share/codex-info/install-transaction.json"
        journal_bytes = journal_path.read_bytes()
        journal = json.loads(journal_bytes)
        self.assertEqual(journal["phase"], "committed")
        undo = self.home / ".local/share/codex-info/legacy-backups" / (
            journal["operation_id"] + "-control-state.json"
        )
        self.assertEqual(undo.read_bytes(), control_before)
        self.assertEqual(undo.stat().st_mode & 0o777, 0o600)
        self.assertTrue(control.is_file(), "copying undo must not remove live intent")
        del self.env["CODEX_INFO_INTERRUPT_PHASE"]
        readonly = self.command(self.installed, "--status")
        self.assertEqual(readonly.returncode, 0, readonly.stdout + readonly.stderr)
        self.assertEqual(undo.read_bytes(), control_before)
        self.assertEqual(journal_path.read_bytes(), journal_bytes)
        resumed = self.command(self.installed, "--update")
        self.assertEqual(resumed.returncode, 0, resumed.stdout + resumed.stderr)
        self.assertEqual(self.current().resolve(strict=True), generation)
        self.assertEqual(journal_path.read_bytes(), journal_bytes)
        self.assertFalse(undo.exists() or undo.is_symlink())
        self.assertFalse(
            undo.parent.exists(),
            "the new empty control-undo directory must not remain after terminal cleanup",
        )
        self.preserved_profile()

    def prepared_failure_environment(self, kind):
        shim = r"""
import errno, json, os, pathlib, sys
assert sys.argv[1] == "-"
sys.argv = ["-", *sys.argv[2:]]
program = sys.stdin.read()
home = pathlib.Path(os.environ["HOME"])
share = home / ".local/share/codex-info"
journal = share / "install-transaction.json"
marker = home / "prepared-io-fault.json"
trace = home / "prepared-write-trace.jsonl"
kind = os.environ["FAKE_PREPARED_IO_KIND"]
original_replace = os.replace
def observed(source, destination, pending=None):
    prior = json.loads(journal.read_text())
    marker.write_text(json.dumps({
        "kind": kind, "phase": prior["phase"],
        "operation_id": (pending or prior)["operation_id"],
        "source": str(source), "destination": str(destination),
    }))
    raise OSError(errno.EIO, "controlled prepared transaction I/O failure")
def controlled_replace(source, destination):
    destination = pathlib.Path(destination)
    if destination == journal:
        pending = json.loads(pathlib.Path(source).read_text())
        candidate = pending["new_generation"].startswith("1.0.20-")
        if candidate and pending["phase"] == "prepared":
            with trace.open("a") as output:
                output.write(json.dumps(pending) + "\n")
        if (
            candidate and not marker.exists()
            and kind in {"prepared", "activation_requested", "candidate_verified", "committed"}
            and pending["phase"] == kind
        ):
            observed(source, destination, pending)
    if not marker.exists():
        override = home / ".config/systemd/user/codex-info-recorder.service.d/90-issue-134-current-cli.conf"
        if kind == "legacy-backup" and pathlib.Path(source) == override:
            observed(source, destination)
        if kind == "entrypoint" and destination == home / ".local/bin/codex_info":
            observed(source, destination)
        if kind == "control" and destination == share / "control-state.json":
            pending = json.loads(pathlib.Path(source).read_text())
            if pending["generation_id"].startswith("1.0.20-"):
                observed(source, destination)
    return original_replace(source, destination)
os.replace = controlled_replace
exec(compile(program, "actual-installer-inline-python", "exec"))
"""
        wrapper = """#!/bin/bash
if [[ "${1:-}" == -B ]]; then shift; fi
if [[ "${1:-}" == - ]]; then
    exec /usr/bin/python3 -B "$FAKE_PREPARED_IO_SHIM" "$@"
fi
exec /usr/bin/python3 -B "$@"
"""
        private_bin = self.root / "prepared-io-python"
        private_bin.mkdir()
        shim_path = private_bin / "shim.py"
        shim_path.write_text(shim)
        executable = private_bin / "python3"
        executable.write_text(wrapper)
        executable.chmod(0o755)
        self.env["PATH"] = str(private_bin) + os.pathsep + self.env["PATH"]
        self.env["FAKE_PREPARED_IO_SHIM"] = str(shim_path)
        self.env["FAKE_PREPARED_IO_KIND"] = kind

    def test_remaining_prepared_io_failures_restore_the_same_operation(self):
        cases = (
            "legacy-backup", "entrypoint", "activation_requested",
            "candidate_verified", "committed", "control", "prepared",
        )
        for index, kind in enumerate(cases):
            with self.subTest(kind=kind):
                if index:
                    self.doCleanups()
                    self.setUp()
                if kind == "legacy-backup":
                    self.active_override(owned=True)
                original = self.current().resolve(strict=True)
                manifest_before = (original / "manifest.json").read_bytes()
                document = json.loads(manifest_before)
                members_before = {
                    item["path"]: digest(original / item["path"])
                    for item in document["files"]
                }
                control_path = self.home / ".local/share/codex-info/control-state.json"
                control_before = control_path.read_bytes()
                journal_path = self.home / ".local/share/codex-info/install-transaction.json"
                journal_before = journal_path.read_bytes()

                def runtime_intent():
                    observed = {}
                    for unit in (
                        "codex-info-recorder.service",
                        "codex-info-rest.service",
                        "codex-info-update.timer",
                    ):
                        for query in ("is-active", "is-enabled"):
                            result = subprocess.run(  # noqa: S603 -- test-owned fake systemctl and literal query/unit arguments.
                                [str(self.bin / "systemctl"), "--user", query, "--quiet", unit],
                                env=self.env, capture_output=True, text=True,
                                check=False, timeout=5,
                            )
                            allowed = {0, 3, 4} if query == "is-active" else {0, 1, 4}
                            self.assertIn(result.returncode, allowed, result.stderr)
                            observed[(unit, query)] = result.returncode
                    return observed

                intent_before = runtime_intent()
                self.release("1.0.20", "3" * 40)
                self.prepared_failure_environment(kind)
                result = self.command(self.installed, "--update")
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                marker = json.loads((self.home / "prepared-io-fault.json").read_text())
                self.assertEqual(marker["kind"], kind)
                self.assertTrue(marker["operation_id"])
                self.assertNotIn("command not found", result.stderr)
                self.assertNotIn("NameError", result.stderr)
                self.assertEqual((original / "manifest.json").read_bytes(), manifest_before)
                self.assertEqual({
                    item["path"]: digest(original / item["path"])
                    for item in document["files"]
                }, members_before)
                self.preserved_profile()
                print(
                    "Observed prepared I/O failure:", kind, marker["phase"],
                    "control_changed=", control_path.read_bytes() != control_before,
                    flush=True,
                )
                self.assertEqual(
                    self.current().resolve(strict=True), original,
                    "ordinary prepared failure changed current instead of restoring it",
                )
                journal = json.loads(journal_path.read_text())
                self.assertEqual(
                    journal["phase"], "committed",
                    "ordinary prepared failure left its transaction unsettled",
                )
                if kind == "prepared":
                    self.assertEqual(journal_path.read_bytes(), journal_before)
                else:
                    self.assertEqual(journal["operation_id"], marker["operation_id"])
                    self.assertEqual(journal["old_generation"], original.name)
                self.assertEqual(runtime_intent(), intent_before)
                self.assertEqual(control_path.read_bytes(), control_before)
                undo = self.home / ".local/share/codex-info/legacy-backups" / (
                    marker["operation_id"] + "-control-state.json"
                )
                self.assertFalse(undo.exists() or undo.is_symlink())
                if kind == "legacy-backup":
                    self.assertEqual(self.override.read_bytes(), self.override_bytes)
                    self.assertEqual(self.hotfix.read_bytes(), self.hotfix_bytes)

    def test_prepared_authority_is_not_republished_without_a_state_change(self):
        self.release("1.0.20", "3" * 40)
        self.prepared_failure_environment("prepared-count")
        result = self.command(self.installed, "--update")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        generation = self.current().resolve(strict=True)
        self.assertTrue(generation.name.startswith("1.0.20-"))
        journal = json.loads(
            (self.home / ".local/share/codex-info/install-transaction.json").read_text()
        )
        self.assertEqual(journal["phase"], "committed")
        undo = self.home / ".local/share/codex-info/legacy-backups" / (
            journal["operation_id"] + "-control-state.json"
        )
        self.assertFalse(undo.exists() or undo.is_symlink())
        events = [
            json.loads(line)
            for line in (self.home / "prepared-write-trace.jsonl").read_text().splitlines()
        ]
        self.assertTrue(events)
        comparable = [
            {key: value for key, value in event.items() if key != "updated_at_unix"}
            for event in events
        ]
        canonical = [
            json.dumps(event, sort_keys=True, separators=(",", ":"))
            for event in comparable
        ]
        self.assertTrue(
            all(value == canonical[0] for value in canonical),
            "prepared payload changed beyond its diagnostic timestamp",
        )
        for event in events:
            self.assertEqual(event["operation_id"], journal["operation_id"])
            self.assertEqual(event["phase"], "prepared")
            self.assertEqual(event["old_generation"], self.old_generation.name)
            self.assertEqual(event["new_generation"], generation.name)
        print(
            "PREPARED_PAYLOAD_IDENTICAL=True",
            "count=" + str(len(events)),
            "keys=" + ",".join(sorted(comparable[0])),
            "payload_sha256=" + hashlib.sha256(canonical[0].encode()).hexdigest(),
            flush=True,
        )
        self.preserved_profile()
        self.assertEqual(
            len(events), 1,
            "the unchanged prepared authority was published repeatedly",
        )

    def test_paired_removal_io_failure_restores_the_same_operation(self):
        shim = r"""
import errno, json, os, pathlib, sys
assert sys.argv[1] == "-"
sys.argv = ["-", *sys.argv[2:]]
program = sys.stdin.read()
home = pathlib.Path(os.environ["HOME"])
marker = home / "paired-removal-fault.json"
journal = home / ".local/share/codex-info/install-transaction.json"
recorder = home / ".config/systemd/user/codex-info-recorder.service.d/604-hotfix.conf"
rest = home / ".config/systemd/user/codex-info-rest.service.d/604-hotfix.conf"
original_unlink = pathlib.Path.unlink
def controlled_unlink(path, *args, **kwargs):
    if path == rest and not marker.exists():
        document = json.loads(journal.read_text())
        assert document["phase"] == "prepared"
        assert not recorder.exists()
        marker.write_text(json.dumps({
            "phase": document["phase"],
            "operation_id": document["operation_id"],
            "rest_bytes": path.read_bytes().hex(),
        }))
        raise OSError(errno.EIO, "controlled paired REST removal failure")
    return original_unlink(path, *args, **kwargs)
pathlib.Path.unlink = controlled_unlink
exec(compile(program, "actual-installer-inline-python", "exec"))
"""
        wrapper = """#!/bin/bash
if [[ "${1:-}" == -B ]]; then shift; fi
if [[ "${1:-}" == - ]]; then
    exec /usr/bin/python3 -B "$FAKE_PAIRED_REMOVAL_SHIM" "$@"
fi
exec /usr/bin/python3 -B "$@"
"""
        for index, entry in enumerate(("candidate", "no-update")):
            with self.subTest(entry=entry):
                if index:
                    self.doCleanups()
                    self.setUp()
                self.incident_override_pair()
                db, db_bytes, rows = self.incident_database()
                before = self.incident_pair_prestate(db, db_bytes, rows)
                control_path = self.home / ".local/share/codex-info/control-state.json"
                control_before = control_path.read_bytes()
                generation = before["generation"]
                manifest_before = (generation / "manifest.json").read_bytes()
                document = json.loads(manifest_before)
                members_before = {
                    item["path"]: digest(generation / item["path"])
                    for item in document["files"]
                }
                if entry == "candidate":
                    self.release("1.0.20", "3" * 40)
                else:
                    self.release("1.0.19", "1" * 40)
                private_bin = self.root / "paired-removal-python"
                private_bin.mkdir()
                shim_path = private_bin / "shim.py"
                shim_path.write_text(shim)
                executable = private_bin / "python3"
                executable.write_text(wrapper)
                executable.chmod(0o755)
                self.env["PATH"] = str(private_bin) + os.pathsep + self.env["PATH"]
                self.env["FAKE_PAIRED_REMOVAL_SHIM"] = str(shim_path)
                result = self.command(
                    INSTALLER, "--update", "--migrate-recorder-override"
                )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                marker = json.loads((self.home / "paired-removal-fault.json").read_text())
                self.assertEqual(marker["phase"], "prepared")
                self.assertTrue(marker["operation_id"])
                self.assertEqual(
                    bytes.fromhex(marker["rest_bytes"]), self.rest_override_bytes
                )
                self.assertNotIn("command not found", result.stderr)
                self.assertNotIn("NameError", result.stderr)
                self.assertEqual(self.current().resolve(strict=True), generation)
                self.assertEqual(
                    (generation / "manifest.json").read_bytes(), manifest_before
                )
                self.assertEqual({
                    item["path"]: digest(generation / item["path"])
                    for item in document["files"]
                }, members_before)
                self.assertTrue(self.rest_override.is_file())
                self.preserved_profile()
                print("Observed actual paired removal I/O failure:", entry)
                self.assertTrue(
                    self.override.is_file(),
                    "ordinary paired removal failure did not restore Recorder configuration",
                )
                self.assert_incident_pair_prestate(before)
                self.assertEqual(control_path.read_bytes(), control_before)
                restored = json.loads(
                    (self.home / ".local/share/codex-info/install-transaction.json").read_text()
                )
                self.assertEqual(restored["operation_id"], marker["operation_id"])
                self.assertEqual(restored["phase"], "committed")
                self.assertEqual(restored["old_generation"], generation.name)

    def test_publication_failures_finish_the_verified_rollback(self):
        shim = r"""
import errno, json, os, pathlib, sys
assert sys.argv[1] == "-"
sys.argv = ["-", *sys.argv[2:]]
publication_program = sys.stdin.read()
publication_original_replace = os.replace
publication_share = pathlib.Path(os.environ["HOME"]) / ".local/share/codex-info"
publication_marker = pathlib.Path(os.environ["HOME"]) / "publication-fault.json"
publication_journal = publication_share / "install-transaction.json"
publication_kind = os.environ["FAKE_PUBLICATION_KIND"]
def publication_fail(source, destination, phase):
    publication_marker.write_text(json.dumps({
        "kind": publication_kind, "phase": phase,
        "operation_id": json.loads(publication_journal.read_text())["operation_id"],
        "source": str(source), "destination": str(destination),
    }))
    raise OSError(errno.EIO, "controlled publication I/O failure")
def publication_replace(source, destination):
    destination_path = pathlib.Path(destination)
    stage_publication = (
        destination_path.parent == publication_share / "generations"
        and destination_path.name.startswith("1.0.20-")
        and pathlib.Path(source).name.startswith(".candidate.")
    )
    if not publication_marker.exists() and stage_publication:
        phase = json.loads(publication_journal.read_text())["phase"]
        if publication_kind == "publish-io":
            publication_fail(source, destination, phase)
        if publication_kind == "publish-integrity":
            result = publication_original_replace(source, destination)
            payload = destination_path / "codex_info_rest"
            original = payload.read_bytes()
            assert original
            payload.write_bytes(bytes([original[0] ^ 1]) + original[1:])
            publication_marker.write_text(json.dumps({
                "kind": publication_kind, "phase": phase,
        "operation_id": json.loads(publication_journal.read_text())["operation_id"],
                "source": str(source), "destination": str(destination),
            }))
            return result
    if (
        not publication_marker.exists()
        and publication_kind == "journal-io"
        and destination_path == publication_journal
    ):
        pending = json.loads(pathlib.Path(source).read_text())
        if pending["phase"] == "current_switched":
            phase = json.loads(publication_journal.read_text())["phase"]
            publication_fail(source, destination, phase)
    return publication_original_replace(source, destination)
os.replace = publication_replace
exec(compile(publication_program, "actual-installer-inline-python", "exec"))
"""
        wrapper = """#!/bin/bash
if [[ "${1:-}" == -B ]]; then shift; fi
if [[ "${1:-}" == - ]]; then
    exec /usr/bin/python3 -B "$FAKE_PUBLICATION_SHIM" "$@"
fi
exec /usr/bin/python3 -B "$@"
"""
        def runtime_intent():
            result = {}
            for unit in (
                "codex-info-recorder.service",
                "codex-info-rest.service",
                "codex-info-update.timer",
            ):
                for query in ("is-active", "is-enabled"):
                    observed = subprocess.run(  # noqa: S603 -- test-owned fake systemctl and literal query/unit arguments.
                        [str(self.bin / "systemctl"), "--user", query, "--quiet", unit],
                        env=self.env, capture_output=True, text=True,
                        check=False, timeout=5,
                    )
                    allowed = {0, 3, 4} if query == "is-active" else {0, 1, 4}
                    self.assertIn(
                        observed.returncode, allowed,
                        observed.stdout + observed.stderr,
                    )
                    result[(unit, query)] = observed.returncode
            return result

        for index, kind in enumerate(("publish-io", "publish-integrity", "journal-io")):
            with self.subTest(kind=kind):
                if index:
                    self.doCleanups()
                    self.setUp()
                original = self.current().resolve(strict=True)
                manifest_before = (original / "manifest.json").read_bytes()
                document = json.loads(manifest_before)
                members_before = {
                    entry["path"]: digest(original / entry["path"])
                    for entry in document["files"]
                }
                normal = self.command(self.installed, "--status")
                self.assertEqual(normal.returncode, 0, normal.stdout + normal.stderr)
                intent_before = runtime_intent()
                control_path = self.home / ".local/share/codex-info/control-state.json"
                control_before = control_path.read_bytes()
                self.release("1.0.20", "3" * 40)
                private_bin = self.root / "publication-python"
                private_bin.mkdir()
                shim_path = private_bin / "shim.py"
                shim_path.write_text(shim)
                executable = private_bin / "python3"
                executable.write_text(wrapper)
                executable.chmod(0o755)
                self.env["PATH"] = str(private_bin) + os.pathsep + self.env["PATH"]
                self.env["FAKE_PUBLICATION_SHIM"] = str(shim_path)
                self.env["FAKE_PUBLICATION_KIND"] = kind
                result = self.command(self.installed, "--update")
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                marker = json.loads((self.home / "publication-fault.json").read_text())
                self.assertEqual(marker["kind"], kind)
                self.assertEqual(
                    marker["phase"],
                    "candidate_published" if kind == "journal-io" else "entrypoints_linked",
                )
                self.assertNotIn("command not found", result.stderr)
                self.assertEqual((original / "manifest.json").read_bytes(), manifest_before)
                self.assertEqual({
                    entry["path"]: digest(original / entry["path"])
                    for entry in document["files"]
                }, members_before)
                self.preserved_profile()
                print("Observed real publication failure:", kind, marker["phase"])
                self.assertEqual(
                    self.current().resolve(strict=True), original,
                    "ordinary publication failure changed the committed generation",
                )
                journal = json.loads(
                    (self.home / ".local/share/codex-info/install-transaction.json").read_text()
                )
                self.assertEqual(
                    journal["phase"], "committed",
                    "ordinary publication failure left the transaction unsettled",
                )
                self.assertEqual(journal["old_generation"], original.name)
                self.assertTrue(marker["operation_id"])
                self.assertEqual(journal["operation_id"], marker["operation_id"])
                self.assertEqual(runtime_intent(), intent_before)
                self.assertEqual(control_path.read_bytes(), control_before)
                coherent = self.command(self.installed, "--status")
                self.assertEqual(coherent.returncode, 0, coherent.stdout + coherent.stderr)

    def test_current_readiness_accepts_v4_schema(self):
        current = {
            "api_version": "v4", "state": "ready", "observed_at": 1792228061,
            "authenticated": True, "plan_label": None,
            "quota": {"remaining_percent": 88.0, "reset_at": 1792823932,
                      "window_seconds": 604800, "monthly": False},
            "models_status": "partial",
            "models": [{"model": "gpt-6.1-sol", "total_tokens": 15,
                        "input_tokens": 12, "cached_input_tokens": 3,
                        "cache_write_input_tokens": 0, "output_tokens": 3,
                        "estimated_cost": None}],
            "active_thread_count": 0, "open_session_thread_count": 0,
        }
        self.env.update(FAKE_CURRENT_STATUS="200", FAKE_CURRENT_BODY=json.dumps(current),
                        FAKE_REJECT_LEGACY_DETAILS="1")
        result = self.current_readiness()
        log = pathlib.Path(self.env["FAKE_LOG"]).read_text()
        self.assertIn("current-v4 200", log, result.stdout + result.stderr + log)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("rejected-legacy-details", log)
        self.preserved_profile()

    def acquisition_error_response(self, observed_at=1792228061):
        return {
            "api_version": "v4", "state": "error", "observed_at": observed_at,
            "authenticated": True, "plan_label": None, "quota": None,
            "models_status": "unavailable", "models": [],
            "active_thread_count": 0, "open_session_thread_count": 0,
        }


    def use_acquisition_error_response(self, observed_at=1792228061):
        self.env.update(
            FAKE_CURRENT_STATUS="200",
            FAKE_CURRENT_BODY=json.dumps(self.acquisition_error_response(observed_at)),
            FAKE_REJECT_LEGACY_DETAILS="1",
        )


    def test_current_readiness_accepts_acquisition_error_without_rewriting_it(self):
        for observed_at in (1792228061, None):
            with self.subTest(observed_at=observed_at):
                self.use_acquisition_error_response(observed_at)
                before = self.env["FAKE_CURRENT_BODY"]
                result = self.current_readiness()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(self.env["FAKE_CURRENT_BODY"], before)
                self.assertNotIn("rejected-legacy-details", pathlib.Path(self.env["FAKE_LOG"]).read_text())
        self.preserved_profile()


    def test_current_readiness_rejects_invalid_error_snapshots(self):
        invalid = (
            {"state": "initializing"}, {"state": "unknown"}, {"observed_at": -1},
            {"observed_at": True}, {"authenticated": "true"},
            {"models": [{"model": "fabricated"}]}, {"unexpected": "field"},
        )
        for changes in invalid:
            with self.subTest(changes=changes):
                current = self.acquisition_error_response()
                current.update(changes)
                self.env.update(FAKE_CURRENT_STATUS="200", FAKE_CURRENT_BODY=json.dumps(current),
                                FAKE_REJECT_LEGACY_DETAILS="1")
                result = self.current_readiness()
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.preserved_profile()


    def test_current_failure_does_not_fall_back_to_legacy_details(self):
        self.env.update(FAKE_CURRENT_STATUS="503", FAKE_CURRENT_BODY='{"error":"snapshot_unavailable"}')
        before = pathlib.Path(self.env["FAKE_LOG"]).read_text()
        result = self.current_readiness()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        log = pathlib.Path(self.env["FAKE_LOG"]).read_text()[len(before):]
        self.assertIn("current-v4 503", log)
        self.assertNotIn("/v1/details", log)
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

    def test_paired_override_migration_restores_ui_entrypoint(self):
        self.incident_override_pair()
        db, db_before, rows_before = self.incident_database()
        before = self.incident_pair_prestate(db, db_before, rows_before)
        self.release("1.0.20", "2" * 40, ui_bytes=(
            b'#!/bin/sh\n: > "$HOME/ui-started"\n'
            b'printf \'READY\\n\' > "$CODEX_INFO_UI_STARTUP_FIFO"\n'
        ))

        launcher = self.home / ".local/bin/codex-info"
        failed_ui = self.command(launcher, "--ui")
        self.assertNotEqual(failed_ui.returncode, 0)
        self.assertIn(
            "incident Recorder and REST overrides require explicit "
            "--update --migrate-recorder-override; configuration preserved",
            failed_ui.stderr,
        )
        self.assertIn("E_UI_SOURCE_UNAVAILABLE", failed_ui.stderr)
        self.assertEqual(self.override.read_bytes(), self.override_bytes)
        self.assertEqual(self.rest_override.read_bytes(), self.rest_override_bytes)

        migrated = self.command(INSTALLER, "--update", "--migrate-recorder-override")
        self.assertEqual(migrated.returncode, 0, migrated.stdout + migrated.stderr)
        generation = self.current().resolve(strict=True)
        self.assertNotEqual(generation, before["generation"])
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), generation / "codex_info_recorder")
        self.assertEqual((self.proc / str(REST_PID) / "exe").resolve(strict=True), generation / "codex_info_rest")
        backups = self.home / ".local/share/codex-info/legacy-backups"
        for service, override, original in (
            ("recorder", self.override, self.override_bytes),
            ("rest", self.rest_override, self.rest_override_bytes),
        ):
            self.assertFalse(override.exists())
            saved = list(backups.glob(f"*-{service}-604-hotfix.conf"))
            self.assertEqual(len(saved), 1)
            self.assertEqual((saved[0].read_bytes(), saved[0].stat().st_mode & 0o777), (original, 0o600))

        launched = self.command(launcher, "--ui")
        self.assertEqual(launched.returncode, 0, launched.stdout + launched.stderr)
        self.assertTrue((self.home / "ui-started").is_file(), "verified UI payload reached")
        self.assertEqual(db.read_bytes(), db_before)
        with sqlite3.connect(db) as connection:
            self.assertEqual(connection.execute("SELECT observed_at, tokens FROM preserved_usage").fetchall(), rows_before)

    def test_paired_override_failures_restore_exact_prestate(self):
        cases = (
            ("recorder-start", "start", "codex-info-recorder.service"),
            ("rest-start", "start", "codex-info-rest.service"),
            ("recorder-readiness", "readiness", "recorder"),
            ("rest-readiness", "readiness", "rest"),
        )
        for index, (name, failure, target) in enumerate(cases):
            with self.subTest(failure=name):
                if index:
                    self.doCleanups()
                    self.setUp()
                self.incident_override_pair()
                db, db_bytes, rows = self.incident_database()
                before = self.incident_pair_prestate(db, db_bytes, rows)
                self.release("1.0.20", "2" * 40)
                if failure == "start":
                    self.env["FAKE_FAIL_START_UNIT"] = target
                    self.env["FAKE_FAIL_START_ONCE_FILE"] = str(self.root / f"{name}-once")
                else:
                    self.env["CODEX_INFO_DEADLINE"] = str(int(time.time()) + 75)
                    if target == "recorder":
                        self.env["FAKE_RECORDER_FAIL_READY_VERSION"] = "1.0.20"
                        readiness_marker = self.root / "recorder-readiness-injection.json"
                        self.env["FAKE_RECORDER_READINESS_MARKER"] = str(readiness_marker)
                    else:
                        self.env["FAKE_HEALTH_FAIL_VERSION"] = "1.0.20"
                result = self.command(INSTALLER, "--update", "--migrate-recorder-override")
                if failure == "readiness" and target == "recorder":
                    self.assertEqual(
                        json.loads(readiness_marker.read_text()),
                        {"version": "1.0.20", "injection": "recorder-readiness"},
                    )
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn("previous generation restored", result.stderr)
                self.assert_incident_pair_prestate(before)
                journal = json.loads((self.home / ".local/share/codex-info/install-transaction.json").read_text())
                self.assertEqual(journal["phase"], "committed")

    def test_paired_override_failure_preserves_inactive_recorder(self):
        self.incident_override_pair()
        pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]).unlink()
        self.assertFalse(pathlib.Path(self.env["FAKE_MAIN_ACTIVE_FILE"]).exists())
        self.assertTrue(pathlib.Path(self.env["FAKE_REST_ACTIVE_FILE"]).is_file())
        db, db_bytes, rows = self.incident_database()
        before = self.incident_pair_prestate(db, db_bytes, rows)
        self.assertEqual(before["active"], (False, True))
        self.release("1.0.20", "2" * 40)
        self.env["FAKE_FAIL_START_UNIT"] = "codex-info-rest.service"
        fail_once = self.root / "inactive-recorder-rest-start-failed-once"
        self.env["FAKE_FAIL_START_ONCE_FILE"] = str(fail_once)

        result = self.command(INSTALLER, "--update", "--migrate-recorder-override")
        self.assertTrue(fail_once.is_file(), "new-generation REST start failure injection did not fire")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_incident_pair_prestate(before)
        self.assertIn("previous generation restored", result.stderr)
        journal = json.loads((self.home / ".local/share/codex-info/install-transaction.json").read_text())
        self.assertEqual(journal["phase"], "committed")

    def test_paired_override_no_update_migrates_both_services(self):
        self.incident_override_pair()
        db, db_bytes, rows = self.incident_database()
        before = self.incident_pair_prestate(db, db_bytes, rows)
        self.release("1.0.19", "1" * 40)

        result = self.command(INSTALLER, "--update", "--migrate-recorder-override")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        generation = self.current().resolve(strict=True)
        self.assertEqual(generation, before["generation"])
        self.assertEqual((self.proc / str(RECORDER_PID) / "exe").resolve(strict=True), generation / "codex_info_recorder")
        self.assertEqual((self.proc / str(REST_PID) / "exe").resolve(strict=True), generation / "codex_info_rest")
        self.assertFalse(self.override.exists())
        self.assertFalse(self.rest_override.exists())
        backups = self.home / ".local/share/codex-info/legacy-backups"
        for service, expected in (("recorder", self.override_bytes), ("rest", self.rest_override_bytes)):
            saved = list(backups.glob(f"*-{service}-604-hotfix.conf"))
            self.assertEqual(len(saved), 1)
            self.assertEqual((saved[0].read_bytes(), saved[0].stat().st_mode & 0o777), (expected, 0o600))
        for path, expected in before["payloads"].items():
            metadata = path.stat()
            self.assertEqual((metadata.st_dev, metadata.st_ino, digest(path), path.read_bytes()), expected)
        self.assertEqual(db.read_bytes(), db_bytes)
        with sqlite3.connect(db) as connection:
            self.assertEqual(connection.execute("SELECT observed_at, tokens FROM preserved_usage").fetchall(), rows)
        self.preserved_profile()

    def test_paired_override_rejects_unsupported_rest_before_mutation(self):
        self.incident_override_pair()
        self.rest_override.write_bytes(self.rest_override_bytes + b"Environment=USER_SETTING=preserve\n")
        self.rest_override.chmod(0o600)
        db, db_bytes, rows = self.incident_database()
        before = self.incident_pair_prestate(db, db_bytes, rows)
        self.release("1.0.20", "2" * 40)
        journal_path = self.home / ".local/share/codex-info/install-transaction.json"
        journal_before = journal_path.read_bytes()
        commands_before = pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_bytes()

        result = self.command(INSTALLER, "--update", "--migrate-recorder-override")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rest drop-in contains unsupported settings", result.stderr)
        self.assert_incident_pair_prestate(before)
        self.assertEqual(journal_path.read_bytes(), journal_before)
        self.assertEqual(pathlib.Path(self.env["FAKE_EFFECTIVE_LOG"]).read_bytes(), commands_before)

    def test_paired_override_interrupted_backup_restores_both(self):
        self.incident_override_pair()
        db, db_bytes, rows = self.incident_database()
        before = self.incident_pair_prestate(db, db_bytes, rows)
        self.release("1.0.20", "2" * 40)
        self.env["CODEX_INFO_INTERRUPT_PHASE"] = "incident_recorder_backed_up"

        interrupted = self.command(INSTALLER, "--update", "--migrate-recorder-override")
        self.assertNotEqual(interrupted.returncode, 0, interrupted.stdout + interrupted.stderr)
        self.assertFalse(self.override.exists())
        self.assertTrue(self.rest_override.exists())
        journal = json.loads((self.home / ".local/share/codex-info/install-transaction.json").read_text())
        self.assertEqual(journal["phase"], "prepared")
        self.env.pop("CODEX_INFO_INTERRUPT_PHASE")

        resumed = self.command(INSTALLER, "--update")
        self.assertNotEqual(resumed.returncode, 0, resumed.stdout + resumed.stderr)
        self.assertIn("explicit --update --migrate-recorder-override", resumed.stderr)
        self.assert_incident_pair_prestate(before)
        journal = json.loads((self.home / ".local/share/codex-info/install-transaction.json").read_text())
        self.assertEqual(journal["phase"], "committed")

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



class RecorderTimestampBoundaryTests(unittest.TestCase):
    NOW = 1793806800

    def verify_offset(self, field, offset):
        functions = re.findall(
            r"(?ms)^recorder_identity_check\(\) \{\n.*?^\}\n",
            INSTALLER.read_text(),
        )
        self.assertEqual(len(functions), 1)
        with tempfile.TemporaryDirectory(prefix="recorder-utc-boundary-") as directory:
            root = pathlib.Path(directory)
            history = root / "data/history"
            history.mkdir(parents=True)
            process = root / "proc/70"
            process.mkdir(parents=True)
            payload = root / "recorder"
            payload.write_bytes(b"independent recorder identity fixture\n")
            payload.chmod(0o755)
            metadata = payload.stat()
            (process / "exe").symlink_to(payload)
            fields = ["S"] + ["0"] * 18 + ["42"]
            (process / "stat").write_text("70 (recorder) " + " ".join(fields) + "\n")
            lock = history / "usage_record_daemon.lock"
            lock.write_text(json.dumps({
                "pid": 70,
                "started_at": self.NOW - 20,
                "starttime_ticks": 42,
                "executable_device": metadata.st_dev,
                "executable_inode": metadata.st_ino,
                "owner_nonce": "b" * 32,
            }))
            state_path = history / "recorder-state.json"
            state = {
                "schema": "codex-info-recorder-state-v1",
                "pid": 70,
                "process_starttime": 42,
                "owner_nonce": "b" * 32,
                "write_state": "ready",
                "partition_id_hash": "c" * 64,
                "data_generation": 1,
                "collector_epoch": "d" * 32,
                "cycle_seq": 1,
                "last_commit_unix": self.NOW,
                "updated_at_unix": self.NOW,
            }
            state[field] = self.NOW + offset
            state_path.write_text(json.dumps(state))
            lock.chmod(0o600)
            state_path.chmod(0o600)
            before = (lock.read_bytes(), state_path.read_bytes())
            shim = root / "clock-shim.py"
            shim.write_text(
                "import os, sys, time\n"
                "script = sys.stdin.read()\n"
                "sys.argv = sys.argv[1:]\n"
                "time.time = lambda: int(os.environ['FIXTURE_NOW'])\n"
                "exec(compile(script, '<recorder-state-input>', 'exec'), "
                "{'__name__': '__main__'})\n"
            )
            prefix = r"""
set -euo pipefail
home_dir="$FIXTURE_ROOT"
proc_root="$FIXTURE_ROOT/proc"
python3() { /usr/bin/python3 "$FIXTURE_ROOT/clock-shim.py" "$@"; }
"""
            result = subprocess.run(
                ["/bin/bash", "--noprofile", "--norc", "-s"],
                input=prefix + functions[0]
                    + "\nrecorder_identity_check 70 1.0.170 source manifest\n",
                env={
                    "PATH": "/usr/bin:/bin",
                    "LC_ALL": "C",
                    "HOME": str(root),
                    "FIXTURE_ROOT": str(root),
                    "FIXTURE_NOW": str(self.NOW),
                    "CODEX_INFO_DATA_DIR": str(root / "data"),
                },
                capture_output=True,
                text=True,
                check=False,
                timeout=3,
            )
            self.assertEqual((lock.read_bytes(), state_path.read_bytes()), before)
            return result

    def boundary(self, field):
        for offset, expected in ((0, 0), (-150, 0), (1, 1), (5, 1), (-151, 1)):
            with self.subTest(field=field, offset=offset):
                result = self.verify_offset(field, offset)
                self.assertEqual(
                    result.returncode, expected,
                    result.stdout + result.stderr,
                )

    def test_heartbeat_uses_exact_utc_freshness(self):
        self.boundary("updated_at_unix")

    def test_commit_uses_exact_utc_freshness(self):
        self.boundary("last_commit_unix")


class ListenerObservationBoundaryTests(unittest.TestCase):
    def observe(self, function, observation, active=False):
        matches = re.findall(
            r"(?ms)^" + re.escape(function) + r"\(\) \{\n.*?^\}\n",
            INSTALLER.read_text(),
        )
        self.assertEqual(len(matches), 1)
        with tempfile.TemporaryDirectory(prefix="listener-observation-") as directory:
            root = pathlib.Path(directory)
            generation = "1.0.170-" + "a" * 40 + "-" + "b" * 64
            payload_root = root / "share/generations" / generation
            payload_root.mkdir(parents=True)
            rest = payload_root / "codex_info_rest"
            rest.write_bytes(b"independent verified REST member fixture\n")
            rest.chmod(0o755)
            (root / "share/current").symlink_to(payload_root)
            process = root / "proc/70"
            process.mkdir(parents=True)
            (process / "exe").symlink_to(rest)
            net = root / "proc/net"
            net.mkdir()
            header = (
                "  sl  local_address rem_address st tx_queue rx_queue "
                "tr tm->when retrnsmt uid timeout inode\n"
            )
            if observation == "empty_table":
                (net / "tcp").write_text(header)
            elif observation == "unowned_socket":
                (net / "tcp").write_text(
                    header
                    + " 0: 0100007F:2253 00000000:0000 0A "
                    + "00000000:00000000 00:00000000 00000000 1000 0 599 1\n"
                )
            profile = root / "profile-original"
            profile.write_bytes(b"preserve existing profile bytes\n")
            original = profile.read_bytes()
            prefix = r"""
set -euo pipefail
home_dir="$HOME"
share_dir="$HOME/share"
generations_dir="$share_dir/generations"
current_link="$share_dir/current"
manifest_destination="$share_dir/manifest.json"
proc_root="$HOME/proc"

# These are independently established caller preconditions. This fixture
# changes only listener observation, not generation or artifact validity.
verify_fixed_links_local() { return 0; }
verify_generation_files() { return 0; }
current_generation() { printf '%s\n' "$FIXTURE_GENERATION"; }
manifest_record() {
    printf '1.0.170\t%s\t%s\t%s\n' "$FIXTURE_SOURCE" "$FIXTURE_MANIFEST" "$FIXTURE_SHA";
}
manifest_rest_hash() { printf '%s\n' "$FIXTURE_SHA"; }
probe_active() { [[ "$ACTIVE_REST" == 1 ]]; }
systemd_pid() { printf '70\n'; }
"""
            if function == "verify_ui_source":
                prefix += r"""
socket_pid() {
    case "$OBSERVATION" in
        absent) return 0 ;;
        managed) printf '70\n' ;;
        foreign) printf '71\n' ;;
        unknown) printf 'listener ownership cannot be inspected\n' >&2; return 2 ;;
        *) return 3 ;;
    esac
}
"""
            result = subprocess.run(
                ["/bin/bash", "--noprofile", "--norc", "-s"],
                input=prefix + matches[0] + "\n" + function + "\n",
                env={
                    "PATH": "/usr/bin:/bin",
                    "LC_ALL": "C",
                    "HOME": str(root),
                    "ACTIVE_REST": str(int(active)),
                    "OBSERVATION": observation,
                    "FIXTURE_GENERATION": generation,
                    "FIXTURE_SOURCE": "a" * 40,
                    "FIXTURE_MANIFEST": "b" * 64,
                    "FIXTURE_SHA": digest(rest),
                },
                capture_output=True, text=True, check=False, timeout=3,
            )
            self.assertEqual(profile.read_bytes(), original)
            self.assertEqual(
                (root / "share/current").resolve(strict=True), payload_root
            )
            self.assertEqual(
                rest.read_bytes(), b"independent verified REST member fixture\n"
            )
            return result

    def ui_boundary(self, active):
        absent = self.observe("verify_ui_source", "absent", active)
        self.assertEqual(absent.returncode, 0, absent.stdout + absent.stderr)
        if active:
            managed = self.observe("verify_ui_source", "managed", True)
            self.assertEqual(
                managed.returncode, 0, managed.stdout + managed.stderr
            )
        foreign = self.observe("verify_ui_source", "foreign", active)
        self.assertNotEqual(foreign.returncode, 0, foreign.stdout + foreign.stderr)
        unknown = self.observe("verify_ui_source", "unknown", active)
        self.assertNotEqual(unknown.returncode, 0, unknown.stdout + unknown.stderr)

    def test_inactive_rest_ui_rejects_unknown_listener(self):
        self.ui_boundary(False)

    def test_active_rest_ui_rejects_unknown_listener(self):
        self.ui_boundary(True)

    def test_missing_listener_table_is_unknown(self):
        empty = self.observe("socket_pid", "empty_table")
        self.assertEqual(empty.returncode, 0, empty.stdout + empty.stderr)
        self.assertEqual(empty.stdout.strip(), "")
        unowned = self.observe("socket_pid", "unowned_socket")
        self.assertNotEqual(
            unowned.returncode, 0, unowned.stdout + unowned.stderr
        )
        missing = self.observe("socket_pid", "missing_table")
        self.assertNotEqual(missing.returncode, 0, missing.stdout + missing.stderr)


class ListenerTerminalBoundaryTests(unittest.TestCase):
    def observe_terminal(self, function, observation):
        matches = re.findall(
            r"(?ms)^" + re.escape(function) + r"\(\) \{\n.*?^\}\n",
            INSTALLER.read_text(),
        )
        self.assertEqual(len(matches), 1)
        dependencies = ""
        source = INSTALLER.read_text()
        for name in ("cleanup_control_prestate", "generation_prune_failed"):
            definitions = re.findall(
                rf"(?ms)^{name}\(\) \{{\n.*?^\}}\n", source
            )
            self.assertEqual(len(definitions), 1)
            dependencies += definitions[0] + "\n"
        with tempfile.TemporaryDirectory(prefix="listener-terminal-") as directory:
            root = pathlib.Path(directory)
            (root / "backups").mkdir(mode=0o700)
            profile = root / "profile-original"
            profile.write_bytes(b"preserve existing DB, backup and Session owner\n")
            original = profile.read_bytes()
            (root / "transaction.json").write_text(json.dumps({
                "schema": "codex-info-install-transaction-v1",
                "operation_id": "fixture-operation",
                "owner_pid": 99,
                "owner_starttime": 42,
                "boot_id": "fixture-boot",
                "phase": "rollback_switched",
                "old_generation": "",
                "new_generation": "",
                "desired_state": "stopped",
                "updated_at_unix": 1000,
            }))
            if function == "verify_nonrunning_terminal":
                retained = root / "retained-generation"
                retained.mkdir()
                (root / "current").symlink_to(retained)
            prefix = r"""
set -euo pipefail
home_dir="$HOME"
share_dir="$HOME"
current_link="$HOME/current"
backup_dir="$HOME/backups"
transaction="$HOME/transaction.json"
legacy_recovery_journal="$HOME/no-handoff"
legacy_combined_enable_destination="$HOME/no-enable"
operation_id=fixture-operation
operation_deadline=2000
install_deadline=2000
ROLLBACK_TIMEOUT=60
update_stage=
desired_state=stopped
preserved_local_timer_state=paused
previous_combined=0
previous_flat=0
previous_id=
candidate_id=
main_active=0
rest_active=0
legacy_combined_generation=0
legacy_combined_enabled=0
legacy_combined_active=0
legacy_combined_prestate=verified-prestate
legacy_combined_manifest_hash=fixture-manifest
legacy_recovery_reader_hash=
journal_legacy_recovery_reader_hash=

# Other caller preconditions are fixed independently. Journal publication
# is observed by phase so an unknown input cannot falsely commit success.
safe_blocked() { printf 'SAFE_BLOCKED: %s\n' "$*" >&2; exit 1; }
probe_active() { return 1; }
probe_enabled() { return 0; }
probe_legacy_combined_enabled() { return 1; }
legacy_combined_retired() { return 0; }
legacy_combined_present() { return 0; }
verify_local_generation() { return 0; }
verify_fixed_links_local() { return 0; }
legacy_flat_record() { return 0; }
legacy_flat_present() { return 1; }
legacy_combined_record() {
    printf '1.0.170\tsource\tfixture-manifest\tdigest\t1\n';
}
require_user_manager() { return 0; }
now_unix() { printf '1000\n'; }
owner_starttime() { printf '42\n'; }
boot_id() { printf 'fixture-boot\n'; }
incident_override_pair_receipt() { printf '%s/no-receipt\n' "$backup_dir"; }
capture_runtime_state() { main_active=0; rest_active=0; }
recover_legacy_combined_state() {
    previous_combined=0
    legacy_recovery_reader_hash=
}
read_journal() {
    journal_phase=rollback_switched
    journal_operation_id=fixture-operation
    journal_candidate_id=
    journal_previous_id=
    journal_desired=stopped
    journal_legacy_recovery_reader_hash=
}
journal_owner_stale() { return 0; }
write_journal() { printf '%s\n' "$1" >> "$HOME/events"; }
update_log() { return 0; }
systemctl_user() { return 0; }
systemctl_stop_user() {
    printf 'unexpected-service-stop\n' >> "$HOME/events"; return 1;
}
atomic_unlink() { return 0; }
remove_published_entrypoints() { return 0; }
restore_backups() { return 0; }
ensure_entrypoints_for_generation() { return 0; }
restore_runtime_state() { return 0; }
backup_legacy_combined_unit() { printf 'backup\n' >> "$HOME/events"; }
legacy_combined_listener_matches() { return 1; }
retire_known_unmanaged() { printf 'signal\n' >> "$HOME/events"; return 1; }
rollback_transaction() { printf 'fallback\n' >> "$HOME/events"; return 1; }

socket_pid() {
    local observed=0
    if [[ -f "$HOME/probe-count" ]]; then
        read -r observed < "$HOME/probe-count"
    fi
    printf '%s\n' "$((observed + 1))" > "$HOME/probe-count"
    case "$OBSERVATION" in
        absent) return 0 ;;
        unknown) printf 'fixture observation unavailable\n' >&2; return 2 ;;
        second_unknown)
            if (( observed == 0 )); then return 0; fi
            printf 'fixture later observation unavailable\n' >&2
            return 2
            ;;
        *) return 3 ;;
    esac
}
"""
            invocation = {
                "verify_nonrunning_terminal": "verify_nonrunning_terminal stopped",
                "retire_legacy_combined": "retire_legacy_combined 0",
                "rollback_transaction": "rollback_transaction '' fixture",
            }.get(function, function)
            result = subprocess.run(
                ["/bin/bash", "--noprofile", "--norc", "-s"],
                input=prefix + dependencies + matches[0] + "\n" + invocation + "\n",
                env={
                    "PATH": "/usr/bin:/bin",
                    "LC_ALL": "C",
                    "HOME": str(root),
                    "OBSERVATION": observation,
                },
                capture_output=True, text=True, check=False, timeout=3,
            )
            self.assertEqual(profile.read_bytes(), original)
            events = (
                (root / "events").read_text().splitlines()
                if (root / "events").exists() else []
            )
            self.assertNotIn("signal", events)
            self.assertNotIn("unexpected-service-stop", events)
            return result, events

    def require_absence(self, function):
        good, good_events = self.observe_terminal(function, "absent")
        self.assertEqual(good.returncode, 0, good.stdout + good.stderr)
        if function in {"rollback_transaction", "resume_transaction"}:
            self.assertIn("committed", good_events)
        bad, bad_events = self.observe_terminal(function, "unknown")
        self.assertNotEqual(bad.returncode, 0, bad.stdout + bad.stderr)
        self.assertNotIn("committed", bad_events)
        self.assertNotIn("backup", bad_events)

    def test_nonrunning_terminal_requires_observed_absence(self):
        self.require_absence("verify_nonrunning_terminal")

    def test_flat_terminal_requires_observed_absence(self):
        self.require_absence("verify_legacy_terminal")

    def test_combined_terminal_requires_observed_absence(self):
        self.require_absence("verify_legacy_combined_terminal")

    def test_retirement_requires_both_observations(self):
        good, events = self.observe_terminal("retire_legacy_combined", "absent")
        self.assertEqual(good.returncode, 0, good.stdout + good.stderr)
        self.assertIn("backup", events)
        for observation in ("unknown", "second_unknown"):
            with self.subTest(observation=observation):
                bad, events = self.observe_terminal(
                    "retire_legacy_combined", observation
                )
                self.assertNotEqual(bad.returncode, 0, bad.stdout + bad.stderr)
                self.assertNotIn("backup", events)
                self.assertNotIn("committed", events)

    def test_empty_rollback_does_not_commit_unknown(self):
        self.require_absence("rollback_transaction")

    def test_empty_resume_does_not_commit_unknown(self):
        self.require_absence("resume_transaction")


class CurrentModelIdentityTests(unittest.TestCase):
    def frame(self, keys):
        return {
            "api_version": "v4", "state": "error", "observed_at": 1792228061,
            "authenticated": True, "plan_label": None, "quota": None,
            "models_status": "partial",
            "models": [{
                "model": key, "total_tokens": 0, "input_tokens": 0,
                "cached_input_tokens": 0, "cache_write_input_tokens": None,
                "output_tokens": 0, "estimated_cost": None,
            } for key in keys],
            "active_thread_count": 0, "open_session_thread_count": 0,
        }

    def probe(self, document):
        source = INSTALLER.read_text()
        owner = source.index("health_readback() {\n")
        marker = "    python3 -c '\n"
        start = source.index(marker, owner) + len(marker)
        end = source.index(
            "\n' \"$current_version\" <<< \"$details\" || return 1", start
        )
        validator = source[start:end]
        return subprocess.run(  # noqa: S603 -- repository validator with fixture JSON passed only on stdin.
            [sys.executable, "-B", "-c", validator, "v4"],
            input=json.dumps(document), text=True, capture_output=True,
            check=False, timeout=5,
        )

    def test_valid_model_order_is_not_an_admission_condition(self):
        ascending = self.probe(self.frame(["model-a", "model-z"]))
        self.assertEqual(ascending.returncode, 0, ascending.stderr)
        duplicate = self.probe(self.frame(["model-a", "model-a"]))
        self.assertNotEqual(duplicate.returncode, 0, duplicate.stderr)
        descending = self.probe(self.frame(["model-z", "model-a"]))
        self.assertEqual(
            descending.returncode, 0,
            "valid unordered models were rejected: " + descending.stderr,
        )

    def test_model_key_scalar_control_and_bidi_domain(self):
        for keys in (
            ["x" * 127 + "\U0001f600"],
            ["e\u0301", "\u00e9"],
        ):
            result = self.probe(self.frame(keys))
            self.assertEqual(result.returncode, 0, result.stderr)
        empty = self.probe(self.frame([""]))
        self.assertNotEqual(empty.returncode, 0, empty.stderr)
        invalid = (
            ("too_long", "x" * 129),
            ("c0", "model\nx"),
            ("c1", "model\u0085x"),
            ("arabic_bidi", "model\u061cx"),
            ("lrm", "model\u200ex"),
            ("override", "model\u202ex"),
            ("isolate", "model\u2069x"),
            ("surrogate", "model\ud800x"),
        )
        admitted = []
        for name, key in invalid:
            result = self.probe(self.frame([key]))
            if result.returncode == 0:
                admitted.append(name)
        print("MODEL_KEY_MISADMISSIONS=" + json.dumps(admitted))
        self.assertEqual(admitted, [], "invalid model keys were admitted")

    def test_model_array_limit_and_exact_uniqueness(self):
        keys = [f"model-{index:04d}" for index in range(1025)]
        boundary = self.probe(self.frame(keys[:1024]))
        self.assertEqual(boundary.returncode, 0, boundary.stderr)
        duplicate = self.probe(self.frame([keys[0], keys[0]]))
        self.assertNotEqual(duplicate.returncode, 0, duplicate.stderr)
        overflow = self.probe(self.frame(keys))
        self.assertNotEqual(
            overflow.returncode, 0, "1025 models were admitted"
        )



class CurrentFrameDomainTests(unittest.TestCase):
    probe = CurrentModelIdentityTests.probe

    def base(self):
        return CurrentModelIdentityTests.frame(self, ["SOL"])

    @staticmethod
    def quota():
        return {
            "remaining_percent": 44.0, "reset_at": 1792823932,
            "window_seconds": 604800, "monthly": False,
        }

    def priced_frame(self):
        document = CurrentModelIdentityTests.frame(self, ["SOL"])
        document["models"][0].update({
            "total_tokens": 135888894,
            "input_tokens": 134789644,
            "cached_input_tokens": 130605312,
            "cache_write_input_tokens": 0,
            "output_tokens": 1099250,
            "estimated_cost": {
                "price_version": "LOCAL_ESTIMATE_V1_2026-08-14",
                "ordinary_input_dollars": 20.92166,
                "cached_input_dollars": 65.302656,
                "cache_write_input_dollars": 0.0,
                "output_dollars": 32.9775,
                "total_dollars": 119.20181600000001,
            },
        })
        return document

    def require_valid(self, document):
        result = self.probe(document)
        self.assertEqual(result.returncode, 0, result.stderr)

    def reject_all(self, label, cases):
        admitted = []
        for name, document in cases:
            result = self.probe(document)
            if result.returncode == 0:
                admitted.append(name)
        print(label + "=" + json.dumps(admitted))
        self.assertEqual(admitted, [], label)

    def test_price_value_types_and_finiteness(self):
        self.require_valid(self.priced_frame())
        unknown = self.base()
        self.require_valid(unknown)
        cases = []
        for name, key, value in (
            ("boolean_amount", "ordinary_input_dollars", True),
            ("string_amount", "cached_input_dollars", "0.1"),
            ("null_amount", "cache_write_input_dollars", None),
            ("negative_amount", "output_dollars", -1.0),
            ("nan_amount", "total_dollars", float("nan")),
            ("infinite_amount", "ordinary_input_dollars", float("inf")),
            ("unrepresentable_amount", "output_dollars", 10 ** 400),
        ):
            document = self.priced_frame()
            document["models"][0]["estimated_cost"][key] = value
            cases.append((name, document))
        for name, revision in (
            ("null_revision", None), ("boolean_revision", True),
            ("integer_revision", 42), ("empty_revision", ""),
        ):
            document = self.priced_frame()
            document["models"][0]["estimated_cost"]["price_version"] = revision
            cases.append((name, document))
        self.reject_all("PRICE_DOMAIN_MISADMISSIONS", cases)

    def test_common_clock_and_plan_domains(self):
        maximum = 253402300799
        document = self.base()
        document["observed_at"] = maximum
        self.require_valid(document)
        for label in (
            "無料", "Go", "Plus", "Pro", "Pro Lite", "Team",
            "Business", "エンタープライズ", "教育", "プラン未設定",
        ):
            document = self.base()
            document["plan_label"] = label
            document["quota"] = self.quota()
            document["quota"]["monthly"] = label == "エンタープライズ"
            document["quota"]["window_seconds"] = 17 * 86400
            self.require_valid(document)
        document = self.base()
        document["quota"] = self.quota()
        document["quota"]["reset_at"] = maximum
        self.require_valid(document)
        cases = []
        document = self.base()
        document["observed_at"] = maximum + 1
        cases.append(("observed_outside_wire_time", document))
        document = self.base()
        document["quota"] = self.quota()
        document["quota"]["reset_at"] = maximum + 1
        cases.append(("reset_outside_wire_time", document))
        document = self.base()
        document["plan_label"] = "enterprise"
        cases.append(("noncanonical_plan", document))
        for label, monthly in (
            ("Plus", True), ("エンタープライズ", False),
        ):
            document = self.base()
            document["plan_label"] = label
            document["quota"] = self.quota()
            document["quota"]["monthly"] = monthly
            cases.append(("monthly_mismatch_" + label, document))
        self.reject_all("COMMON_DOMAIN_MISADMISSIONS", cases)

    def boundary(self, state="error"):
        document = CurrentModelIdentityTests.frame(self, [])
        document.update(
            state=state, observed_at=None, authenticated=False,
            models_status="unavailable",
        )
        return document

    def test_account_boundary_is_strict_empty(self):
        self.require_valid(self.boundary())
        self.require_valid(self.boundary("auth_required"))
        known_error = self.priced_frame()
        known_error["observed_at"] = None
        self.require_valid(known_error)
        cases = []
        document = self.base()
        document.update(state="ready", authenticated=False)
        cases.append(("ready_without_auth", document))
        document = self.boundary("auth_required")
        document["authenticated"] = True
        cases.append(("auth_required_with_auth", document))
        changes = (
            ("boundary_observed", {"observed_at": 1792228061}),
            ("boundary_plan", {"plan_label": "Plus"}),
            ("boundary_quota", {"quota": self.quota()}),
            ("boundary_models", {
                "models": self.base()["models"], "models_status": "partial",
            }),
            ("boundary_counts", {
                "active_thread_count": 1, "open_session_thread_count": 1,
            }),
            ("boundary_quality", {"models_status": "partial"}),
        )
        for name, fields in changes:
            document = self.boundary()
            document.update(fields)
            cases.append((name, document))
        self.reject_all("AUTH_DOMAIN_MISADMISSIONS", cases)

    def test_window_is_positive_without_an_unsigned_cap(self):
        for value in (0, -1, True, 1.5):
            document = self.base()
            document["quota"] = self.quota()
            document["quota"]["window_seconds"] = value
            result = self.probe(document)
            self.assertNotEqual(result.returncode, 0, result.stderr)
        document = self.base()
        document["quota"] = self.quota()
        document["quota"]["window_seconds"] = 1
        self.require_valid(document)
        document["quota"]["window_seconds"] = 1 << 64
        result = self.probe(document)
        self.assertEqual(
            result.returncode, 0,
            "positive window was rejected by token-width cap: " + result.stderr,
        )

    def test_uint64_tokens_and_counts_keep_exact_boundaries(self):
        maximum = (1 << 64) - 1
        for values in (
            {
                "total_tokens": maximum, "input_tokens": maximum,
                "cached_input_tokens": maximum, "cache_write_input_tokens": 0,
                "output_tokens": 0,
            },
            {
                "total_tokens": maximum, "input_tokens": maximum - 1,
                "cached_input_tokens": 0, "cache_write_input_tokens": None,
                "output_tokens": 1,
            },
        ):
            document = self.base()
            document["models"][0].update(values)
            document["active_thread_count"] = maximum
            document["open_session_thread_count"] = maximum
            self.require_valid(document)
        document = self.base()
        document["models"][0].update(
            total_tokens=maximum, input_tokens=maximum,
            cached_input_tokens=0, cache_write_input_tokens=0, output_tokens=1,
        )
        result = self.probe(document)
        self.assertNotEqual(result.returncode, 0, result.stderr)
        document = self.base()
        document["open_session_thread_count"] = maximum + 1
        result = self.probe(document)
        self.assertNotEqual(result.returncode, 0, result.stderr)



class FixedLinkPrefixTests(unittest.TestCase):
    def probe(self, source, link, expected):
        text = source.read_text()
        body = text.split("fixed_link_matches() {\n", 1)[1].split("\n}\n", 1)[0]
        shell = (
            "set -eu\nfixed_link_matches() {\n" + body
            + "\n}\nfixed_link_matches \"$1\" \"$2\"\n"
        )
        return subprocess.run(  # noqa: S603 -- repository function and test-owned link paths passed as positional arguments.
            ["/bin/bash", "--noprofile", "--norc", "-c", shell,
             "fixed-link-fixture", str(link), str(expected)],
            text=True, capture_output=True, check=False, timeout=5,
        )

    def fixture(self, directory):
        home = pathlib.Path(directory)
        share = home / ".local/share/codex-info"
        binary = home / ".local/bin/codex_info"
        binary.parent.mkdir(parents=True)
        for generation in ("g1", "g2"):
            path = share / "generations" / generation
            path.mkdir(parents=True)
            (path / "codex_info").write_text(generation)
        current = share / "current"
        current.symlink_to("generations/g1")
        return share, current, binary


    def test_regular_member_tail_must_end_at_member(self):
        with tempfile.TemporaryDirectory() as directory:
            share, current, binary = self.fixture(directory)
            expected = "../share/codex-info/current/codex_info"
            for source in (INSTALLER, ROOT / "run.sh"):
                for tail in ("codex_info/", "codex_info/."):
                    with self.subTest(source=source.name, tail=tail):
                        binary.unlink(missing_ok=True)
                        binary.symlink_to(str(current) + "/./codex_info")
                        normal = self.probe(source, binary, expected)
                        self.assertEqual(normal.returncode, 0, normal.stderr)
                        binary.unlink()
                        binary.symlink_to(str(current) + "/" + tail)
                        self.assertFalse(binary.exists())
                        result = self.probe(source, binary, expected)
                        self.assertNotEqual(
                            result.returncode, 0,
                            "regular member tail was admitted",
                        )

    def test_missing_prefix_component_is_not_a_current_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            share, current, binary = self.fixture(directory)
            expected = "../share/codex-info/current/codex_info"
            for source in (INSTALLER, ROOT / "run.sh"):
                with self.subTest(source=source.name):
                    binary.unlink(missing_ok=True)
                    binary.symlink_to(expected)
                    normal = self.probe(source, binary, expected)
                    self.assertEqual(normal.returncode, 0, normal.stderr)
                    binary.unlink()
                    binary.symlink_to(str(share) + "/missing/../current/codex_info")
                    self.assertFalse((share / "missing").exists())
                    self.assertFalse(binary.exists())
                    rejected = self.probe(source, binary, expected)
                    self.assertNotEqual(
                        rejected.returncode, 0,
                        "unresolvable parent prefix was admitted",
                    )

    def test_valid_forms_keep_following_current_switch(self):
        with tempfile.TemporaryDirectory() as directory:
            share, current, binary = self.fixture(directory)
            expected = "../share/codex-info/current/codex_info"
            forms = (
                expected,
                str(current / "codex_info"),
                "../share/./codex-info/current/./codex_info",
            )
            for source in (INSTALLER, ROOT / "run.sh"):
                for target in forms:
                    with self.subTest(source=source.name, target=target):
                        current.unlink()
                        current.symlink_to("generations/g1")
                        binary.unlink(missing_ok=True)
                        binary.symlink_to(target)
                        normal = self.probe(source, binary, expected)
                        self.assertEqual(normal.returncode, 0, normal.stderr)
                        self.assertEqual(binary.read_text(), "g1")
                        current.unlink()
                        current.symlink_to("generations/g2")
                        switched = self.probe(source, binary, expected)
                        self.assertEqual(switched.returncode, 0, switched.stderr)
                        self.assertEqual(binary.read_text(), "g2")

    def test_parent_binding_can_be_prepared_before_current_publish(self):
        with tempfile.TemporaryDirectory() as directory:
            share, current, binary = self.fixture(directory)
            current.unlink()
            expected = "../share/codex-info/current/codex_info"
            binary.symlink_to(expected)
            self.assertFalse(binary.exists())
            for source in (INSTALLER, ROOT / "run.sh"):
                with self.subTest(source=source.name):
                    result = self.probe(source, binary, expected)
                    self.assertEqual(result.returncode, 0, result.stderr)

    def test_direct_generation_and_cancelled_current_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            share, current, binary = self.fixture(directory)
            expected = "../share/codex-info/current/codex_info"
            (share / "generations/current").symlink_to("g1")
            targets = (
                str(share / "generations/g1/codex_info"),
                str(share) + "/current/../current/codex_info",
                str(current / "different-member"),
            )
            for source in (INSTALLER, ROOT / "run.sh"):
                for target in targets:
                    with self.subTest(source=source.name, target=target):
                        binary.unlink(missing_ok=True)
                        binary.symlink_to(target)
                        if target != str(current / "different-member"):
                            self.assertEqual(
                                binary.resolve(strict=True),
                                (current / "codex_info").resolve(strict=True),
                            )
                        result = self.probe(source, binary, expected)
                        self.assertNotEqual(result.returncode, 0, result.stderr)



class FixedLinkRemainingConsumers(unittest.TestCase):
    MEMBERS = {
        "binary_destination": "codex_info",
        "recorder_binary_destination": "codex_info_recorder",
        "rest_binary_destination": "codex_info_rest",
        "launcher_destination": "run.sh",
        "installer_destination": "install.sh",
        "manifest_destination": "manifest.json",
        "unit_destination": "codex-info-recorder.service",
        "rest_unit_destination": "codex-info-rest.service",
        "update_service_destination": "codex-info-update.service",
        "update_timer_destination": "codex-info-update.timer",
    }

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        home = pathlib.Path(temporary.name)
        share = home / ".local/share/codex-info"
        binary = home / ".local/bin"
        libexec = home / ".local/libexec"
        units = home / ".config/systemd/user"
        backup = share / "legacy-backups"
        for directory in (binary, libexec, units, backup):
            directory.mkdir(parents=True, exist_ok=True)
        backup.chmod(0o700)
        for generation in ("g1", "g2", "g3"):
            directory = share / "generations" / generation
            directory.mkdir(parents=True)
            for member in set(self.MEMBERS.values()) | {"codex-info.service"}:
                (directory / member).write_text(generation + ":" + member)
        self.current = share / "current"
        self.current.symlink_to("generations/g1")
        self.paths = {
            "share_dir": share,
            "generations_dir": share / "generations",
            "current_link": self.current,
            "local_bin": binary,
            "local_libexec": libexec,
            "unit_dir": units,
            "backup_dir": backup,
            "binary_destination": binary / "codex_info",
            "recorder_binary_destination": binary / "codex_info_recorder",
            "rest_binary_destination": binary / "codex_info_rest",
            "launcher_destination": binary / "codex-info",
            "installer_destination": libexec / "codex-info-install.sh",
            "manifest_destination": share / "manifest.json",
            "unit_destination": units / "codex-info-recorder.service",
            "rest_unit_destination": units / "codex-info-rest.service",
            "update_service_destination": units / "codex-info-update.service",
            "update_timer_destination": units / "codex-info-update.timer",
            "legacy_combined_unit_destination": units / "codex-info.service",
            "legacy_combined_enable_destination": units / "default.target.wants/codex-info.service",
        }
        self.install_links()
        self.original_members = self.member_snapshot()

    def install_links(self):
        for variable, member in self.MEMBERS.items():
            path = self.paths[variable]
            path.unlink(missing_ok=True)
            path.symlink_to(self.current / member)

    def member_snapshot(self):
        return {
            str(path): digest(path)
            for path in self.paths["generations_dir"].rglob("*")
            if path.is_file()
        }

    def link_snapshot(self):
        return {
            name: os.readlink(self.paths[name])
            if self.paths[name].is_symlink() else None
            for name in self.MEMBERS
        }

    @staticmethod
    def function(source, name):
        start = source.index(name + "() {\n")
        return source[start:source.index("\n}\n", start) + 2] + "\n"

    def invoke(self, name, extra=""):
        source = INSTALLER.read_text()
        definitions = [
            self.function(source, "fixed_link_matches"),
            self.function(source, "atomic_symlink"),
            self.function(source, "atomic_unlink"),
            self.function(source, "validate_legacy_combined_enable_link"),
            self.function(source, name),
        ]
        assignments = "\n".join(
            f'{variable}="${{{index}}}"'
            for index, variable in enumerate(self.paths, 1)
        )
        shell = (
            "set -eu\n" + assignments + "\n"
            + "\n".join(definitions)
            + r"""
operation_id=entrypoint-fixture
previous_id=g1
candidate_id=g2
desired_state=removed
legacy_recovery_reader_hash=
legacy_combined_generation=1
legacy_combined_manifest_hash=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
safe_blocked() { echo "$*" >&2; exit 3; }
current_generation() { basename -- "$(readlink -f -- "$current_link")"; }
verify_generation_files() { [[ "$1" == "$generations_dir/g1" ]]; }
legacy_recovery_reader_active() { return 1; }
legacy_combined_record_at() {
    [[ "$#" == 6 ]] || return 1
    local path
    for path in "$@"; do
        [[ "$path" == "$backup_dir/$operation_id-"* && -f "$path" && ! -L "$path" ]] || return 1
    done
    printf '%s\t%s\t%s\t%s\t%s\n' 1.0.19 source "$legacy_combined_manifest_hash" binary rest
}
"""
            + extra + "\n" + name + "\n"
        )
        return subprocess.run(  # noqa: S603 -- repository functions, literal stubs, and test-owned positional paths.
            ["/bin/bash", "--noprofile", "--norc", "-c", shell,
             "fixed-consumer-fixture", *map(str, self.paths.values())],
            text=True, capture_output=True, check=False, timeout=10,
        )

    def test_removed_local_generation_accepts_absolute_payload_bindings(self):
        for name in (
            "unit_destination", "rest_unit_destination",
            "update_service_destination", "update_timer_destination",
        ):
            self.paths[name].unlink()
        result = self.invoke("verify_local_generation")
        self.assertEqual(result.returncode, 0, result.stderr)
        installer = self.paths["installer_destination"]
        installer.unlink()
        installer.symlink_to(self.paths["generations_dir"] / "g1/install.sh")
        before = self.link_snapshot()
        rejected = self.invoke("verify_local_generation")
        self.assertNotEqual(rejected.returncode, 0, rejected.stderr)
        self.assertEqual(self.link_snapshot(), before)
        self.assertEqual(self.member_snapshot(), self.original_members)

    def test_removed_entrypoint_publication_removes_absolute_unit_bindings(self):
        result = self.invoke("link_entrypoints")
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in (
            "unit_destination", "rest_unit_destination",
            "update_service_destination", "update_timer_destination",
        ):
            self.assertFalse(
                self.paths[name].exists() or self.paths[name].is_symlink()
            )
        for name, member in self.MEMBERS.items():
            if name in {
                "unit_destination", "rest_unit_destination",
                "update_service_destination", "update_timer_destination",
            }:
                continue
            self.assertEqual(
                self.paths[name].resolve(strict=True),
                (self.current / member).resolve(strict=True),
            )
        self.assertEqual(self.member_snapshot(), self.original_members)

    def test_rollback_removes_absolute_current_bindings_and_preserves_foreign(self):
        result = self.invoke("remove_published_entrypoints")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(all(
            not self.paths[name].exists() and not self.paths[name].is_symlink()
            for name in self.MEMBERS
        ))
        self.assertEqual(self.member_snapshot(), self.original_members)
        self.install_links()
        binary = self.paths["binary_destination"]
        binary.unlink()
        binary.symlink_to(self.paths["generations_dir"] / "g1/codex_info")
        before = self.link_snapshot()
        rejected = self.invoke("remove_published_entrypoints")
        self.assertNotEqual(rejected.returncode, 0, rejected.stderr)
        self.assertEqual(self.link_snapshot(), before)
        self.assertEqual(self.member_snapshot(), self.original_members)

    def test_legacy_installer_binding_preserves_exact_operation_generations(self):
        absolute = self.invoke("validate_legacy_recovery_installer_binding")
        self.assertEqual(absolute.returncode, 0, absolute.stderr)
        installer = self.paths["installer_destination"]
        installer.unlink()
        installer.symlink_to("../share/codex-info/current/install.sh")
        relative = self.invoke("validate_legacy_recovery_installer_binding")
        self.assertEqual(relative.returncode, 0, relative.stderr)
        self.current.unlink()
        self.current.symlink_to("generations/g3")
        before = self.link_snapshot()
        rejected = self.invoke("validate_legacy_recovery_installer_binding")
        self.assertNotEqual(rejected.returncode, 0, rejected.stderr)
        self.assertEqual(self.link_snapshot(), before)
        self.assertEqual(self.member_snapshot(), self.original_members)

    def test_legacy_flat_restore_accepts_absolute_bindings_and_preserves_outside(self):
        legacy_unit = self.paths["legacy_combined_unit_destination"]
        legacy_unit.symlink_to(self.current / "codex-info.service")
        variables = (
            "manifest_destination", "binary_destination", "installer_destination",
            "legacy_combined_unit_destination",
            "update_service_destination", "update_timer_destination",
        )
        saved = {}
        for variable in variables:
            destination = self.paths[variable]
            backup = self.paths["backup_dir"] / ("entrypoint-fixture-" + destination.name)
            data = ("saved:" + destination.name).encode()
            backup.write_bytes(data)
            backup.chmod(0o755 if variable in {
                "binary_destination", "installer_destination",
            } else 0o644)
            saved[backup] = data
        extra = "legacy_combined_generation=0\n"
        result = self.invoke("validate_legacy_combined_recovery", extra)
        self.assertEqual(result.returncode, 0, result.stderr)
        outside = self.paths["share_dir"].parent / "outside-payload"
        outside.write_text("external fixture bytes")
        binary = self.paths["binary_destination"]
        binary.unlink()
        binary.symlink_to(outside)
        before = self.link_snapshot()
        rejected = self.invoke("validate_legacy_combined_recovery", extra)
        self.assertNotEqual(rejected.returncode, 0, rejected.stderr)
        self.assertEqual(self.link_snapshot(), before)
        self.assertEqual(outside.read_text(), "external fixture bytes")
        self.assertEqual({path: path.read_bytes() for path in saved}, saved)
        self.assertEqual(self.member_snapshot(), self.original_members)



class LauncherStartupBoundaryTests(unittest.TestCase):
    ENV_PAYLOAD = b"""#!/usr/bin/python3
import json, os, pathlib
root = pathlib.Path(os.environ["HOME"])
keys = (
    "DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "WINIT_X11_SCALE_FACTOR",
    "LIBGL_ALWAYS_SOFTWARE", "MESA_LOADER_DRIVER_OVERRIDE",
    "CODEX_INFO_UI_CLIENT_ONLY",
)
(root / "ui-environment.json").write_text(
    json.dumps({key: os.environ.get(key) for key in keys})
)
with open(os.environ["CODEX_INFO_UI_STARTUP_FIFO"], "w") as ready:
    ready.write("READY\\n")
"""
    HANG_PAYLOAD = b"""#!/usr/bin/python3
import os, pathlib, signal, time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
pathlib.Path(os.environ["HOME"], "ignore-term-installed").touch()
while True:
    time.sleep(1)
"""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="launcher-boundary-")
        self.addCleanup(temporary.cleanup)
        self.home = pathlib.Path(temporary.name)
        self.share = self.home / ".local/share/codex-info"
        self.generation = self.share / "generations/g1"
        self.generation.mkdir(parents=True)
        current = self.share / "current"
        current.symlink_to("generations/g1")
        binary = self.home / ".local/bin"
        libexec = self.home / ".local/libexec"
        binary.mkdir(parents=True)
        libexec.mkdir(parents=True)
        self.payload = binary / "codex_info"
        self.payload.symlink_to("../share/codex-info/current/codex_info")
        self.launcher = binary / "codex-info"
        self.launcher.symlink_to("../share/codex-info/current/run.sh")
        installed = libexec / "codex-info-install.sh"
        installed.symlink_to("../share/codex-info/current/install.sh")
        (self.generation / "run.sh").write_bytes((ROOT / "run.sh").read_bytes())
        installer = self.generation / "install.sh"
        installer.write_text(
            "#!/bin/bash\n"
            'case "$1" in\n'
            '  --start) [[ "${FAKE_START_FAILURE:-0}" == 0 ]] ;;\n'
            "  --verify-ui) exit 0 ;;\n"
            "  *) exit 2 ;;\n"
            "esac\n"
        )
        installer.chmod(0o755)
        self.env = {
            "HOME": str(self.home), "PATH": "/usr/bin:/bin", "LC_ALL": "C",
            "DISPLAY": "fixture-display",
            "WAYLAND_DISPLAY": "fixture-wayland",
            "WAYLAND_SOCKET": "fixture-socket",
            "WINIT_X11_SCALE_FACTOR": "9.0",
            "LIBGL_ALWAYS_SOFTWARE": "0",
            "MESA_LOADER_DRIVER_OVERRIDE": "fixture-driver",
        }

    def payload_bytes(self, value):
        path = self.generation / "codex_info"
        path.write_bytes(value)
        path.chmod(0o755)

    def test_verified_fallback_keeps_the_normal_x11_environment(self):
        self.payload_bytes(self.ENV_PAYLOAD)
        normal = subprocess.run(  # noqa: S603 -- repository launcher copy with a test-owned HOME, payload, and service environment.
            ["/bin/bash", str(self.launcher), "--ui"], env=self.env,
            text=True, capture_output=True, check=False, timeout=45,
        )
        self.assertEqual(normal.returncode, 0, normal.stdout + normal.stderr)
        path = self.home / "ui-environment.json"
        expected = json.loads(path.read_text())
        self.assertEqual(expected, {
            "DISPLAY": "fixture-display",
            "WAYLAND_DISPLAY": None, "WAYLAND_SOCKET": None,
            "WINIT_X11_SCALE_FACTOR": None,
            "LIBGL_ALWAYS_SOFTWARE": "1",
            "MESA_LOADER_DRIVER_OVERRIDE": "llvmpipe",
            "CODEX_INFO_UI_CLIENT_ONLY": "1",
        })
        self.env["FAKE_START_FAILURE"] = "1"
        fallback = subprocess.run(  # noqa: S603 -- same test-owned launcher and environment with a literal failure switch.
            ["/bin/bash", str(self.launcher), "--ui"], env=self.env,
            text=True, capture_output=True, check=False, timeout=45,
        )
        self.assertEqual(fallback.returncode, 0, fallback.stdout + fallback.stderr)
        self.assertEqual(
            json.loads(path.read_text()), expected,
            "fallback UI backend environment changed",
        )

    def test_unconfirmed_child_is_reaped_before_startup_returns(self):
        self.payload_bytes(self.HANG_PAYLOAD)
        source = (ROOT / "run.sh").read_text()
        program = source.split("<<'PY_UI_STARTUP'\n", 1)[1].split("\nPY_UI_STARTUP", 1)[0]
        observation = self.home / "cleanup-observation.json"
        wrapper = r"""
import json, pathlib, sys, threading
program = sys.stdin.read()
scope = {"__name__": "__main__"}
status = 0
try:
    exec(compile(program, "actual-ui-startup-helper", "exec"), scope)
except SystemExit as error:
    status = error.code if isinstance(error.code, int) else 1
finally:
    child = scope.get("child")
    running = child is not None and child.poll() is None
    pathlib.Path(sys.argv[2]).write_text(json.dumps({"running_after_exit": running}))
    # The real helper has ended. Keep its child handle in this fixture
    # process so even the RED cannot leave an orphan behind.
    threading.excepthook = lambda args: None
    if child is not None:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)
raise SystemExit(status)
"""
        result = subprocess.run(  # noqa: S603 -- literal wrapper and test-owned payload/observation paths; repository program on stdin.
            [sys.executable, "-B", "-c", wrapper,
             str(self.payload), str(observation)],
            input=program, env=self.env, text=True,
            capture_output=True, check=False, timeout=45,
        )
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("E_UI_STARTUP_UNCONFIRMED", result.stderr)
        self.assertTrue((self.home / "ignore-term-installed").is_file())
        self.assertFalse(
            json.loads(observation.read_text())["running_after_exit"],
            "unconfirmed owned UI child remained running after helper exit",
        )



class PrivateImageCodecTests(unittest.TestCase):
    @staticmethod
    def function(name):
        source = INSTALLER.read_text()
        start = source.index(name + "() {\n")
        return source[start:source.index("\n}\n", start) + 2] + "\n"

    def encodings(self, data):
        import base64
        import gzip
        compressed = gzip.compress(data, mtime=0)
        if len(compressed) % 3 == 0:
            compressed += b"\0"
        canonical = base64.b64encode(compressed).decode("ascii")
        alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        index = len(canonical.rstrip("=")) - 1
        value = alphabet.index(canonical[index])
        equivalent = canonical[:index] + alphabet[value ^ 1] + canonical[index + 1:]
        self.assertNotEqual(equivalent, canonical)
        self.assertEqual(base64.b64decode(equivalent, validate=True), compressed)
        self.assertEqual(gzip.decompress(compressed), data)
        return canonical, equivalent

    def module_probe(self, image):
        shell = (
            'set -eu\nrunning_installer_source="$1"\n'
            'RECOVERY_PRODUCT_VERSION_B64="$2"\n'
            + self.function("version_python")
            + '\nprintf \'print("module-loaded")\\n\' | version_python\n'
        )
        return subprocess.run(  # noqa: S603 -- repository codec, literal Python input, and test-built image as a positional argument.
            ["/bin/bash", "--noprofile", "--norc", "-c", shell,
             "module-codec-fixture", str(INSTALLER), image],
            text=True, capture_output=True, check=False, timeout=5,
        )

    def journal_probe(self, image, expected_reader):
        with tempfile.TemporaryDirectory(prefix="reader-codec-") as directory:
            path = pathlib.Path(directory) / "pending.json"
            reader_hash = hashlib.sha256(expected_reader).hexdigest()
            document = {
                "schema": "codex-info-install-transaction-v1",
                "operation_id": "codec-fixture~lc1~000~" + "a" * 64 + "~" + reader_hash,
                "owner_pid": 999999, "owner_starttime": 1,
                "boot_id": "codec-fixture-boot", "phase": "prepared",
                "old_generation": "", "new_generation": "",
                "desired_state": "running", "updated_at_unix": 1,
                "legacy_reader_image": image,
            }
            path.write_text(json.dumps(document) + "\n")
            path.chmod(0o600)
            before = path.read_bytes()
            shell = (
                'set -eu\nrunning_installer_source="$1"\n'
                'RECOVERY_PRODUCT_VERSION_B64=\n'
                'transaction="$2"\nlegacy_recovery_journal="$2"\n'
                'safe_blocked() { echo "$*" >&2; exit 3; }\n'
                + self.function("version_python")
                + self.function("read_journal")
                + "\nread_journal\n"
            )
            result = subprocess.run(  # noqa: S603 -- repository journal reader and test-owned journal path as a positional argument.
                ["/bin/bash", "--noprofile", "--norc", "-c", shell,
                 "reader-codec-fixture", str(INSTALLER), str(path)],
                text=True, capture_output=True, check=False, timeout=5,
            )
            self.assertEqual(path.read_bytes(), before)
            return result

    def test_embedded_module_admission_uses_decoded_bytes(self):
        module = (ROOT / "scripts/product_version.py").read_bytes()
        canonical, equivalent = self.encodings(module)
        normal = self.module_probe(canonical)
        self.assertEqual(normal.returncode, 0, normal.stderr)
        self.assertEqual(normal.stdout.strip(), "module-loaded")
        accepted = self.module_probe(equivalent)
        self.assertEqual(
            accepted.returncode, 0,
            "equivalent embedded module encoding was rejected: " + accepted.stderr,
        )
        self.assertEqual(accepted.stdout.strip(), "module-loaded")

    def test_staged_reader_admission_uses_bound_decoded_bytes(self):
        reader = b"#!/bin/bash\n# literal same recovery reader bytes\n"
        canonical, equivalent = self.encodings(reader)
        normal = self.journal_probe(canonical, reader)
        self.assertEqual(normal.returncode, 0, normal.stderr)
        accepted = self.journal_probe(equivalent, reader)
        self.assertEqual(
            accepted.returncode, 0,
            "equivalent staged reader encoding was rejected: " + accepted.stderr,
        )

    def test_malformed_encoding_and_wrong_reader_identity_remain_rejected(self):
        module = self.module_probe("!")
        self.assertNotEqual(module.returncode, 0, module.stderr)
        reader = b"literal recovery reader bytes\n"
        canonical, equivalent = self.encodings(reader)
        malformed = self.journal_probe("!", reader)
        self.assertNotEqual(malformed.returncode, 0, malformed.stderr)
        mismatch = self.journal_probe(canonical, b"different reader bytes\n")
        self.assertNotEqual(mismatch.returncode, 0, mismatch.stderr)



class PrivateReceiptCodecTests(unittest.TestCase):
    function = staticmethod(PrivateImageCodecTests.function)

    def plain_alias(self, data):
        import base64
        original = base64.b64encode(data).decode("ascii")
        self.assertTrue(original.endswith("="))
        alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
        index = len(original.rstrip("=")) - 1
        alternate = (
            original[:index] + alphabet[alphabet.index(original[index]) ^ 1]
            + original[index + 1:]
        )
        self.assertEqual(base64.b64decode(alternate, validate=True), data)
        return alternate

    def pair_code(self):
        source = INSTALLER.read_text()
        start = source.index("<<'PY_INCIDENT_PAIR'\n") + len("<<'PY_INCIDENT_PAIR'\n")
        return source[start:source.index("\nPY_INCIDENT_PAIR\n", start)]

    def pair_command(self, fixture, action, receipt=True):
        args = list(fixture["args"])
        args[0] = action
        args[2] = str(fixture["receipt"]) if receipt else ""
        return subprocess.run(  # noqa: S603 -- repository pair code and arguments built solely by this temporary fixture.
            [sys.executable, "-B", "-c", self.pair_code(), *args],
            text=True, encoding="utf-8", capture_output=True,
            check=False, timeout=5,
        )

    def pair(self, directory):
        root = pathlib.Path(directory)
        generation = root / "generation"
        generation.mkdir(mode=0o700)
        backup = root / "backups"
        backup.mkdir(mode=0o700)
        payloads = root / "payloads"
        payloads.mkdir()
        overrides, executables, properties = {}, {}, {}
        for service, suffix in (("recorder", ""), ("rest", " --port 8787")):
            for padding in range(3):
                executable = payloads / (service + "-hotfix" + "x" * padding)
                data = (
                    "[Service]\nExecStart=\nExecStart=" + str(executable) + suffix + "\n"
                ).encode()
                if len(data) % 3:
                    break
            executable.write_bytes(b"owned inactive fixture executable\n")
            executable.chmod(0o700)
            override = root / (service + ".service.d") / "604-hotfix.conf"
            override.parent.mkdir()
            override.write_bytes(data)
            override.chmod(0o600)
            fragment = generation / ("codex-info-" + service + ".service")
            fragment.write_text("[Service]\n")
            properties[service] = (
                f"ExecStart={{ path={executable} ; argv[]={executable}{suffix} ; ignore_errors=no ; }}\n"
                f"FragmentPath={fragment}\nDropInPaths={override}\n"
            )
            overrides[service] = override
            executables[service] = executable
        fixture = {
            "receipt": root / "receipt.json", "backup": backup,
            "overrides": overrides, "executables": executables,
            "args": [
                "capture", "g1", "", "codec-operation",
                str(overrides["recorder"]), str(overrides["rest"]),
                str(generation), str(executables["recorder"]), str(executables["rest"]),
                properties["recorder"], properties["rest"],
                "0", "0", "", "0", "0", "0", "0",
                str(root / "unused-proc"), str(backup),
            ],
        }
        captured = self.pair_command(fixture, "capture", receipt=False)
        self.assertEqual(captured.returncode, 0, captured.stderr)
        fixture["document"] = json.loads(captured.stdout)
        self.save_receipt(fixture)
        return fixture

    @staticmethod
    def save_receipt(fixture):
        fixture["receipt"].write_text(json.dumps(fixture["document"]) + "\n")
        fixture["receipt"].chmod(0o600)

    def test_inspection_uses_exact_decoded_configuration_bytes(self):
        for service in ("recorder", "rest"):
            with self.subTest(service=service):
                with tempfile.TemporaryDirectory(prefix="pair-codec-") as directory:
                    fixture = self.pair(directory)
                    normal = self.pair_command(fixture, "inspect")
                    self.assertEqual(normal.returncode, 0, normal.stderr)
                    before = {
                        key: path.read_bytes()
                        for key, path in fixture["overrides"].items()
                    }
                    fixture["document"][service]["override_b64"] = self.plain_alias(
                        before[service]
                    )
                    self.save_receipt(fixture)
                    accepted = self.pair_command(fixture, "inspect")
                    self.assertEqual(
                        {key: path.read_bytes() for key, path in fixture["overrides"].items()},
                        before,
                    )
                    self.assertEqual(
                        accepted.returncode, 0,
                        "equivalent configuration receipt was rejected: " + accepted.stderr,
                    )

    def test_rest_removal_uses_saved_decoded_recorder_bytes(self):
        for alternate in (False, True):
            with self.subTest(alternate=alternate):
                with tempfile.TemporaryDirectory(prefix="pair-remove-codec-") as directory:
                    fixture = self.pair(directory)
                    original = fixture["overrides"]["recorder"].read_bytes()
                    snapshot = self.pair_command(fixture, "snapshot")
                    self.assertEqual(snapshot.returncode, 0, snapshot.stderr)
                    removed = self.pair_command(fixture, "remove-recorder")
                    self.assertEqual(removed.returncode, 0, removed.stderr)
                    if alternate:
                        fixture["document"]["recorder"]["override_b64"] = self.plain_alias(original)
                        self.save_receipt(fixture)
                    result = self.pair_command(fixture, "remove-rest")
                    self.assertEqual(
                        result.returncode, 0,
                        "equivalent saved recorder bytes were rejected: " + result.stderr,
                    )
                    self.assertFalse(fixture["overrides"]["rest"].exists())
                    self.assertEqual(
                        (fixture["backup"] / "codec-operation-recorder-604-hotfix.conf").read_bytes(),
                        original,
                    )

    def render(self, path):
        shell = (
            'set -eu\nrunning_installer_source="$1"\nreader_source="$2"\n'
            'RECOVERY_PRODUCT_VERSION_B64=\nlegacy_recovery_reader_hash=\n'
            + self.function("version_python")
            + self.function("legacy_recovery_reader_build")
            + '\nlegacy_recovery_reader_source() { printf \'%s\\n\' "$reader_source"; }\n'
            + "legacy_recovery_reader_build render\n"
        )
        return subprocess.run(  # noqa: S603 -- repository render function and test-owned reader path as positional arguments.
            ["/bin/bash", "--noprofile", "--norc", "-c", shell,
             "reader-render-fixture", str(INSTALLER), str(path)],
            text=True, encoding="utf-8", capture_output=True,
            check=False, timeout=5,
        )

    def reader(self, directory, module_image):
        source = INSTALLER.read_bytes()
        field = b"RECOVERY_PRODUCT_VERSION_B64=''"
        self.assertEqual(source.count(field), 1)
        value = source.replace(
            field, b"RECOVERY_PRODUCT_VERSION_B64='" + module_image + b"'", 1
        )
        path = pathlib.Path(directory) / "reader.sh"
        path.write_bytes(value)
        path.chmod(0o755)
        return path, value

    def test_render_preserves_equal_embedded_module_bytes(self):
        import base64
        import gzip
        module = (ROOT / "scripts/product_version.py").read_bytes()
        canonical = base64.b64encode(gzip.compress(module, mtime=0))
        alternate = base64.b64encode(gzip.compress(module, mtime=1))
        self.assertNotEqual(canonical, alternate)
        self.assertEqual(gzip.decompress(base64.b64decode(alternate)), module)
        with tempfile.TemporaryDirectory(prefix="reader-render-codec-") as directory:
            path, original = self.reader(directory, canonical)
            normal = self.render(path)
            self.assertEqual(normal.returncode, 0, normal.stderr)
            self.assertEqual(normal.stdout.encode(), original)
            path, original = self.reader(directory, alternate)
            result = self.render(path)
            self.assertEqual(
                result.returncode, 0,
                "equal embedded module bytes could not be rendered: " + result.stderr,
            )
            self.assertEqual(result.stdout.encode(), original)

    def test_saved_runtime_intent_is_boolean(self):
        with tempfile.TemporaryDirectory(prefix="pair-intent-codec-") as directory:
            fixture = self.pair(directory)
            normal = self.pair_command(fixture, "inspect")
            self.assertEqual(normal.returncode, 0, normal.stderr)
            self.assertIs(fixture["document"]["runtime"]["recorder_active"], False)
            fixture["document"]["runtime"]["recorder_active"] = 0
            self.save_receipt(fixture)
            result = self.pair_command(fixture, "inspect")
            self.assertNotEqual(
                result.returncode, 0,
                "non-boolean saved service intent was admitted",
            )

    def test_malformed_changed_bytes_and_different_module_remain_rejected(self):
        import base64
        import gzip
        with tempfile.TemporaryDirectory(prefix="pair-bad-codec-") as directory:
            fixture = self.pair(directory)
            original = fixture["document"]["recorder"]["override_b64"]
            fixture["document"]["recorder"]["override_b64"] = "!"
            self.save_receipt(fixture)
            malformed = self.pair_command(fixture, "inspect")
            self.assertNotEqual(malformed.returncode, 0, malformed.stderr)
            raw = base64.b64decode(original)
            changed = bytes([raw[0] ^ 1]) + raw[1:]
            fixture["document"]["recorder"]["override_b64"] = base64.b64encode(changed).decode()
            self.save_receipt(fixture)
            mismatch = self.pair_command(fixture, "inspect")
            self.assertNotEqual(mismatch.returncode, 0, mismatch.stderr)
        with tempfile.TemporaryDirectory(prefix="reader-bad-module-") as directory:
            path, original = self.reader(
                directory, base64.b64encode(gzip.compress(b"different module\n", mtime=0))
            )
            mismatch = self.render(path)
            self.assertNotEqual(mismatch.returncode, 0, mismatch.stderr)
            self.assertEqual(path.read_bytes(), original)



class ControlPrestateCleanupTests(unittest.TestCase):

    def test_missing_control_uses_defaults_and_foreign_paths_are_preserved(self):
        source = INSTALLER.read_text()
        definitions = []
        for name in ("control_defaults", "load_control_state"):
            matches = re.findall(rf"(?ms)^{name}\(\) \{{\n.*?^\}}\n", source)
            self.assertEqual(len(matches), 1)
            definitions.append(matches[0])
        script = (
            'set -euo pipefail\ncontrol_state="$CONTROL_STATE"\n'
            'boot_id() { printf "fixture-boot\\n"; }\n'
            'safe_blocked() { printf "%s\\n" "$*" >&2; exit 88; }\n'
            + "\n".join(definitions)
            + '\nload_control_state\nprintf "%s %s\\n" "$desired_state" "$state_boot_id"\n'
        )
        with tempfile.TemporaryDirectory(prefix="missing-control-defaults-") as temporary:
            root = pathlib.Path(temporary)
            for kind in ("missing", "dangling", "directory"):
                with self.subTest(kind=kind):
                    path = root / kind
                    if kind == "dangling":
                        path.symlink_to("absent-target")
                    elif kind == "directory":
                        path.mkdir()
                    result = subprocess.run(  # nosec B603 -- fixed real functions and isolated filesystem.
                        ["/bin/bash", "--noprofile", "--norc", "-s"],
                        input=script, capture_output=True, text=True,
                        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C",
                             "CONTROL_STATE": str(path), "PYTHONDONTWRITEBYTECODE": "1"},
                        check=False, timeout=5,
                    )
                    self.assertNotIn("command not found", result.stderr)
                    if kind == "missing":
                        self.assertEqual(
                            result.returncode, 0,
                            "missing control did not load existing running defaults: "
                            + result.stdout + result.stderr,
                        )
                        self.assertEqual(result.stdout, "running fixture-boot\n")
                        self.assertFalse(path.exists() or path.is_symlink())
                    else:
                        self.assertNotEqual(result.returncode, 0, result.stderr)
                        if kind == "dangling":
                            self.assertTrue(path.is_symlink())
                            self.assertEqual(path.readlink(), pathlib.Path("absent-target"))
                        else:
                            self.assertTrue(path.is_dir())
                            self.assertEqual(list(path.iterdir()), [])

    def test_cleanup_removes_only_owned_operation_and_empty_product_directory(self):
        source = INSTALLER.read_text()
        matches = re.findall(
            r"(?ms)^cleanup_control_prestate\(\) \{\n.*?^\}\n", source
        )
        self.assertEqual(len(matches), 1)
        definition = matches[0]
        script = (
            'set -euo pipefail\nbackup_dir="$BACKUP_DIR"\n'
            + definition + '\ncleanup_control_prestate "$OPERATION"\n'
        )
        cases = (
            "owned", "absent-marker", "missing-empty",
            "missing-other-backup", "foreign-file-mode",
            "foreign-file-link", "foreign-directory",
            "missing-foreign-directory", "invalid-operation",
        )
        with tempfile.TemporaryDirectory(prefix="control-prestate-cleanup-") as temporary:
            base = pathlib.Path(temporary)
            for kind in cases:
                with self.subTest(kind=kind):
                    case = base / kind
                    case.mkdir()
                    directory = case / "legacy-backups"
                    directory.mkdir(mode=0o700)
                    operation = "owned-operation"
                    record = directory / (operation + "-control-state.json")
                    other = directory / "other-operation-control-state.json"
                    protected = b"existing user backup must remain\n"
                    should_fail = False
                    if kind in {"owned", "absent-marker"}:
                        record.write_bytes(b"" if kind == "absent-marker" else b"saved control\n")
                        record.chmod(0o600)
                    elif kind == "missing-other-backup":
                        other.write_bytes(protected)
                        other.chmod(0o600)
                    elif kind == "foreign-file-mode":
                        record.write_bytes(protected)
                        record.chmod(0o644)
                        should_fail = True
                    elif kind == "foreign-file-link":
                        other.write_bytes(protected)
                        other.chmod(0o600)
                        record.symlink_to(other.name)
                        should_fail = True
                    elif kind == "foreign-directory":
                        record.mkdir()
                        should_fail = True
                    elif kind == "missing-foreign-directory":
                        directory.chmod(0o755)
                    elif kind == "invalid-operation":
                        operation = "../outside"
                        outside = case / "outside-control-state.json"
                        outside.write_bytes(protected)
                        outside.chmod(0o600)
                        should_fail = True
                    result = subprocess.run(  # nosec B603 -- fixed offline function and isolated paths.
                        ["/bin/bash", "--noprofile", "--norc", "-s"],
                        input=script, capture_output=True, text=True,
                        env={
                            "PATH": "/usr/bin:/bin", "LC_ALL": "C",
                            "PYTHONDONTWRITEBYTECODE": "1",
                            "BACKUP_DIR": str(directory), "OPERATION": operation,
                        },
                        check=False, timeout=5,
                    )
                    if should_fail:
                        self.assertNotEqual(result.returncode, 0, result.stderr)
                    else:
                        self.assertEqual(result.returncode, 0, result.stderr)
                    if kind in {"owned", "absent-marker", "missing-empty"}:
                        self.assertFalse(record.exists() or record.is_symlink())
                        self.assertFalse(directory.exists())
                    elif kind == "missing-other-backup":
                        self.assertEqual(other.read_bytes(), protected)
                        self.assertTrue(directory.is_dir())
                    elif kind == "foreign-file-mode":
                        self.assertEqual(record.read_bytes(), protected)
                        self.assertEqual(record.stat().st_mode & 0o777, 0o644)
                    elif kind == "foreign-file-link":
                        self.assertTrue(record.is_symlink())
                        self.assertEqual(other.read_bytes(), protected)
                    elif kind == "foreign-directory":
                        self.assertTrue(record.is_dir())
                    elif kind == "missing-foreign-directory":
                        self.assertTrue(directory.is_dir())
                        self.assertEqual(directory.stat().st_mode & 0o777, 0o755)
                    elif kind == "invalid-operation":
                        self.assertEqual(outside.read_bytes(), protected)


if __name__ == "__main__":
    unittest.main()

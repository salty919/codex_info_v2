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
        launcher = subprocess.Popen(  # noqa: S603 -- installed fixture launcher and isolated HOME.  # nosec B603
            ["/bin/bash", str(self.home / ".local/bin/codex-info"), "--ui"],
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


if __name__ == "__main__":
    unittest.main()

"""Finite real installer/update commands against closed fixture homes only."""

import hashlib
import json
import os
import stat
import subprocess  # nosec B404 # Fixed /bin/bash in private offline fixtures only.
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE_INSTALLER = ROOT / "packaging/install_linux_bundle.sh"
SCHEMA = "codex-info-update-channel-v1"
STABLE = "1.0.109"
BETA_OLD = "1.0.110-beta.9.1"
BETA_NEW = "1.0.110-beta.10.2"

CURL_FIXTURE = r'''#!/usr/bin/env python3
import json, os, pathlib, re, sys
root = pathlib.Path(os.environ["BETA_CASE_ROOT"])
args = sys.argv[1:]
url = output = header_file = write_out = ""
while args:
    option = args.pop(0)
    if option in {"--output","-o","--dump-header","-D","--write-out","-w","--max-time","--max-redirs","--proto","--proto-redir","--header","-H"}:
        value = args.pop(0)
        if option in {"--output","-o"}: output = value
        if option in {"--dump-header","-D"}: header_file = value
        if option in {"--write-out","-w"}: write_out = value
    elif not option.startswith("-"):
        url = option
with (root/"commands.log").open("a") as trace:
    trace.write("curl " + url + "\n")
if url.startswith("http://127.0.0.1:8787/"):
    os.execv("/bin/bash", ["/bin/bash", str(root/"fake-bin/curl-health"), *sys.argv[1:]])
headers = "HTTP/1.1 200 OK\r\n\r\n"
if url == "https://api.github.com/repos/salty919/codex_info_v2/releases/latest":
    payload = (root/"release-latest.json").read_bytes()
elif re.fullmatch(r"https://api[.]github[.]com/repos/salty919/codex_info_v2/releases[?]per_page=100&page=[1-5]", url):
    page = int(url.rsplit("=",1)[1])
    page_file = root / ("release-list-page" + str(page) + ".json")
    if not page_file.is_file(): raise SystemExit(22)
    payload = page_file.read_bytes()
    if (root/("release-list-page" + str(page+1) + ".json")).is_file():
        next_url = "https://api.github.com/repos/salty919/codex_info_v2/releases?per_page=100&page=" + str(page+1)
        headers = 'HTTP/1.1 200 OK\r\nLink: <' + next_url + '>; rel="next"\r\n\r\n'
    stale = root / "change-channel-on-discovery"
    if stale.is_file():
        channel = root / "home/.local/share/codex-info/update-channel.json"
        changed = channel.with_name("changed-channel.json")
        changed.write_text(json.dumps({"schema":"codex-info-update-channel-v1","channel":"stable","revision":"f"*32})+"\n")
        changed.chmod(0o600)
        changed.replace(channel)
        stale.unlink()
else:
    assets = json.loads((root/"expected-asset-urls.json").read_text())
    if url not in assets: raise SystemExit(22)
    payload = pathlib.Path(assets[url]).read_bytes()
if header_file: pathlib.Path(header_file).write_text(headers)
if output: pathlib.Path(output).write_bytes(payload)
else: sys.stdout.buffer.write(payload)
if write_out == "%{url_effective}": sys.stdout.write(url)
'''

SYSTEMCTL_FIXTURE = r'''#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys
root = pathlib.Path(os.environ["BETA_CASE_ROOT"])
result = subprocess.run(["/bin/bash", str(root/"fake-bin/systemctl-base"), *sys.argv[1:]], check=False)
if result.returncode == 0 and "daemon-reload" in sys.argv:
    trigger = root/"tamper-candidate-on-activation"
    current = root/"home/.local/share/codex-info/current"
    if trigger.is_file() and current.is_symlink():
        manifest = json.loads((current/"manifest.json").read_text())
        if "-beta." in manifest["version"]:
            (current/"codex_info_rest").write_bytes(b"changed fixture candidate\n")
            trigger.unlink()
raise SystemExit(result.returncode)
'''


class LinuxBetaUpdateIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="issue467-beta-update-")
        self.addCleanup(self.temporary.cleanup)
        self.case = Path(self.temporary.name)
        self.home = self.case / "home"
        self.home.mkdir()
        fixture = (ROOT / "scripts/test_linux_bundle.sh").read_text()
        boundary = 'boot_id_value="$(< /proc/sys/kernel/random/boot_id)"'
        prefix, _ = fixture.split(boundary, 1)
        old_root = 'ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"'
        prefix = prefix.replace(old_root, 'ROOT_DIR="${BETA_REPO_ROOT:?}"', 1)
        prefix = prefix.replace('TEST_ROOT="$(mktemp -d /tmp/codex-info-linux-bundle-test.XXXXXX)"', 'TEST_ROOT="${BETA_CASE_ROOT:?}"', 1)
        prefix = prefix.replace("trap 'rm -r -- \"$TEST_ROOT\"' EXIT\n", "", 1)
        self.setup_script = self.case / "fixture-setup.sh"
        self.setup_script.write_text(prefix)
        self.environment = os.environ.copy()
        for key in tuple(self.environment):
            if key.startswith(("FAKE_", "CODEX_INFO_INSTALL_", "CODEX_INFO_RELEASE_", "CODEX_INFO_UPDATE_", "CODEX_INFO_INTERRUPT", "CODEX_INFO_INTERNAL_")):
                self.environment.pop(key)
        self.environment.update({
            "BETA_CASE_ROOT": str(self.case), "BETA_REPO_ROOT": str(ROOT),
            "HOME": str(self.home), "CODEX_HOME": str(self.home / ".codex"),
            "FAKE_LOG": str(self.case / "commands.log"),
            "FAKE_TIMER_ACTIVE_FILE": str(self.home / ".timer-active"),
            "FAKE_STARTUP_CONDITION": "1",
            "FAKE_INSTALLER": str(self.home / ".local/libexec/codex-info-install.sh"),
            "FAKE_RELEASE_JSON": str(self.case / "release-latest.json"),
            "FAKE_RELEASE_ASSETS": str(self.case / "release-assets"),
            "CODEX_INFO_PROC_ROOT": str(self.case / "proc"),
            "SYSTEMCTL_BIN": "systemctl", "CURL_BIN": "curl",
            "TMPDIR": str(self.case / "update-tmp"),
            "PYTHONDONTWRITEBYTECODE": "1",
        })
        self.archives = {}
        self.prepared = False

    def prepare(self):
        if self.prepared:
            return
        result = subprocess.run(["/bin/bash", str(self.setup_script)], env=self.environment,  # nosec B603 # Owned offline setup; shell=False default.
                                capture_output=True, text=True, timeout=15, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.environment["PATH"] = str(self.case / "fake-bin") + os.pathsep + os.environ["PATH"]
        for name, replacement in (("curl", CURL_FIXTURE), ("systemctl", SYSTEMCTL_FIXTURE)):
            original = self.case / "fake-bin" / name
            suffix = "curl-health" if name == "curl" else "systemctl-base"
            original.replace(original.with_name(suffix))
            original.write_text(replacement)
            original.chmod(0o755)
        (self.case / "expected-asset-urls.json").write_text("{}")
        self.prepared = True

    @property
    def share(self):
        return self.home / ".local/share/codex-info"

    @property
    def channel_path(self):
        return self.share / "update-channel.json"

    def command(self, *arguments, source=False):
        self.prepare()
        installed = self.home / ".local/libexec/codex-info-install.sh"
        script = SOURCE_INSTALLER if source or not installed.is_symlink() else installed
        return subprocess.run(["/bin/bash", str(script), *arguments], env=self.environment,  # nosec B603 # Private HOME/proc and stubbed transports; shell=False default.
                              capture_output=True, text=True, timeout=35, check=False)

    def channel(self, value):
        result = self.command("--set-update-channel", value)
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(self.channel_path.read_text())
        self.assertEqual(set(value), {"schema", "channel", "revision"})
        self.assertEqual(value["schema"], SCHEMA)
        self.assertRegex(value["revision"], r"\A[0-9a-f]{32}\Z")
        self.assertEqual(stat.S_IMODE(self.channel_path.stat().st_mode), 0o600)
        self.assertEqual(self.channel_path.stat().st_uid, os.getuid())
        return value

    def build(self, version, *, attempt=1, source_digit="1"):
        if version in self.archives:
            return self.archives[version]
        self.prepare()
        environment = dict(self.environment, SOURCE_SHA=source_digit * 40, RUN_ID="92001",
                           RUN_ATTEMPT=str(attempt), OBJDUMP_BIN=str(self.case / "fake-bin/objdump"))
        result = subprocess.run(["/bin/bash", str(ROOT / "scripts/build_linux_bundle.sh"),  # nosec B603 # Fixed producer/argv and inert payloads; shell=False default.
            "--ui-binary", str(self.case / "fixture/codex_info"),
            "--recorder-binary", str(self.case / "fixture/codex_info_recorder"),
            "--rest-binary", str(self.case / "fixture/codex_info_rest"),
            "--version", version, "--output-dir", str(self.case / "output")],
            env=environment, capture_output=True, text=True, timeout=15, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = self.case / "output" / f"codex-info-{version}-x86_64-unknown-linux-gnu.tar.gz"
        self.assertTrue(archive.is_file())
        self.archives[version] = archive
        return archive

    def seed(self, version=STABLE, *, attempt=1):
        self.prepare()
        self.share.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.share.chmod(0o700)
        state = {"schema": "codex-info-control-state-v1", "desired_state": "stopped",
            "boot_id": Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
            "operation_id": "fixture", "generation_id": "", "updated_at_unix": 1}
        (self.share / "control-state.json").write_text(json.dumps(state) + "\n")
        (self.share / "control-state.json").chmod(0o600)
        archive = self.build(version, attempt=attempt)
        manifest = archive.with_name(archive.name.removesuffix(".tar.gz") + ".manifest.json")
        result = self.command("--bundle", str(archive), "--manifest", str(manifest), source=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.installed_version(), version)
        (self.home / ".codex").mkdir(exist_ok=True)
        for filename, contents in (("fixture-data.bin", b"fixture rows and checkpoint\n"),
                                   ("fixture-settings.json", b'{"language":"ja","timezone":"UTC"}\n'),
                                   ("fixture-backup.bin", b"fixture verified backup sentinel\n")):
            path = self.home / ".codex" / filename
            path.write_bytes(contents)
            path.chmod(0o600)

    def installed_version(self):
        return json.loads((self.share / "current/manifest.json").read_text())["version"]

    def protected(self):
        current = self.share / "current"
        generation = current.resolve()
        values = {"current": os.readlink(current)}
        for path in sorted(generation.rglob("*")):
            if path.is_file() and not path.is_symlink():
                values["generation/" + path.relative_to(generation).as_posix()] = (
                    hashlib.sha256(path.read_bytes()).hexdigest(), stat.S_IMODE(path.stat().st_mode))
        for path in sorted((self.home / ".codex").iterdir()):
            values["data/" + path.name] = (path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
        return values

    def release(self, version, *, attempt=1, source_digit="2"):
        archive = self.build(version, attempt=attempt, source_digit=source_digit)
        url = f"https://github.com/salty919/codex_info_v2/releases/download/windows-v{version}/{archive.name}"
        urls_path = self.case / "expected-asset-urls.json"
        urls = json.loads(urls_path.read_text())
        urls[url] = str(archive)
        urls_path.write_text(json.dumps(urls))
        return {"tag_name": "windows-v" + version, "draft": False,
            "prerelease": "-beta." in version, "published_at": "2026-09-01T00:00:00Z",
            "assets": [{"name": archive.name, "state": "uploaded", "browser_download_url": url,
                        "digest": "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest()}]}

    def publish(self, latest, *pages):
        (self.case / "release-latest.json").write_text(json.dumps(latest))
        for number, page in enumerate(pages, 1):
            (self.case / f"release-list-page{number}.json").write_text(json.dumps(page))

    def trace(self):
        path = self.case / "commands.log"
        return path.read_text() if path.exists() else ""

    def clear_trace(self):
        (self.case / "commands.log").write_text("")

    def test_unset_and_stable_keep_normal_stable_update(self):
        initial = self.command("--get-update-channel")
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self.assertEqual(initial.stdout, "selected_channel=stable installed_channel=unavailable installed_version=unavailable\n")
        self.assertFalse(self.share.exists())
        self.seed()
        stable = self.release("1.0.110")
        beta = self.release("1.0.111-beta.1.1")
        self.publish(stable, [stable, beta])
        self.clear_trace()
        result = self.command("--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.installed_version(), "1.0.110")
        self.assertIn("/releases/latest", self.trace())
        self.assertNotIn("/releases?", self.trace())
        self.assertNotIn("/download/windows-v1.0.111-beta.1.1/", self.trace())
        current = self.command("--get-update-channel")
        self.assertEqual(current.stdout, "selected_channel=stable installed_channel=stable installed_version=1.0.110\n")
        self.channel("stable")
        self.clear_trace()
        unchanged = self.protected()
        result = self.command("--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.protected(), unchanged)
        self.assertNotIn("/download/", self.trace())

    def test_channel_roundtrip_survives_process_restart_and_preserves_settings(self):
        self.channel("beta")
        self.seed()
        before = self.protected()
        beta = self.channel("beta")
        result = self.command("--get-update-channel")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "selected_channel=beta installed_channel=stable installed_version=1.0.109\n")
        self.assertEqual(self.channel("beta"), beta)
        stable = self.channel("stable")
        self.assertNotEqual(stable["revision"], beta["revision"])
        self.assertEqual(self.protected(), before)
        self.assertEqual(stat.S_IMODE(self.share.stat().st_mode), 0o700)

    def test_unsafe_channel_file_is_rejected_before_update(self):
        self.seed()
        stable = self.release(STABLE)
        self.publish(stable, [stable])
        before = self.protected()
        invalid = [
            ({"schema": SCHEMA, "channel": "nightly", "revision": "1" * 32}, 0o600),
            ({"schema": "unknown", "channel": "beta", "revision": "1" * 32}, 0o600),
            ({"schema": SCHEMA, "channel": "beta", "revision": "1" * 32, "extra": True}, 0o600),
            ({"schema": SCHEMA, "channel": "beta", "revision": "1" * 32}, 0o644),
        ]
        for value, mode in invalid:
            self.channel_path.write_text(json.dumps(value))
            self.channel_path.chmod(mode)
            original = self.channel_path.read_bytes()
            self.clear_trace()
            result = self.command("--update")
            self.assertNotEqual(result.returncode, 0, (value, mode))
            self.assertIn("channel", result.stderr.lower())
            self.assertNotIn("curl ", self.trace())
            self.assertEqual(self.channel_path.read_bytes(), original)
            self.assertEqual(self.protected(), before)
        self.channel_path.unlink()
        target = self.home / "foreign-channel.json"
        target.write_text(json.dumps({"schema": SCHEMA, "channel": "beta", "revision": "1" * 32}))
        target.chmod(0o600)
        self.channel_path.symlink_to(target)
        self.clear_trace()
        result = self.command("--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("curl ", self.trace())
        self.assertTrue(self.channel_path.is_symlink())
        self.assertEqual(self.protected(), before)

    def test_persisted_beta_updates_through_actual_transaction(self):
        self.channel("beta")
        self.seed()
        lower = self.release(BETA_OLD)
        expected = self.release(BETA_NEW, attempt=2, source_digit="3")
        higher_stable = self.release("1.0.111")
        self.publish(higher_stable, [lower], [expected, higher_stable])
        before_data = {key: value for key, value in self.protected().items() if key.startswith("data/")}
        self.clear_trace()
        result = self.command("--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.installed_version(), BETA_NEW)
        expected_manifest = json.loads((self.share / "current/manifest.json").read_text())
        self.assertEqual(expected_manifest["source_sha"], "3" * 40)
        self.assertEqual(expected_manifest["run_attempt"], 2)
        self.assertIn("/releases?per_page=100&page=1", self.trace())
        self.assertIn("/releases?per_page=100&page=2", self.trace())
        self.assertIn(expected["assets"][0]["browser_download_url"], self.trace())
        self.assertNotIn(lower["assets"][0]["browser_download_url"], self.trace())
        self.assertNotIn(higher_stable["assets"][0]["browser_download_url"], self.trace())
        journal = json.loads((self.share / "install-transaction.json").read_text())
        self.assertEqual(journal["phase"], "committed")
        self.assertEqual(journal["new_generation"], os.readlink(self.share / "current").removeprefix("generations/"))
        result = self.command("--get-update-channel")
        self.assertEqual(result.stdout, "selected_channel=beta installed_channel=beta installed_version=" + BETA_NEW + "\n")
        self.assertEqual({key: value for key, value in self.protected().items() if key.startswith("data/")}, before_data)

    def test_absent_beta_keeps_current_without_stable_fallback(self):
        self.channel("beta")
        self.seed()
        stable = self.release("1.0.110")
        self.publish(stable, [stable])
        before = self.protected()
        channel = self.channel_path.read_bytes()
        self.clear_trace()
        result = self.command("--update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("no beta candidate", result.stdout.lower())
        self.assertNotIn("/releases/latest", self.trace())
        self.assertNotIn("/download/", self.trace())
        self.assertEqual(self.protected(), before)
        self.assertEqual(self.channel_path.read_bytes(), channel)

    def test_changed_channel_rejects_old_discovery_before_apply(self):
        self.channel("beta")
        self.seed()
        beta = self.release(BETA_NEW, attempt=2)
        self.publish(self.release("1.0.110"), [beta])
        before = self.protected()
        (self.case / "change-channel-on-discovery").write_text("change intent\n")
        self.clear_trace()
        result = self.command("--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("channel", result.stderr.lower())
        self.assertNotIn("/download/", self.trace())
        self.assertEqual(self.protected(), before)
        self.assertEqual(json.loads(self.channel_path.read_text())["channel"], "stable")

    def test_failure_rolls_back_generation_without_changing_data_or_selection(self):
        self.channel("beta")
        self.seed()
        beta = self.release(BETA_NEW, attempt=2)
        self.publish(self.release("1.0.110"), [beta])
        before = self.protected()
        channel = self.channel_path.read_bytes()
        (self.case / "tamper-candidate-on-activation").write_text("closed fixture fault\n")
        result = self.command("--update")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.case / "tamper-candidate-on-activation").exists())
        self.assertEqual(self.protected(), before)
        self.assertEqual(self.channel_path.read_bytes(), channel)
        journal = json.loads((self.share / "install-transaction.json").read_text())
        self.assertEqual(journal["phase"], "committed")
        self.assertEqual(journal["old_generation"], os.readlink(self.share / "current").removeprefix("generations/"))
        self.assertEqual(json.loads((self.share / "control-state.json").read_text())["desired_state"], "stopped")

    def test_normal_timer_refuses_beta_to_lower_stable_without_transition(self):
        self.channel("beta")
        self.seed(BETA_OLD)
        self.channel("stable")
        self.publish(self.release(STABLE), [self.release(BETA_NEW, attempt=2)])
        before = self.protected()
        self.clear_trace()
        result = self.command("--timer-update")
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("/download/", self.trace())
        self.assertEqual(self.protected(), before)
        self.assertEqual(self.installed_version(), BETA_OLD)
        self.assertEqual(json.loads(self.channel_path.read_text())["channel"], "stable")


if __name__ == "__main__":
    unittest.main()

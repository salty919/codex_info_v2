"""Exercise the real recovery fixture and strict last-good comparison."""

import copy
import json
import select
import sqlite3

# This test executes only fixed local interpreters and repository fixtures.
import subprocess  # noqa: B404  # nosec B404
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent


class LastGoodRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def server(self, **variables):
        env = {"PATH": "/usr/bin:/bin", "CODEX_INFO_FAKE_RESET_AT": "1800000000"}
        env.update(variables)
        # Static argv, isolated Python, private fixture values, no caller environment.
        process = subprocess.Popen(  # noqa: B603  # nosec B603
            ["/usr/bin/python3", "-I", "fake_codex_app_server.py"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=env,
            cwd=ROOT,
            shell=False,
        )

        def cleanup():
            if process.poll() is None:
                process.terminate()
            process.communicate(timeout=3)

        self.addCleanup(cleanup)
        return process

    def send(self, process, method):
        process.stdin.write(json.dumps({"id": 1, "method": method}) + "\n")
        process.stdin.flush()

    def receive(self, process):
        self.assertTrue(
            select.select([process.stdout], [], [], 3)[0], "RPC response missing"
        )
        return json.loads(process.stdout.readline())

    def test_paused_quota_cannot_advance_baseline_and_fails_after_release(self):
        pause = self.root / "pause"
        failure = self.root / "failure"
        pause.touch()
        process = self.server(
            CODEX_INFO_FAKE_QUOTA_PAUSE_FILE=str(pause),
            CODEX_INFO_FAKE_FAILURE_FILE=str(failure),
        )
        self.send(process, "account/rateLimits/read")
        deadline = time.monotonic() + 3
        while not Path(f"{pause}.ack").exists() and time.monotonic() < deadline:
            if select.select([process.stdout], [], [], 0.01)[0]:
                self.fail("quota response escaped before the baseline could be pinned")
        self.assertTrue(
            Path(f"{pause}.ack").exists(), "quota pause was not acknowledged"
        )
        self.assertFalse(select.select([process.stdout], [], [], 0)[0])
        failure.touch()
        pause.unlink()
        self.assertEqual(self.receive(process)["error"]["code"], -32000)
        failure.unlink()
        self.send(process, "account/rateLimits/read")
        self.assertEqual(
            self.receive(process)["result"]["rateLimits"]["primary"]["usedPercent"], 56
        )

    def test_unconfigured_fixture_preserves_other_consumers(self):
        process = self.server()
        for method, expected in (
            ("initialize", {}),
            (
                "account/read",
                {
                    "requiresOpenaiAuth": False,
                    "account": {
                        "type": "chatgpt",
                        "email": "fixture@example.com",
                        "planType": "pro",
                    },
                },
            ),
            (
                "account/rateLimits/read",
                {
                    "rateLimits": {
                        "primary": {
                            "usedPercent": 56,
                            "resetsAt": 1800000000,
                            "windowDurationMins": 10080,
                        }
                    }
                },
            ),
            ("thread/list", {"data": []}),
        ):
            self.send(process, method)
            self.assertEqual(self.receive(process)["result"], expected)

    def test_failure_injection_waits_for_latest_durable_observation(self):
        source = (ROOT / "x11_service_recovery_visual_gate.sh").read_text()
        function = source.split("inject_app_server_failure() {\n", 1)[1].split(
            "\n}\n", 1
        )[0]
        journal = self.root / "data/history/quota/observations.sqlite3"
        journal.parent.mkdir(parents=True)
        with sqlite3.connect(journal) as db:
            db.execute(
                "CREATE TABLE quota_observations "
                "(sequence INTEGER, observed_at INTEGER, account_verified INTEGER)"
            )
            db.execute("INSERT INTO quota_observations VALUES (1, 104, 1)")
        (self.root / "app-server-quota-pause.ack").touch()
        ready = self.root / "ready.json"
        script = (
            "curl() {\n"
            'if [[ -f "$temp_root/read-once" ]]; then timestamp=104; '
            'else timestamp=100; fi\n'
            'touch "$temp_root/read-once"\n'
            'printf \'{"state":"ready","authenticated":true,"observed_at":%s}\' '
            '"$timestamp"\n'
            "}\n"
            "inject_app_server_failure() {\n" + function + "\n}\n"
            "inject_app_server_failure\n"
        )
        # stdin contains only the repository function and literal fixture commands.
        # Values remain environment data; no caller shell startup hooks are inherited.
        result = subprocess.run(  # noqa: B603  # nosec B603
            ["/bin/bash", "--noprofile", "--norc", "-s"],
            input=script,
            capture_output=True,
            text=True,
            check=False,
            shell=False,
            env={
                "PATH": "/usr/bin:/bin",
                "temp_root": str(self.root),
                "ready_current": str(ready),
                "port": "0",
            },
            timeout=5,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            json.loads(ready.read_text())["observed_at"],
            104,
            "failure was injected before the last durable quota reached REST",
        )
        self.assertTrue((self.root / "app-server-failure").exists())

    def test_gate_rejects_lost_or_changed_last_good_fields(self):
        source = (ROOT / "x11_service_recovery_visual_gate.sh").read_text()
        function = source.split("service_last_good_error() {\n", 1)[1].split(
            "\n}\n", 1
        )[0]
        baseline = {
            "state": "ready",
            "authenticated": True,
            "observed_at": 100,
            "plan_label": "pro",
            "quota": {"remaining_percent": 44},
            "models": [{"model": "gpt-5.6-sol", "total_tokens": 1000}],
        }
        ready = self.root / "ready.json"
        ready.write_text(json.dumps(baseline))
        current = dict(baseline, state="error")
        cases = [("retained", current, 0)]
        for key in ("observed_at", "plan_label", "quota", "models"):
            missing = copy.deepcopy(current)
            del missing[key]
            cases.append((f"missing {key}", missing, 1))
        cases.extend(
            [
                ("new timestamp", dict(current, observed_at=104), 1),
                ("lost authentication", dict(current, authenticated=False), 1),
                ("not yet failed", baseline, 1),
            ]
        )
        for name, value, expected in cases:
            with self.subTest(name=name):
                script = (
                    'curl() { printf "%s" "$CURRENT"; }\n'
                    "service_last_good_error() {\n" + function + "\n}\n"
                    "service_last_good_error\n"
                )
                # Execute the fixed repository function; JSON is environment data.
                result = subprocess.run(  # noqa: B603  # nosec B603
                    ["/bin/bash", "--noprofile", "--norc", "-s"],
                    input=script,
                    capture_output=True,
                    text=True,
                    check=False,
                    shell=False,
                    env={
                        "PATH": "/usr/bin:/bin",
                        "CURRENT": json.dumps(value),
                        "ready_current": str(ready),
                        "port": "0",
                    },
                    timeout=3,
                )
                self.assertEqual(result.returncode, expected, result.stderr)


if __name__ == "__main__":
    unittest.main()

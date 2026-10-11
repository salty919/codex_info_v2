"""Exercise the CLI acceptance caller with deterministic REST startup replies."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/cli_contract_e2e.sh"
READY = {"state": "ready", "authenticated": True}


class RestReadinessTests(unittest.TestCase):
    def run_caller(self, replies, phase="restart"):
        source = SCRIPT.read_text(encoding="utf-8")
        helper = ""
        if "wait_for_ready_details() {" in source:
            helper = source.split("wait_for_ready_details() {", 1)[1].split("\n}", 1)[0]
            helper = "wait_for_ready_details() {" + helper + "\n}\n"
        if phase == "restart":
            start = source.index("details_after=", source.index("# Restart REST"))
            end = source.index('python3 - "$details_before" "$details_after"', start)
            result = "$details_after"
        else:
            start = source.index('details_body=', source.index('details_before='))
            end = source.index('# REST details must be backed', start)
            result = "$details_body"
        with tempfile.TemporaryDirectory(prefix="cli-readiness-") as directory:
            root = Path(directory)
            (root / "replies").write_text("\n".join(replies), encoding="utf-8")
            (root / "count").write_text("0", encoding="utf-8")
            shell = r'''
set -euo pipefail
port=12345
details_before="$FIXTURE/details-before.json"
fail() { echo "$*" >&2; exit 1; }
sleep() { :; }
curl() {
    local count
    count=$(cat "$FIXTURE/count")
    count=$((count + 1))
    printf '%s' "$count" > "$FIXTURE/count"
    sed -n "${count}p" "$FIXTURE/replies"
}
'''
            command = shell + helper + source[start:end] + f'printf "%s" "{result}"'
            completed = subprocess.run(
                ["bash", "-c", command], capture_output=True, text=True,
                env={**os.environ, "FIXTURE": str(root)}, timeout=10, check=False,
            )
            return completed, int((root / "count").read_text(encoding="utf-8"))

    def test_initializing_then_ready_for_initial_start_and_restart(self):
        for phase in ("initial", "restart"):
            with self.subTest(phase=phase):
                completed, calls = self.run_caller([
                    json.dumps({"state": "initializing", "authenticated": False}),
                    json.dumps(READY),
                ], phase)
                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertEqual(json.loads(completed.stdout), READY)
                self.assertEqual(calls, 2)

    def test_nonready_unauthenticated_and_malformed_never_pass(self):
        for response in (
            '{"state":"initializing","authenticated":false}',
            '{"state":"ready","authenticated":false}',
            'not-json',
            '{"state":"ready","authenticated":"True"}',
        ):
            with self.subTest(response=response):
                completed, calls = self.run_caller([response] * 40)
                self.assertNotEqual(completed.returncode, 0)
                self.assertLessEqual(calls, 40)
                self.assertIn("non-ready", completed.stderr)


if __name__ == "__main__":
    unittest.main()

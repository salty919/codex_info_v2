"""Execute the CLI gate's real readback consumers with independent wire facts."""
import copy
import json
import pathlib
import re
import sqlite3
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/cli_contract_e2e.sh"
FIXTURE = json.loads((ROOT / "tests/fixtures/recorded_scope_contract.json").read_text())


def python_consumer(arguments):
    marker = f"python3 - {arguments} <<'PY'\n"
    source = SCRIPT.read_text()
    if source.count(marker) != 1:
        raise AssertionError("CLI consumer must have one production caller")
    return source.split(marker, 1)[1].split("\nPY\n", 1)[0]


class RecordedScopeCliConsumers(unittest.TestCase):
    def run_consumer(self, arguments, values):
        result = subprocess.run(["python3", "-c", python_consumer(arguments), *values],
                                capture_output=True, text=True, check=False, timeout=5)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_readiness_reads_the_recorded_current_resource(self):
        source = SCRIPT.read_text()
        function = source.split("wait_for_ready_details() {\n", 1)[1].split("\n}\n", 1)[0]
        shell = """curl() {
    if [[ "${@: -1}" == */v5/current ]]; then printf '%s' "$FIXTURE_CURRENT"; else return 22; fi
}
wait_for_ready_details() {
""" + function + "\n}\nwait_for_ready_details 12345\n"
        result = subprocess.run(["bash", "-c", shell], env={"PATH": "/usr/bin:/bin",
                                "FIXTURE_CURRENT": json.dumps(FIXTURE["current"])},
                                capture_output=True, text=True, check=False, timeout=8)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["models"][0]["total_tokens"], 15)

    def test_initial_wire_consumer_accepts_the_exact_new_contract(self):
        health = {"api_version": "v1", "service": "codex-info", "product_version": "1.0.0"}
        self.run_consumer('"$health_body" "$details_body" "$ROOT_VERSION"',
                          [json.dumps(health), json.dumps(FIXTURE["current"]), "1.0.0"])

    def test_recorded_models_are_compared_with_their_explicit_range(self):
        with tempfile.TemporaryDirectory(prefix="cli-recorded-consumer-") as directory:
            database = pathlib.Path(directory) / "history.sqlite3"
            with sqlite3.connect(database) as db:
                db.executescript("CREATE TABLE quota_receipts(sequence INTEGER,observed_at INTEGER,reset_at INTEGER,window_seconds INTEGER,remaining_percent REAL);"
                                 "CREATE TABLE recorded_usage_facts(fact_id TEXT,timestamp INTEGER,model TEXT,tokens_json TEXT,eligible INTEGER);"
                                 "CREATE TABLE session_ranges(start_offset INTEGER);")
                db.execute("INSERT INTO quota_receipts VALUES(1,1792228061,1792823932,604800,88.0)")
                db.execute("INSERT INTO session_ranges VALUES(0)")
                for name, at, total, incoming, cached, outgoing in [
                    ("A", 1792228000, 10, 8, 2, 2), ("B", 1792228020, 5, 4, 1, 1)]:
                    tokens = {"total": total, "input": incoming, "cached_input": cached,
                              "cache_write_input": 0, "output": outgoing}
                    db.execute("INSERT INTO recorded_usage_facts VALUES(?,?,?,?,1)",
                               (name, at, "gpt-6.1-sol", json.dumps(tokens)))
            self.run_consumer('"$details_body" "$database"',
                              [json.dumps(FIXTURE["current"]), str(database)])

    def test_restart_consumer_reads_the_new_model_fields(self):
        with tempfile.TemporaryDirectory(prefix="cli-recorded-restart-") as directory:
            before = pathlib.Path(directory) / "before.json"
            before.write_text(json.dumps(FIXTURE["current"]))
            after = copy.deepcopy(FIXTURE["current"])
            after["models"][0].update(total_tokens=20, input_tokens=16,
                                      cached_input_tokens=4, output_tokens=4)
            database = pathlib.Path(directory) / "history.sqlite3"
            with sqlite3.connect(database) as db:
                db.executescript("CREATE TABLE session_ranges(start_offset INTEGER); INSERT INTO session_ranges VALUES(0);")
            self.run_consumer('"$details_before" "$details_after" "$database"',
                              [str(before), json.dumps(after), str(database)])


if __name__ == "__main__":
    unittest.main()

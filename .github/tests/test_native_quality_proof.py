"""Reject incomplete or mismatched native reuse evidence before skipping tests."""
from __future__ import annotations

import copy
import importlib.util
import io
import json
from pathlib import Path
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]


class NativeProofTests(unittest.TestCase):
    def setUp(self):
        path = ROOT / ".github/scripts/native_quality_proof.py"
        spec = importlib.util.spec_from_file_location("native_quality_proof", path)
        self.module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.module)
        self.expected = {"tree": "a" * 40, "main_base": "b" * 40,
                         "workflow": "c" * 40,
                         "runtime": {"rustc": "rustc exact", "cargo": "cargo exact",
                                     "clippy": "clippy exact", "coverage": "coverage exact",
                                     "image_os": "ubuntu24", "image_version": "exact", "packages": "exact",
                                     "environment": {}}}
        self.proof = {"schema": 1, "kind": "feat-preflight", "source": "d" * 40,
                      "main_included": True, "pr": 7, "run": 10, "attempt": 2,
                      **copy.deepcopy(self.expected)}
        self.run = {"id": 10, "run_attempt": 2, "head_sha": "d" * 40,
                    "status": "completed", "conclusion": "success",
                    "event": "pull_request_target", "path": ".github/workflows/feat-integration.yml",
                    "repository": {"full_name": "owner/repo"},
                    "display_title": "codex-feat-preflight-v1:pr=7:event_head=" + "d" * 40,
                    "referenced_workflows": [
                        {"path": "owner/repo/.github/workflows/rust.yml@" + "c" * 40, "sha": "c" * 40},
                        {"path": "owner/repo/.github/workflows/selective-quality.yml@" + "c" * 40, "sha": "c" * 40},
                    ]}
        self.job = {"name": "Run selected advisory quality / linux-backend-quality / native-quality",
                    "status": "completed", "conclusion": "success",
                    "steps": [{"name": name, "status": "completed", "conclusion": "success"} for name in
                              ("Reconstruct the trusted planned release tree", "Run native unit tests with coverage",
                               "Reject native compiler and Clippy warnings", "Bind successful native checks to exact inputs")]}

    def test_only_exact_successful_proof_can_skip(self):
        self.assertTrue(self.module.matches(self.proof, self.expected))
        self.assertTrue(self.module.producer_matches(self.proof, self.run, [self.job], "owner/repo"))
        for key in ("tree", "main_base", "workflow", "runtime", "main_included", "kind", "schema"):
            bad = copy.deepcopy(self.proof)
            bad[key] = {} if key == "runtime" else False
            self.assertFalse(self.module.matches(bad, self.expected), key)
        for key in ("rustc", "cargo", "clippy", "coverage", "image_os", "image_version", "packages", "environment"):
            expected = copy.deepcopy(self.expected)
            expected["runtime"][key] = "changed"
            self.assertFalse(self.module.matches(self.proof, expected), key)

    def test_failed_latest_attempt_wrong_run_repo_definition_and_jobs_are_rejected(self):
        for key, value in (("conclusion", "failure"), ("status", "in_progress"),
                           ("id", 11), ("run_attempt", 3), ("head_sha", "e" * 40),
                           ("event", "push"), ("path", ".github/workflows/other.yml"),
                           ("repository", {"full_name": "fork/repo"}),
                           ("referenced_workflows", []), ("display_title", "unbound title")):
            run = copy.deepcopy(self.run); run[key] = value
            self.assertFalse(self.module.producer_matches(self.proof, run, [self.job], "owner/repo"), key)
        for jobs in ([], [self.job, self.job], [{**self.job, "conclusion": "failure"}],
                     [{**self.job, "steps": []}],
                     [{**self.job, "steps": [{**item, "conclusion": "skipped"} for item in self.job["steps"]]}]):
            self.assertFalse(self.module.producer_matches(self.proof, self.run, jobs, "owner/repo"))

    def test_artifact_digest_and_bounded_single_report(self):
        import hashlib
        def archive(entries):
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as z:
                for name, content in entries:
                    z.writestr(name, content)
            return buffer.getvalue()
        raw = archive([("rust.cobertura.xml", b'<coverage lines-valid="2"/>')])
        digest = "sha256:" + hashlib.sha256(raw).hexdigest()
        self.assertEqual(self.module.report_from_archive(raw, digest), b'<coverage lines-valid="2"/>')
        for bad, hash_value in ((raw, "sha256:" + "0" * 64),
                                (archive([("../rust.cobertura.xml", b"x")]), digest),
                                (archive([("rust.cobertura.xml", b"x"), ("run.py", b"x")]), digest)):
            with self.assertRaises(ValueError):
                self.module.report_from_archive(bad, hash_value)

    def test_actual_cobertura_declaration_without_entities_is_accepted(self):
        declaration = b'<!DOCTYPE coverage SYSTEM "https://cobertura.sourceforge.net/xml/coverage-04.dtd">'
        xml = declaration + b'<coverage lines-valid="2"/>'
        self.assertEqual(self.module.read_xml(xml).tag, "coverage")
        for unsafe in (
            declaration + b'<!ENTITY injected "value"><coverage lines-valid="2"/>',
            b'<!DOCTYPE coverage SYSTEM "https://example.invalid/evil.dtd"><coverage lines-valid="2"/>',
            b'<!DOCTYPE coverage [<!ENTITY injected "value">]><coverage lines-valid="2"/>',
            declaration + declaration + b'<coverage lines-valid="2"/>',
        ):
            with self.assertRaises(ValueError):
                self.module.read_xml(unsafe)

    def test_multibyte_encoding_cannot_bypass_entity_rejection(self):
        payload = '<!DOCTYPE coverage [<!ENTITY injected "encoding-bypass">]><coverage lines-valid="2">&injected;</coverage>'
        for encoding in ("utf-16", "utf-16-le", "utf-32", "utf-32-le"):
            with self.subTest(encoding=encoding), self.assertRaises(ValueError):
                self.module.read_xml(payload.encode(encoding))

    def test_missing_or_unsafe_metadata_is_not_evidence(self):
        for data in (b'<coverage lines-valid="2"/>', b'<!DOCTYPE coverage><coverage/>',
                     b'<coverage codex-native-proof="not json"/>'):
            with self.assertRaises(ValueError):
                self.module.read_proof(data)


if __name__ == "__main__":
    unittest.main()

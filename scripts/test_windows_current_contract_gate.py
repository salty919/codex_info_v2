"""Exercise the current acquisition gate, including missing-consumer evidence."""

import os
import shutil
import subprocess  # noqa: B404  # nosec B404 - fixed local test command
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FAKE_DOTNET = r'''#!/usr/bin/python3
import os
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

args = sys.argv[1:]
if args[0] != "test":
    sys.exit(0)
if "--filter" in args:
    classes = [part.split("~", 1)[1] for part in args[args.index("--filter") + 1].split("|")]
else:
    classes = ["CodexInfo.WindowsClient.Core.Tests.LoopbackStatusClientTests",
               "CodexInfo.WindowsClient.Presentation.Tests.MainWindowViewModelTests",
               "CodexInfo.WindowsClient.Presentation.Tests.ModelUsageDisplayContractTests",
               "CodexInfo.WindowsClient.Presentation.Tests.UnrelatedTests"]
print("fixture consumers: " + ",".join(classes))
mode = os.environ.get("GATE_FIXTURE_MODE", "pass")
if mode == "missing-report":
    sys.exit(0)
if mode == "missing-consumer":
    classes = [name for name in classes if not name.endswith("ModelUsageDisplayContractTests")]
root = ET.Element("TestRun")
definitions = ET.SubElement(root, "TestDefinitions")
for name in classes:
    unit = ET.SubElement(definitions, "UnitTest")
    ET.SubElement(unit, "TestMethod", className=name, name="FixtureObservation")
skipped = int(mode == "skipped" and any(name.endswith("ModelUsageDisplayContractTests") for name in classes))
failed = int(mode == "failed")
ET.SubElement(root, "Counters", total=str(len(classes)), executed=str(len(classes) - skipped),
              passed=str(len(classes) - skipped - failed), failed=str(failed), notExecuted=str(skipped))
directory = Path(args[args.index("--results-directory") + 1])
directory.mkdir(parents=True, exist_ok=True)
ET.ElementTree(root).write(directory / (Path(args[1]).stem + ".trx"))
'''


class WindowsCurrentContractGateTests(unittest.TestCase):
    def setUp(self):
        artifacts = ROOT / "artifacts"
        artifacts.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=artifacts)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        scripts = self.root / "scripts"
        scripts.mkdir()
        shutil.copy2(ROOT / "scripts/windows_client_contract_gate.sh", scripts)
        binaries = self.root / "bin"
        binaries.mkdir()
        dotnet = binaries / "dotnet"
        dotnet.write_text(FAKE_DOTNET)
        dotnet.chmod(0o755)

    def run_gate(self, mode, *, full=False):
        return subprocess.run(  # noqa: B603,S603  # nosec B603 - only two literal argv lists; test-owned cwd
            ["/bin/bash", "scripts/windows_client_contract_gate.sh"]
            + ([] if full else ["--current-acquisition"]),
            cwd=self.root,
            env={**os.environ, "PATH": str(self.root / "bin") + ":/usr/bin:/bin",
                 "GATE_FIXTURE_MODE": mode, "TMPDIR": str(self.root)},
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )

    def test_all_three_consumers_are_required(self):
        result = self.run_gate("pass")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS check=windows-current-acquisition", result.stdout)
        for consumer in (
            "CodexInfo.WindowsClient.Core.Tests.LoopbackStatusClientTests",
            "CodexInfo.WindowsClient.Presentation.Tests.MainWindowViewModelTests",
            "CodexInfo.WindowsClient.Presentation.Tests.ModelUsageDisplayContractTests",
        ):
            self.assertIn(consumer, result.stdout)

    def test_missing_skipped_failed_or_absent_evidence_is_not_accepted(self):
        for full in (False, True):
            for mode in ("missing-consumer", "skipped", "failed", "missing-report"):
                with self.subTest(mode=mode, full=full):
                    result = self.run_gate(mode, full=full)
                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertNotIn("PASS check=windows-", result.stdout)

    def test_full_gate_accepts_complete_evidence_with_other_consumers(self):
        result = self.run_gate("pass", full=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS check=windows-contract", result.stdout)


if __name__ == "__main__":
    unittest.main()

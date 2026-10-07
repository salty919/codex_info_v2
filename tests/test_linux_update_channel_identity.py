"""Public installer identity must survive a missing derived manifest link."""

import importlib.util
import json
import os
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "linux_beta_update_fixture", ROOT / "scripts/test_linux_beta_update.py"
)
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


class LinuxUpdateChannelIdentityTests(unittest.TestCase):
    def setUp(self):
        # Compose the existing offline fixture without inheriting its test cases.
        self.fixture = fixtures.LinuxBetaUpdateIntegrationTests("runTest")
        self.addCleanup(self.fixture.doCleanups)
        self.addCleanup(self.fixture.tearDown)
        self.fixture.setUp()

    def test_missing_manifest_projection_repairs_same_generation(self):
        self.fixture.seed()
        projection = self.fixture.share / "manifest.json"
        generation_manifest = (self.fixture.share / "current/manifest.json").resolve()
        before = self.fixture.protected()
        self.assertEqual(json.loads(generation_manifest.read_text())["source_sha"], "1" * 40)
        self.assertTrue(projection.is_symlink())
        self.assertFalse(self.fixture.channel_path.exists())
        projection.unlink()
        self.fixture.publish(self.fixture.release(fixtures.STABLE))

        result = self.fixture.command("--update")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(projection.is_symlink())
        self.assertEqual(projection.resolve(), generation_manifest)
        self.assertEqual(self.fixture.protected(), before)
        self.assertFalse(self.fixture.channel_path.exists())

    def test_missing_projection_does_not_allow_beta_to_stable_install(self):
        self.fixture.channel("beta")
        self.fixture.seed(fixtures.BETA_OLD)
        self.fixture.channel("stable")
        projection = self.fixture.share / "manifest.json"
        self.assertTrue(projection.is_symlink())
        projection.unlink()
        before = self.fixture.protected()
        selection = self.fixture.channel_path.read_bytes()
        archive = self.fixture.build(fixtures.STABLE)
        manifest = archive.with_name(archive.name.removesuffix(".tar.gz") + ".manifest.json")

        result = self.fixture.command("--bundle", str(archive), "--manifest", str(manifest), source=True)

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("explicit beta-to-stable return is not supported", result.stderr)
        self.assertEqual(self.fixture.protected(), before)
        self.assertEqual(self.fixture.channel_path.read_bytes(), selection)
        self.assertEqual(os.readlink(self.fixture.share / "current"), before["current"])
        self.assertFalse(projection.exists())
        self.assertFalse(projection.is_symlink())


if __name__ == "__main__":
    unittest.main()

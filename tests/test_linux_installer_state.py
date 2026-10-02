#!/usr/bin/env python3
"""Finite tests of installer state functions; no live services or profile writes."""

import pathlib
import re
import shlex
import subprocess  # nosec B404 # offline installer fixture API; fixed shell/env reviewed at the call.
import tempfile
import unittest

INSTALLER = pathlib.Path(__file__).resolve().parents[1] / "packaging/install_linux_bundle.sh"


def installer_function(name):
    source = INSTALLER.read_text()
    matches = re.findall(rf"(?ms)^{re.escape(name)}\(\) \{{\n.*?^\}}\n", source)
    if len(matches) != 1:
        raise AssertionError(f"expected one installer function: {name}")
    return matches[0]


class LegacyCombinedStateTests(unittest.TestCase):
    def test_trusted_regular_and_generation_units_capture_without_errexit(self):
        function = installer_function("capture_legacy_combined_state")
        with tempfile.TemporaryDirectory(prefix="codex-info-installer-state-") as directory:
            root = pathlib.Path(directory)
            regular = root / "regular.service"
            regular.write_text("[Service]\nExecStart=/fixture/old\n")
            generation = root / "generation.service"
            generation.symlink_to(regular.name)
            # Literal pre-state oracle: flat active/enabled and generation
            # inactive/disabled are both valid trusted predecessors.
            for unit, probe_status, expected in [
                (regular, 0, "1 1 0\n"),
                (generation, 1, "0 0 1\n"),
            ]:
                with self.subTest(unit=unit.name):
                    script = f"""set -euo pipefail
legacy_combined_unit_destination={shlex.quote(str(unit))}
legacy_combined_record() {{ return 0; }}
validate_legacy_combined_enable_link() {{ return 0; }}
probe_legacy_combined_enabled() {{ return {probe_status}; }}
probe_active() {{ return {probe_status}; }}
safe_blocked() {{ exit 88; }}
{function}
capture_legacy_combined_state
printf '%s %s %s\\n' "$legacy_combined_enabled" "$legacy_combined_active" "$legacy_combined_generation"
"""
                    # Shell input is this repository function plus literal fixtures.
                    # Exclude caller PATH and non-interactive shell startup hooks.
                    result = subprocess.run(  # nosec B603 # fixed bash/env; repository function and literal fixture stdin.
                        ["/bin/bash", "--noprofile", "--norc", "-s"],
                        input=script,
                        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"},
                        check=False,
                        capture_output=True,
                        text=True,
                        timeout=3,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout, expected)


if __name__ == "__main__":
    unittest.main()

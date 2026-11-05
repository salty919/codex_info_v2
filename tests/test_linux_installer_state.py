"""Finite tests of installer state functions; no live services or profile writes."""

import hashlib
import json
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



class LegacyManifestOrderTests(unittest.TestCase):
    def invoke_reader(self, home, combined):
        prefix = r"""
set -euo pipefail
SCHEMA=codex-info-linux-bundle-v1
PRODUCT=codex_info
TARGET=x86_64-unknown-linux-gnu
COMPATIBILITY=glibc
manifest_destination="$HOME/.local/share/codex-info/manifest.json"
binary_destination="$HOME/.local/bin/codex_info"
recorder_binary_destination="$HOME/.local/bin/codex_info_recorder"
rest_binary_destination="$HOME/.local/bin/codex_info_rest"
installer_destination="$HOME/.local/libexec/codex-info-install.sh"
unit_destination="$HOME/.config/systemd/user/codex-info-recorder.service"
rest_unit_destination="$HOME/.config/systemd/user/codex-info-rest.service"
legacy_combined_unit_destination="$HOME/.config/systemd/user/codex-info.service"
update_service_destination="$HOME/.config/systemd/user/codex-info-update.service"
update_timer_destination="$HOME/.config/systemd/user/codex-info-update.timer"
"""
        name = "legacy_combined_record_at" if combined else "legacy_flat_record"
        invocation = name
        if combined:
            invocation += (
                ' "$manifest_destination" "$binary_destination"'
                ' "$installer_destination" "$legacy_combined_unit_destination"'
                ' "$update_service_destination" "$update_timer_destination"'
            )
        return subprocess.run(  # nosec B603 # fixed Bash/env; repository function and owned synthetic legacy-state arguments
            ["/bin/bash", "--noprofile", "--norc", "-s"],
            input=prefix + installer_function(name) + "\n" + invocation + "\n",
            env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "HOME": str(home)},
            capture_output=True, text=True, check=False, timeout=3,
        )

    def legacy_shape(self, *, combined, modes=False):
        with tempfile.TemporaryDirectory(prefix="legacy-member-order-") as directory:
            home = pathlib.Path(directory)
            names = ["codex_info", "install.sh",
                     "codex-info-update.service", "codex-info-update.timer"]
            names += (
                ["codex-info.service"] if combined else
                ["codex_info_recorder", "codex_info_rest",
                 "codex-info-recorder.service", "codex-info-rest.service"]
            )
            payload = {}
            paths = {}
            for name in names:
                data = ("literal owned predecessor " + name + "\n").encode()
                mode = (
                    0o755 if name in {
                        "codex_info", "codex_info_recorder", "codex_info_rest", "install.sh"
                    } else 0o644
                )
                if name == "install.sh":
                    path = home / ".local/libexec/codex-info-install.sh"
                elif name.startswith("codex_info"):
                    path = home / ".local/bin" / name
                else:
                    path = home / ".config/systemd/user" / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
                path.chmod(mode)
                paths[name] = path
                payload[name] = (data, mode)

            entries = []
            for name in sorted(payload, reverse=True):
                data, mode = payload[name]
                entry = {
                    "path": name, "size": len(data),
                    "sha256": hashlib.sha256(data).hexdigest(),
                }
                if modes:
                    entry["mode"] = mode
                entries.append(entry)
            document = {
                "schema": "codex-info-linux-bundle-v1",
                "product": "codex_info", "version": "1.0.48",
                "source_sha": "3" * 40, "run_id": "100", "run_attempt": 1,
                "target": "x86_64-unknown-linux-gnu",
                "compatibility": "glibc", "glibc_minimum": "2.31",
                "files": entries,
            }
            manifest = home / ".local/share/codex-info/manifest.json"
            manifest.parent.mkdir(parents=True, exist_ok=True)
            raw = json.dumps(document, indent=2).encode() + b"\n"
            manifest.write_bytes(raw)
            manifest.chmod(0o644)
            protected = home / "original-database"
            protected.write_bytes(b"literal preserved database and settings\n")

            accepted = self.invoke_reader(home, combined)
            self.assertEqual(
                accepted.returncode, 0, accepted.stdout + accepted.stderr
            )
            fields = accepted.stdout.strip().split("\t")
            expected_binary = "codex_info" if combined else "codex_info_recorder"
            self.assertEqual(fields, [
                "1.0.48", "3" * 40, hashlib.sha256(raw).hexdigest(),
                hashlib.sha256(payload[expected_binary][0]).hexdigest(), str(len(raw)),
            ])

            # Removing order rejection must not permit duplicate identities.
            duplicated = dict(document)
            duplicated["files"] = list(entries) + [dict(entries[0])]
            manifest.write_text(json.dumps(duplicated) + "\n")
            duplicate = self.invoke_reader(home, combined)
            self.assertNotEqual(duplicate.returncode, 0)
            self.assertIn("path is invalid", duplicate.stderr)

            # Same-size payload tampering must still fail its recorded digest.
            manifest.write_bytes(raw)
            original = payload[expected_binary][0]
            altered = bytes([original[0] ^ 1]) + original[1:]
            paths[expected_binary].write_bytes(altered)
            bad_digest = self.invoke_reader(home, combined)
            self.assertNotEqual(bad_digest.returncode, 0)
            self.assertIn("does not match manifest", bad_digest.stderr)
            self.assertEqual(paths[expected_binary].read_bytes(), altered)
            self.assertEqual(
                protected.read_bytes(), b"literal preserved database and settings\n"
            )
            for name, (data, mode) in payload.items():
                expected = altered if name == expected_binary else data
                self.assertEqual(paths[name].read_bytes(), expected)
                self.assertEqual(paths[name].stat().st_mode & 0o777, mode)

    def test_flat_order_keeps_unique_paths_and_member_digest(self):
        self.legacy_shape(combined=False)

    def test_combined_order_keeps_both_entry_schemas_and_member_digest(self):
        for modes in (False, True):
            with self.subTest(modes=modes):
                self.legacy_shape(combined=True, modes=modes)

if __name__ == "__main__":
    unittest.main()

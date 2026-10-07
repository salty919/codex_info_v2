#!/usr/bin/env bash
set -euo pipefail

# Exercise the production discovery code against real X11 window states.
ROOT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
[[ -n "${DISPLAY:-}" ]] || { echo 'graph-readiness-test: HOLD: DISPLAY is unavailable' >&2; exit 2; }
for command in python3 xwininfo xprop xwd; do
    command -v "$command" >/dev/null || { echo "graph-readiness-test: HOLD: $command is unavailable" >&2; exit 2; }
done

python3 - "$@" <<'PY'
import ctypes
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest


class XWindowAttributes(ctypes.Structure):
    _fields_ = [
        ("x", ctypes.c_int), ("y", ctypes.c_int),
        ("width", ctypes.c_int), ("height", ctypes.c_int),
        ("border_width", ctypes.c_int), ("depth", ctypes.c_int),
        ("visual", ctypes.c_void_p), ("root", ctypes.c_ulong),
        ("class_", ctypes.c_int), ("bit_gravity", ctypes.c_int),
        ("win_gravity", ctypes.c_int), ("backing_store", ctypes.c_int),
        ("backing_planes", ctypes.c_ulong), ("backing_pixel", ctypes.c_ulong),
        ("save_under", ctypes.c_int), ("colormap", ctypes.c_ulong),
        ("map_installed", ctypes.c_int), ("map_state", ctypes.c_int),
        ("all_event_masks", ctypes.c_long), ("your_event_mask", ctypes.c_long),
        ("do_not_propagate_mask", ctypes.c_long),
        ("override_redirect", ctypes.c_int), ("screen", ctypes.c_void_p),
    ]


class OwnedGraphWindow:
    def __init__(self):
        self.lib = ctypes.CDLL("libX11.so.6")
        bindings = {
            "XOpenDisplay": ([ctypes.c_char_p], ctypes.c_void_p),
            "XDefaultRootWindow": ([ctypes.c_void_p], ctypes.c_ulong),
            "XCreateSimpleWindow": ([ctypes.c_void_p, ctypes.c_ulong,
                                     ctypes.c_int, ctypes.c_int,
                                     ctypes.c_uint, ctypes.c_uint, ctypes.c_uint,
                                     ctypes.c_ulong, ctypes.c_ulong], ctypes.c_ulong),
            "XStoreName": ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_char_p], ctypes.c_int),
            "XInternAtom": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int], ctypes.c_ulong),
            "XChangeProperty": ([ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong,
                                 ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
                                 ctypes.POINTER(ctypes.c_ubyte), ctypes.c_int], ctypes.c_int),
            "XGetWindowAttributes": ([ctypes.c_void_p, ctypes.c_ulong,
                                      ctypes.POINTER(XWindowAttributes)], ctypes.c_int),
            "XMapWindow": ([ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
            "XDestroyWindow": ([ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
            "XSync": ([ctypes.c_void_p, ctypes.c_int], ctypes.c_int),
            "XCloseDisplay": ([ctypes.c_void_p], ctypes.c_int),
        }
        for name, (arguments, result) in bindings.items():
            function = getattr(self.lib, name)
            function.argtypes, function.restype = arguments, result
        self.display = self.lib.XOpenDisplay(None)
        if not self.display:
            raise RuntimeError("XOpenDisplay failed")
        root = self.lib.XDefaultRootWindow(self.display)
        self.window = self.lib.XCreateSimpleWindow(self.display, root,
                                                   0, 0, 940, 640, 0, 0, 0)
        self.lib.XStoreName(self.display, self.window, b"Codex Info Graph")
        pid_atom = self.lib.XInternAtom(self.display, b"_NET_WM_PID", 0)
        cardinal = self.lib.XInternAtom(self.display, b"CARDINAL", 0)
        pid = (ctypes.c_ulong * 1)(os.getpid())
        self.lib.XChangeProperty(self.display, self.window, pid_atom, cardinal,
                                32, 0, ctypes.cast(pid, ctypes.POINTER(ctypes.c_ubyte)), 1)
        self.lib.XSync(self.display, 0)

    def attributes(self):
        attributes = XWindowAttributes()
        if not self.lib.XGetWindowAttributes(self.display, self.window,
                                             ctypes.byref(attributes)):
            raise RuntimeError("XGetWindowAttributes failed")
        return attributes

    def map(self):
        self.lib.XMapWindow(self.display, self.window)
        self.lib.XSync(self.display, 0)

    def close(self):
        self.lib.XDestroyWindow(self.display, self.window)
        self.lib.XCloseDisplay(self.display)


class GraphWindowReadinessTests(unittest.TestCase):
    def setUp(self):
        self.window = OwnedGraphWindow()
        self.addCleanup(self.window.close)
        self.assertEqual(self.window.attributes().map_state, 0)
        temporary = tempfile.TemporaryDirectory(prefix="codex-info-graph-readiness-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        (self.directory / "client.log").write_text("", encoding="utf-8")
        self.marker = self.directory / "name-queries"
        tools = self.directory / "tools"
        tools.mkdir()
        # Forward the real property query unchanged. Its notifications let the
        # fixture release MapWindow after discovery has seen the named window
        # more than once; no delay is inserted into the capture command.
        xprop = tools / "xprop"
        xprop.write_text(
            f"#!{sys.executable}\n"
            "import pathlib, subprocess, sys\n"
            f"result = subprocess.run([{shutil.which('xprop')!r}, *sys.argv[1:]], capture_output=True)\n"
            "sys.stdout.buffer.write(result.stdout)\n"
            "sys.stderr.buffer.write(result.stderr)\n"
            "sys.stdout.flush()\n"
            "if result.returncode == 0 and sys.argv[-1] == 'WM_NAME':\n"
            f"    with pathlib.Path({str(self.marker)!r}).open('a') as marker:\n"
            "        marker.write('named\\n')\n"
            "sys.exit(result.returncode)\n",
            encoding="utf-8",
        )
        xprop.chmod(0o700)
        self.environment = dict(os.environ,
                                PATH=f"{tools}{os.pathsep}{os.environ['PATH']}",
                                LC_ALL="C",
                                TEST_PREVIEW_PID=str(os.getpid()),
                                TEST_TEMP_ROOT=str(self.directory))

    def discovery(self):
        source = Path("scripts/x11_graph_visual_gate.sh").read_text(encoding="utf-8")
        start = source.index('\ngraph_id=""\n')
        end = source.index('\ngraph_name=', start)
        program = (
            'set -euo pipefail\n'
            'fail() { printf "graph-readiness-test: FAIL: %s\\n" "$*" >&2; exit 1; }\n'
            'preview_pid="$TEST_PREVIEW_PID"\n'
            'temp_root="$TEST_TEMP_ROOT"\n'
            + source[start:end]
            + '\nprintf "%s\\n" "$graph_id"\n'
        )
        process = subprocess.Popen(["bash", "-c", program], env=self.environment,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

        def stop():
            if process.poll() is None:
                process.terminate()
                process.communicate(timeout=5)

        self.addCleanup(stop)
        return process

    def name_queries(self):
        return len(self.marker.read_text().splitlines()) if self.marker.exists() else 0

    def test_discovery_waits_for_viewable_graph(self):
        process = self.discovery()
        deadline = time.monotonic() + 10
        while process.poll() is None and self.name_queries() < 2:
            if time.monotonic() >= deadline:
                self.fail("discovery did not observe the named Graph within its startup bound")
            time.sleep(0.01)
        if process.poll() is None:
            self.assertEqual(self.window.attributes().map_state, 0)
            self.window.map()
        stdout, stderr = process.communicate(timeout=30)
        self.assertEqual(process.returncode, 0, stderr)
        self.assertEqual(int(stdout.strip(), 16), self.window.window)
        state = self.window.attributes().map_state
        capture = subprocess.run(
            [shutil.which("xwd"), "-silent", "-id", hex(self.window.window),
             "-out", str(self.directory / "graph.xwd")],
            capture_output=True, text=True,
        )
        self.assertEqual(state, 2,
                         f"discovery returned an unmapped Graph; xwd exit={capture.returncode}: "
                         f"{capture.stderr.strip()}")
        self.assertEqual(capture.returncode, 0, capture.stderr)
        self.assertGreater((self.directory / "graph.xwd").stat().st_size, 100)

    def test_never_mapped_graph_fails_within_existing_bound(self):
        process = self.discovery()
        stdout, stderr = process.communicate(timeout=30)
        self.assertNotEqual(process.returncode, 0, stdout)
        self.assertIn("graph preview window did not", stderr)
        self.assertEqual(self.name_queries(), 80)
        self.assertEqual(self.window.attributes().map_state, 0)


unittest.main()
PY

#!/usr/bin/env python3
"""Exercise the extracted Graph click path on a real Xvfb server."""

from __future__ import annotations

import ctypes
import os
import shutil
import subprocess  # nosec B404 # required fixture process API; the shell call is reviewed below.
import sys
import time
import unittest
from pathlib import Path
from unittest import mock

GATE = Path(__file__).with_name("x11_service_recovery_visual_gate.sh")
PRESS, RELEASE = 4, 5
PRESS_MASK, RELEASE_MASK = 1 << 2, 1 << 3
CLICK_X, CLICK_Y = 605, 40
HOLD_TEXT = "HOLD: click target not established"
GRAPH_FAIL_TEXT = "FAIL: Graph action did not open the real graph window"
EVENT_BYTES = ctypes.sizeof(ctypes.c_long) * 24


class SetupFailure(Exception):
    pass


def extract_function(lines: list[str], name: str) -> str:
    starts = [i for i, line in enumerate(lines) if line.startswith(f"{name}() {{")]
    if len(starts) != 1:
        raise SetupFailure(f"expected one {name}() definition, found {len(starts)}")
    start = starts[0]
    if lines[start].endswith("}"):
        return lines[start]
    end = next((i for i in range(start + 1, len(lines)) if lines[i] == "}"), None)
    if end is None:
        raise SetupFailure(f"{name}() definition has no closing brace")
    return "\n".join(lines[start : end + 1])


def source_fragments() -> tuple[str, str, str, str]:
    try:
        lines = GATE.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise SetupFailure(f"cannot read gate source: {error}") from error
    click = 'window_action "$window_id" 605 40 click'
    terminal = '[[ -n "$graph_window_id" ]] || fail \'Graph action did not open the real graph window\''
    starts = [i for i, line in enumerate(lines) if line == click]
    ends = [i for i, line in enumerate(lines) if line == terminal]
    if len(starts) != 1 or len(ends) != 1 or starts[0] >= ends[0]:
        raise SetupFailure(f"Graph caller extraction is ambiguous: click={len(starts)}, end={len(ends)}")
    return (
        extract_function(lines, "hold"),
        extract_function(lines, "fail"),
        extract_function(lines, "window_action"),
        "\n".join(lines[starts[0] : ends[0] + 1]),
    )


def x11_api():
    if not os.environ.get("DISPLAY"):
        raise SetupFailure("DISPLAY is unavailable")
    for command in ("bash", "xwininfo", "xprop", "awk", "tr", "seq"):
        if shutil.which(command) is None:
            raise SetupFailure(f"required command is unavailable: {command}")
    try:
        x11 = ctypes.CDLL("libX11.so.6")
        ctypes.CDLL("libXtst.so.6")
    except OSError as error:
        raise SetupFailure(f"required X11 library is unavailable: {error}") from error

    display, window = ctypes.c_void_p, ctypes.c_ulong
    x11.XOpenDisplay.argtypes, x11.XOpenDisplay.restype = [ctypes.c_char_p], display
    x11.XCloseDisplay.argtypes = [display]
    x11.XDefaultScreen.argtypes, x11.XDefaultScreen.restype = [display], ctypes.c_int
    x11.XRootWindow.argtypes, x11.XRootWindow.restype = [display, ctypes.c_int], window
    x11.XCreateSimpleWindow.argtypes = [
        display, window, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint,
        ctypes.c_uint, ctypes.c_ulong, ctypes.c_ulong,
    ]
    x11.XCreateSimpleWindow.restype = window
    x11.XSelectInput.argtypes = [display, window, ctypes.c_long]
    x11.XMapWindow.argtypes = [display, window]
    x11.XUnmapWindow.argtypes = [display, window]
    x11.XDestroyWindow.argtypes = [display, window]
    x11.XSync.argtypes = [display, ctypes.c_int]
    x11.XWarpPointer.argtypes = [
        display, window, window, ctypes.c_int, ctypes.c_int, ctypes.c_uint,
        ctypes.c_uint, ctypes.c_int, ctypes.c_int,
    ]
    x11.XCheckTypedWindowEvent.argtypes = [display, window, ctypes.c_int, ctypes.c_void_p]
    x11.XCheckTypedWindowEvent.restype = ctypes.c_int
    x11.XInternAtom.argtypes = [display, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = window
    x11.XChangeProperty.argtypes = [
        display, window, window, window, ctypes.c_int, ctypes.c_int,
        ctypes.POINTER(ctypes.c_ubyte), ctypes.c_int,
    ]
    x11.XSetErrorHandler.argtypes = [ctypes.c_void_p]
    return x11


def sync_setup(x11, display, errors: list[str], label: str) -> None:
    x11.XSync(display, 0)
    if errors:
        raise SetupFailure(f"X11 setup failed at {label}: {errors[0]}")


def set_pid_property(x11, display, window: int, pid: int, errors: list[str]) -> None:
    pid_atom = x11.XInternAtom(display, b"_NET_WM_PID", 0)
    cardinal = x11.XInternAtom(display, b"CARDINAL", 0)
    value = (ctypes.c_ulong * 1)(pid)
    x11.XChangeProperty(
        display, window, pid_atom, cardinal, 32, 0,
        ctypes.cast(value, ctypes.POINTER(ctypes.c_ubyte)), 1,
    )
    sync_setup(x11, display, errors, "setting _NET_WM_PID")


def make_fixture(x11, *, wrapper: bool, covered: bool = False, unmapped: bool = False) -> dict:
    display = x11.XOpenDisplay(None)
    if not display:
        raise SetupFailure("XOpenDisplay failed")
    errors: list[str] = []
    error_handler = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)

    @error_handler
    def record_x_error(_display, _event):
        errors.append("X server rejected a fixture request")
        return 0

    x11.XSetErrorHandler(ctypes.cast(record_x_error, ctypes.c_void_p))
    root = x11.XRootWindow(display, x11.XDefaultScreen(display))
    parent = x11.XCreateSimpleWindow(display, root, 100, 100, 1000, 600, 0, 0, 0) if wrapper else root
    main_x, main_y = (0, 0) if wrapper else (100, 100)
    main = x11.XCreateSimpleWindow(display, parent, main_x, main_y, 900, 498, 0, 0, 0)
    child = graph = 0
    watched = [(root, "root")]
    if wrapper:
        watched.append((parent, "wrapper"))
    watched.append((main, "Main"))
    if not main or (wrapper and not parent):
        raise SetupFailure("XCreateSimpleWindow failed")

    mask = PRESS_MASK | RELEASE_MASK
    for window, _name in watched:
        x11.XSelectInput(display, window, mask)
    if wrapper:
        x11.XMapWindow(display, parent)
    x11.XMapWindow(display, main)
    sync_setup(x11, display, errors, "mapping root/wrapper/Main")
    set_pid_property(x11, display, main, os.getpid(), errors)

    if covered:
        child = x11.XCreateSimpleWindow(display, main, 580, 20, 60, 50, 0, 0, 0)
        if not child:
            raise SetupFailure("XCreateSimpleWindow failed for covered child")
        x11.XSelectInput(display, child, mask)
        x11.XMapWindow(display, child)
        watched.append((child, "child"))
        sync_setup(x11, display, errors, "mapping covered child")
    if unmapped:
        x11.XWarpPointer(display, 0, main, 0, 0, 0, 0, CLICK_X, CLICK_Y)
        sync_setup(x11, display, errors, "positioning pointer before unmapping Main")
        x11.XUnmapWindow(display, main)
        sync_setup(x11, display, errors, "unmapping Main")
    return {
        "x11": x11, "display": display, "root": root, "wrapper": parent if wrapper else 0,
        "main": main, "child": child, "graph": graph, "watched": watched,
        "errors": errors, "error_handler": record_x_error,
    }


def create_graph_candidate(fixture: dict) -> None:
    x11, display = fixture["x11"], fixture["display"]
    graph = x11.XCreateSimpleWindow(display, fixture["root"], 300, 220, 200, 120, 0, 0, 0)
    if not graph:
        raise SetupFailure("XCreateSimpleWindow failed for Graph candidate")
    set_pid_property(x11, display, graph, os.getpid(), fixture["errors"])
    x11.XMapWindow(display, graph)
    sync_setup(x11, display, fixture["errors"], "mapping Graph candidate")
    fixture["graph"] = graph


def take_events(fixture: dict, events: list[tuple[str, int]], *, graph_on_press: bool) -> None:
    x11, display = fixture["x11"], fixture["display"]
    storage = ctypes.create_string_buffer(EVENT_BYTES)
    for window, name in fixture["watched"]:
        for kind in (PRESS, RELEASE):
            while x11.XCheckTypedWindowEvent(display, window, kind, storage):
                events.append((name, kind))
                if graph_on_press and not fixture["graph"] and name == "Main" and kind == PRESS:
                    create_graph_candidate(fixture)


def run_graph_path(
    sources: tuple[str, str, str, str], fixture: dict, *, graph_on_press: bool
) -> tuple[int, str, list[tuple[str, int]]]:
    hold, fail, helper, graph_block = sources
    shell = (
        "set -euo pipefail\n" + hold + "\n" + fail + "\n" + helper
        + f"\nwindow_id={hex(fixture['main'])}\nui_pid={os.getpid()}\ngraph_window_id=''\n"
        + graph_block + "\n"
    )
    # Intentionally execute repository Graph functions with generated numeric fixture identities.
    # Fixed bash and disabled startup hooks prevent caller executable/startup injection.
    environment = {key: value for key, value in os.environ.items() if key not in {"BASH_ENV", "ENV"}}
    try:
        # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit.dangerous-subprocess-use-audit
        process = subprocess.Popen(  # nosec B603 # fixed bash and intentional repository-only shell fixture.
            ["/bin/bash", "--noprofile", "--norc", "-c", shell, "x11-graph-path-test"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=environment, shell=False,
        )
    except OSError as error:
        raise SetupFailure(f"cannot run extracted Graph path: {error}") from error

    events: list[tuple[str, int]] = []
    deadline = time.monotonic() + 20
    while process.poll() is None:
        fixture["x11"].XSync(fixture["display"], 0)
        take_events(fixture, events, graph_on_press=graph_on_press)
        if time.monotonic() >= deadline:
            process.kill()
            process.communicate()
            raise SetupFailure("extracted Graph caller did not finish in 20 seconds")
        time.sleep(0.002)
    stdout, stderr = process.communicate()
    fixture["x11"].XSync(fixture["display"], 0)
    take_events(fixture, events, graph_on_press=graph_on_press)
    if fixture["errors"]:
        raise SetupFailure(f"X11 fixture failed while observing click: {fixture['errors'][0]}")
    return process.returncode, (stdout + stderr).strip(), events


def close_fixture(fixture: dict) -> None:
    x11, display = fixture["x11"], fixture["display"]
    if fixture["graph"]:
        x11.XDestroyWindow(display, fixture["graph"])
    target = fixture["wrapper"] or fixture["main"]
    x11.XDestroyWindow(display, target)
    x11.XSync(display, 0)
    x11.XCloseDisplay(display)


def event_text(events: list[tuple[str, int]]) -> str:
    names = {PRESS: "ButtonPress", RELEASE: "ButtonRelease"}
    return "[" + ", ".join(f"{window}:{names[kind]}" for window, kind in events) + "]"


def case(x11, sources, label: str, *, wrapper: bool, covered=False, unmapped=False, graph=False) -> tuple[bool, str]:
    fixture = make_fixture(x11, wrapper=wrapper, covered=covered, unmapped=unmapped)
    try:
        status, output, events = run_graph_path(sources, fixture, graph_on_press=graph)
        pair = [("Main", PRESS), ("Main", RELEASE)]
        if label in ("A covered child", "B unmapped Main"):
            passed = status == 2 and HOLD_TEXT in output and GRAPH_FAIL_TEXT not in output and not events
        elif label == "C direct-root Main":
            passed = status == 1 and GRAPH_FAIL_TEXT in output and events == pair
        else:
            passed = status == 0 and GRAPH_FAIL_TEXT not in output and HOLD_TEXT not in output and events == pair and bool(fixture["graph"])
        detail = f"rc={status}, output={output!r}, events={event_text(events)}"
        if label.startswith("D "):
            detail += f", graph_candidate={'created' if fixture['graph'] else 'missing'}"
        return passed, detail
    finally:
        close_fixture(fixture)


class GraphProcessBoundaryTests(unittest.TestCase):
    def graph_process_arguments(self):
        process = mock.Mock()
        process.poll.return_value = 0
        process.communicate.return_value = ("", "")
        process.returncode = 0
        fixture = {"main": 1, "x11": mock.Mock(), "display": None,
                   "watched": [], "graph": 0, "errors": []}
        with mock.patch.object(subprocess, "Popen", return_value=process) as spawn:
            run_graph_path(("", "", "", ""), fixture, graph_on_press=False)
        return spawn.call_args

    def test_graph_process_uses_fixed_bash(self):
        call = self.graph_process_arguments()
        self.assertEqual(call.args[0][:4], ["/bin/bash", "--noprofile", "--norc", "-c"])

    def test_graph_process_removes_startup_hooks(self):
        with mock.patch.dict(os.environ, {"BASH_ENV": "/untrusted/startup", "ENV": "/untrusted/profile"}):
            call = self.graph_process_arguments()
            environment = call.kwargs.get("env", os.environ)
            self.assertNotIn("BASH_ENV", environment)
            self.assertNotIn("ENV", environment)


def main() -> int:
    try:
        sources = source_fragments()
        x11 = x11_api()
    except SetupFailure as error:
        print(f"SETUP-FAIL: {error}", file=sys.stderr)
        return 2
    cases = (
        ("A covered child", dict(wrapper=True, covered=True)),
        ("B unmapped Main", dict(wrapper=True, unmapped=True)),
        ("C direct-root Main", dict(wrapper=False)),
        ("D wrapped Main with Graph", dict(wrapper=True, graph=True)),
    )
    failed = []
    for label, options in cases:
        try:
            passed, detail = case(x11, sources, label, **options)
        except SetupFailure as error:
            print(f"SETUP-FAIL: {error}", file=sys.stderr)
            return 2
        print(f"{'PASS' if passed else 'RED'} {label}: {detail}")
        if not passed:
            failed.append(label)
    if failed:
        print("Expected behavioral RED cases: " + ", ".join(failed), file=sys.stderr)
        return 1
    print("PASS: all four extracted Graph terminal and X event outcomes matched")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SetupFailure as error:
        print(f"SETUP-FAIL: {error}", file=sys.stderr)
        raise SystemExit(2)
    except Exception as error:
        print(f"SETUP-FAIL: unexpected fixture error: {error}", file=sys.stderr)
        raise SystemExit(2)

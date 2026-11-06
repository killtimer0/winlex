import hashlib
import json
import platform
import struct
import sys
import time
import importlib.metadata
from pathlib import Path

import pytest
import uiautomation as auto
import win32api
import win32con
import win32service
from support.config import ConfigProfile
from support.languages import LANGUAGES, language
from support.session import Session, invoke
from support.pe import is_64_bit
from support import native as n
from support.diagnostics import screenshot
from support.artifacts import finish_fixtures
from support.paths import extended


def pytest_addoption(parser):
    parser.addoption("--exe", required=True)
    parser.addoption("--winmd", default="inputs/Windows.Win32.winmd")
    parser.addoption("--artifacts", default="tmp/gui-tests")
    parser.addoption("--language", default="en-US", choices=[option.name for option in LANGUAGES])
    parser.addoption("--dpi", default="auto")
    parser.addoption("--real-input", action="store_true",
                     help="Run only interactive input/screen tests on the current desktop")


def pytest_configure(config):
    config.addinivalue_line("markers", "interactive: requires an exclusive active input desktop")


def pytest_collection_modifyitems(config, items):
    interactive = config.getoption("--real-input")
    selected, deselected = [], []
    for item in items:
        target = selected if bool(item.get_closest_marker("interactive")) == interactive else deselected
        target.append(item)
    items[:] = selected
    config.hook.pytest_deselected(items=deselected)


def _phase_started(item, phase):
    output = Path(item.config.getoption("--artifacts"))
    output.mkdir(parents=True, exist_ok=True)
    with (output / "progress.jsonl").open("a", encoding="utf-8") as stream:
        stream.write(json.dumps({"nodeid": item.nodeid, "phase": phase,
                                 "start": time.time(), "monotonic": time.monotonic()}) + "\n")


@pytest.hookimpl(tryfirst=True)
def pytest_runtest_setup(item):
    _phase_started(item, "setup")
    if not item.get_closest_marker("interactive"):
        return
    try:
        station = win32service.GetProcessWindowStation()
        station_name = win32service.GetUserObjectInformation(int(station), win32con.UOI_NAME)
        current = win32service.GetThreadDesktop(win32api.GetCurrentThreadId())
        active = win32service.OpenInputDesktop(0, False, win32con.DESKTOP_READOBJECTS)
        try:
            current_name = win32service.GetUserObjectInformation(int(current), win32con.UOI_NAME)
            active_name = win32service.GetUserObjectInformation(int(active), win32con.UOI_NAME)
        finally:
            active.CloseDesktop()
        if station_name.lower() != "winsta0" or current_name != active_name:
            pytest.fail("Interactive tests require the active input desktop", pytrace=False)
    except Exception as error:
        pytest.fail("Cannot access the active input desktop: %s" % error, pytrace=False)


@pytest.hookimpl(tryfirst=True)
def pytest_runtest_call(item):
    _phase_started(item, "call")


@pytest.hookimpl(tryfirst=True)
def pytest_runtest_teardown(item):
    _phase_started(item, "teardown")


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    output = Path(item.config.getoption("--artifacts"))
    output.mkdir(parents=True, exist_ok=True)
    with (output / "timings.jsonl").open("a", encoding="utf-8") as stream:
        stream.write(json.dumps({"nodeid": report.nodeid, "phase": report.when,
                                 "outcome": report.outcome, "duration": report.duration,
                                 "start": report.start, "stop": report.stop}) + "\n")
    if report.when == "call":
        item.test_failed = report.failed
        if report.failed:
            (output / (item.name + "-failure.txt")).write_text(report.longreprtext, encoding="utf-8")


@pytest.fixture(scope="session")
def settings(request):
    if hasattr(n.user, "SetThreadDpiAwarenessContext"):
        n.api(n.user, "SetThreadDpiAwarenessContext", n.c.c_void_p, n.c.c_void_p)(n.c.c_void_p(-4))
    option = request.config.getoption
    exe, winmd = Path(option("--exe")).resolve(), Path(option("--winmd")).resolve()
    output = Path(option("--artifacts")).resolve()
    output.mkdir(parents=True, exist_ok=True)
    version = sys.getwindowsversion()
    info = {"exe": str(exe), "winmd": str(winmd), "os": platform.platform(),
             "windows_version": {"major": version.major, "minor": version.minor, "build": version.build},
             "python": sys.version, "python_executable": sys.executable, "python_bits": struct.calcsize("P") * 8,
             "dependencies": {name: importlib.metadata.version(name)
                              for name in ("pytest", "pywin32", "uiautomation", "comtypes")},
            "hashes": {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (exe, winmd)},
            "language": option("--language"), "dpi": option("--dpi"), "capabilities": n.capabilities()}
    info["exe_bits"] = 64 if is_64_bit(exe) else 32
    info["test_group"] = "interactive" if option("--real-input") else "contracts"
    info["desktop"] = win32service.GetUserObjectInformation(
        int(win32service.GetThreadDesktop(win32api.GetCurrentThreadId())), win32con.UOI_NAME)
    (output / "environment.json").write_text(json.dumps(info, indent=2), encoding="utf-8")
    yield exe, winmd, output
    finish_fixtures(extended(output), extended(winmd), info["hashes"][str(winmd)])


@pytest.fixture
def ui_language(request):
    return language(request.config.getoption("--language"))


@pytest.fixture
def profile(request):
    with ConfigProfile(request.config.getoption("--language")) as value:
        yield value


@pytest.fixture
def launch(request, settings, profile):
    sessions = []
    def start(arguments=(), dpi=None, *, metadata_prompt=False):
        session = Session(settings[0], profile, arguments, dpi or request.config.getoption("--dpi"))
        sessions.append(session)
        if not metadata_prompt and not arguments and profile.get("Version") == 1 and profile.get("AutoLoad") in (None, []):
            prompt = auto.WindowControl(searchDepth=3, ClassName="#32770", ProcessId=session.process.pid)
            assert prompt.Exists(10), "empty-startup metadata guide prompt"
            invoke(prompt.ButtonControl(AutomationId=str(win32con.IDNO)))
            n.wait(lambda: not prompt.Exists(0, 0), "metadata guide prompt dismissed")
        return session
    yield start
    errors = []
    for index, session in enumerate(sessions):
        try:
            if session.process.poll() is None and getattr(request.node, "test_failed", False):
                try:
                    screenshot(session.frame, settings[2] / (request.node.name + "-%d.png" % index))
                except Exception as error:
                    (settings[2] / (request.node.name + "-capture-error.txt")).write_text(str(error), encoding="utf-8")
                session.diagnostics(settings[2] / (request.node.name + "-%d-windows.json" % index))
        except Exception as error:
            errors.append(str(error))
        finally:
            try:
                session.close()
            except Exception as error:
                errors.append(str(error))
            (settings[2] / (request.node.name + "-%d-process.json" % index)).write_text(json.dumps({
                "pid": session.process.pid, "exit_code": session.process.poll(),
                "actual_dpi": session.actual_dpi, "cleanup_complete": session.cleanup_complete}), encoding="utf-8")
    (settings[2] / (request.node.name + "-cleanup.json")).write_text(json.dumps({
        "cleanup_complete": not errors and all(s.cleanup_complete for s in sessions),
        "errors": errors,
    }), encoding="utf-8")
    assert not errors, errors

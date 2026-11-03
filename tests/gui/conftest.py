import hashlib
import json
import platform
import struct
import sys
import importlib.metadata
from pathlib import Path

import pytest
from support.config import ConfigProfile
from support.session import Session
from support.pe import is_64_bit
from support import native as n
from support.diagnostics import screenshot
from support.paths import extended


def pytest_addoption(parser):
    parser.addoption("--exe", required=True)
    parser.addoption("--winmd", default="inputs/Windows.Win32.winmd")
    parser.addoption("--artifacts", default="tmp/gui-tests")
    parser.addoption("--language", default="en-US", choices=("en-US", "zh-CN"))
    parser.addoption("--dpi", default="auto")
    parser.addoption("--real-input", action="store_true")


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    if report.when == "call":
        item.test_failed = report.failed


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
            "dependencies": {name: importlib.metadata.version(name) for name in ("pytest", "pywin32")},
            "hashes": {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (exe, winmd)},
            "language": option("--language"), "dpi": option("--dpi"), "capabilities": n.capabilities()}
    info["exe_bits"] = 64 if is_64_bit(exe) else 32
    (output / "environment.json").write_text(json.dumps(info, indent=2), encoding="utf-8")
    yield exe, winmd, output
    root = extended(output)
    hashes = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob("*.winmd")}
    (output / "fixture-hashes.json").write_text(json.dumps(hashes, indent=2), encoding="utf-8")


@pytest.fixture
def profile(request):
    with ConfigProfile(request.config.getoption("--language")) as value:
        yield value


@pytest.fixture
def launch(request, settings, profile):
    sessions = []
    def start(arguments=(), dpi=None):
        session = Session(settings[0], profile, arguments, dpi or request.config.getoption("--dpi"),
                          real_input=request.config.getoption("--real-input"))
        sessions.append(session)
        return session
    yield start
    errors = []
    for index, session in enumerate(sessions):
        try:
            if session.process.poll() is None:
                session.diagnostics(settings[2] / (request.node.name + "-%d-windows.json" % index))
                if getattr(request.node, "test_failed", False):
                    try:
                        screenshot(session.frame, settings[2] / (request.node.name + "-%d.bmp" % index))
                    except Exception as error:
                        (settings[2] / (request.node.name + "-capture-error.txt")).write_text(str(error), encoding="utf-8")
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
    (settings[2] / (request.node.name + "-cleanup.json")).write_text(json.dumps({"cleanup_complete": not errors and all(s.cleanup_complete for s in sessions), "errors": errors}), encoding="utf-8")
    assert not errors, errors

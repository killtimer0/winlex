import hashlib
import json
import os
import platform
import struct
import sys
import importlib.metadata
from pathlib import Path

import pytest
from support import deadline, lifecycle
import win32api
import win32con
import win32service
from support.config import ConfigProfile
from support.languages import LANGUAGES, language
from support.pe import is_64_bit
from support.artifacts import finish_fixtures
from support.paths import extended


def pytest_addoption(parser):
    parser.addoption("--exe", required=True)
    parser.addoption("--winmd", default="inputs/Windows.Win32.winmd")
    parser.addoption("--artifacts", default="tmp/gui-tests")
    parser.addoption("--language", default="en-US", choices=[option.name for option in LANGUAGES])
    parser.addoption("--dpi", default="auto")
    parser.addoption('--hc-theme', help='Explicit .theme appearance expected by real high-contrast tests')
    parser.addoption("--real-input", action="store_true",
                     help="Run only interactive input/screen tests on the current desktop")


def pytest_configure(config):
    config.pluginmanager.register(lifecycle, 'winlex-lifecycle')
    config.addinivalue_line("markers", "interactive: requires an exclusive active input desktop")
    config.addinivalue_line('markers', 'phase_timeout(seconds): maximum call duration for a long behavioral scenario')
    output = Path(config.getoption('--artifacts'))
    output.mkdir(parents=True, exist_ok=True)
    deadline.configure(output)
    deadline.phase('collection', 'collection', 30)


def pytest_unconfigure(config):
    deadline.close()


@pytest.hookimpl(hookwrapper=True, tryfirst=True)
def pytest_collection_modifyitems(config, items):
    interactive = config.getoption("--real-input")
    selected, deselected = [], []
    excluded_path = os.environ.get('WINLEX_TEST_EXCLUDE')
    excluded = set(json.loads(Path(excluded_path).read_text(encoding='utf-8'))) if excluded_path else set()
    for item in items:
        target = selected if (bool(item.get_closest_marker("interactive")) == interactive
                              and item.nodeid not in excluded) else deselected
        target.append(item)
    items[:] = selected
    config.hook.pytest_deselected(items=deselected)
    # Record the final selection after pytest's -k/-m and other collection
    # filters, not the larger interactive/noninteractive candidate set.
    yield
    (Path(config.getoption('--artifacts')) / 'selection.json').write_text(
        json.dumps([item.nodeid for item in items]), encoding='utf-8')


@pytest.fixture(scope="session")
def automation_client():
    # CUIAutomation8 (Windows 8+) lets the client avoid implicit foreground
    # requests when interacting with controls on an isolated desktop.
    if sys.getwindowsversion() < (6, 2):
        yield
        return
    import comtypes.client
    import uiautomation.uiautomation as implementation

    client = implementation._AutomationClient.instance()
    core = client.UIAutomationCore
    modern = comtypes.client.CreateObject(core.CUIAutomation8, interface=core.IUIAutomation2)
    modern.AutoSetFocus = False
    modern.ConnectionTimeout = 2000
    modern.TransactionTimeout = 5000
    assert not modern.AutoSetFocus
    automation = modern.QueryInterface(core.IUIAutomation)
    walker = automation.RawViewWalker
    previous = client.IUIAutomation, client.ViewWalker
    client.IUIAutomation, client.ViewWalker = automation, walker
    try:
        yield
    finally:
        client.IUIAutomation, client.ViewWalker = previous


@pytest.fixture(scope="session")
def settings(request, automation_client):
    from support import native as n

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
def picker_environment(request, settings):
    from support.picker_environment import require_picker_environment

    node = hashlib.sha256(request.node.nodeid.encode('utf-8')).hexdigest()[:16]
    require_picker_environment(settings[2] / ('picker-environment-%s.json' % node))


@pytest.fixture
def hc_appearance(request, settings):
    from test_theme_compatibility import read_high_contrast_appearance

    path = request.config.getoption('--hc-theme')
    if not path:
        pytest.fail('Real high-contrast tests require an explicit --hc-theme file', pytrace=False)
    appearance = read_high_contrast_appearance(Path(path))
    (settings[2] / 'hc-appearance-fixture.json').write_text(
        json.dumps(appearance, indent=2), encoding='utf-8')
    return appearance


@pytest.fixture
def ui_language(request):
    return language(request.config.getoption("--language"))


@pytest.fixture
def profile(request):
    with ConfigProfile(request.config.getoption("--language")) as value:
        yield value


@pytest.fixture
def launch(request, settings, profile):
    from support import native as n
    from support.diagnostics import screenshot
    from support.session import Session, invoke, message_button

    sessions = []
    def start(arguments=(), dpi=None, *, metadata_prompt=False):
        session = Session(settings[0], profile, arguments, dpi or request.config.getoption("--dpi"))
        sessions.append(session)
        if not metadata_prompt and not arguments and profile.get("Version") == 1 and profile.get("AutoLoad") in (None, []):
            prompt = n.wait(lambda: n.find_window(session.process.pid, '#32770', session.frame),
                            'empty-startup metadata guide prompt', 10)
            invoke(n.wait(lambda: message_button(prompt, win32con.IDNO), 'metadata prompt No button'))
            n.wait(lambda: not n.is_window(prompt), "metadata guide prompt dismissed")
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

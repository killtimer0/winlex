"""Explicit desktop authorization and per-test process/config isolation."""
import json
import os
from pathlib import Path
import subprocess
import struct
import sys
import time
import uuid

import pytest
from verify_faults import FAULT_MODES


def pytest_addoption(parser):
    group = parser.getgroup('winlex')
    group.addoption('--exe', help='Debug GUI executable')
    group.addoption('--winmd', help='Metadata input')
    group.addoption('--output', default='tmp/gui-results', help='Evidence directory')
    group.addoption('--desktop-authorized', action='store_true', help='Authorize this input desktop')
    group.addoption('--real-input', action='store_true', help='Include exclusive desktop input scenarios')
    group.addoption('--fault-mode', choices=FAULT_MODES,
                    help='Run one infrastructure fault probe')


def pytest_configure(config):
    if config.option.collectonly:
        return
    if not config.getoption('--desktop-authorized'):
        raise pytest.UsageError('GUI execution requires explicit desktop authorization; use tests/gui/run.py')
    if os.name != 'nt':
        raise pytest.UsageError('GUI tests require Windows')
    for name in ('--exe', '--winmd'):
        value = config.getoption(name)
        if not value or not Path(value).is_file():
            raise pytest.UsageError('{} must name an existing file'.format(name))
    # Reject service/SSH window stations and locked or non-input desktops.
    import win32con
    from execute import require_interactive_desktop
    try:
        require_interactive_desktop()
    except (OSError, RuntimeError) as error:
        raise pytest.UsageError(str(error)) from error
    import win32event
    mutex = win32event.CreateMutex(None, False, 'Local\\Winlex.GuiTests.Desktop')
    if win32event.WaitForSingleObject(mutex, 0) not in (win32con.WAIT_OBJECT_0, win32con.WAIT_ABANDONED):
        mutex.Close()
        raise pytest.UsageError('Another GUI test run owns this desktop')
    config._winlex_desktop_mutex = mutex
    from progress import CaseProgress
    config.pluginmanager.register(CaseProgress(config.getoption('--output')), 'winlex-progress')


def pytest_unconfigure(config):
    mutex = getattr(config, '_winlex_desktop_mutex', None)
    if mutex is not None:
        import win32event
        win32event.ReleaseMutex(mutex)
        mutex.Close()


def pytest_collection_modifyitems(config, items):
    for item in items:
        if 'real_input' in item.keywords and not config.getoption('--real-input'):
            item.add_marker(pytest.mark.skip(reason='use --real-input on an exclusive desktop'))


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    report = (yield).get_result()
    if report.failed:
        app = item.funcargs.get('app')
        if app is not None:
            app.evidence(app.output / 'failure')


@pytest.fixture
def app(request):
    import win32con
    import win32event
    import win32job
    import win32process
    import winreg
    from driver import Winlex
    from execute import process_identity, remove_private_config

    output = Path(request.config.getoption('--output')).resolve() / request.node.name
    output.mkdir(parents=True, exist_ok=True)
    config_root = 'Software\\Winlex\\E2E\\' + uuid.uuid4().hex
    environment = dict(os.environ, WINLEX_CONFIG_ROOT=config_root)
    executable = str(Path(request.config.getoption('--exe')).resolve())
    metadata = str(Path(request.config.getoption('--winmd')).resolve())
    inputs = [metadata]
    job = win32job.CreateJobObject(None, 'Winlex.Gui.' + uuid.uuid4().hex)
    limits = win32job.QueryInformationJobObject(job, win32job.JobObjectExtendedLimitInformation)
    limits['BasicLimitInformation']['LimitFlags'] = win32job.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    win32job.SetInformationJobObject(job, win32job.JobObjectExtendedLimitInformation, limits)
    process = thread = None
    started = time.monotonic()
    pid = None
    created = None
    cleaned = True
    assigned = False
    app = None

    def launch(empty=False):
        nonlocal process, thread, pid, created, cleaned, assigned, app
        assigned = False
        pid = created = None
        # A hard stop during launch must not be reported as an empty, clean run.
        (output / 'process.json').write_text(json.dumps(dict(
            pid=None, created=None, config_root=config_root,
            cleanup_complete=False)), encoding='utf-8')
        process, thread, pid, _ = win32process.CreateProcess(
            executable, subprocess.list2cmdline([executable, *map(str, inputs)]), None, None, False,
            win32con.CREATE_SUSPENDED | win32con.CREATE_UNICODE_ENVIRONMENT
            | (subprocess.CREATE_BREAKAWAY_FROM_JOB if sys.getwindowsversion()[:2] < (6, 2) else 0),
            environment, str(Path(executable).parent), win32process.STARTUPINFO())
        cleaned = False
        # No metadata queries or I/O in the suspended, not-yet-contained interval.
        win32job.AssignProcessToJobObject(job, process)
        assigned = True
        created = process_identity(pid)
        (output / 'process.json').write_text(json.dumps(dict(
            pid=pid, created=created, config_root=config_root,
            cleanup_complete=False)), encoding='utf-8')
        win32process.ResumeThread(thread)
        replacement = Winlex(pid, output)
        if app is None:
            app = replacement
        else:
            app.pid, app.frame = replacement.pid, replacement.frame
        if not empty:
            app.ready()

    def restart(paths=None, empty=False):
        nonlocal process, thread, cleaned, inputs
        app.record('restart with saved preferences')
        app.close()
        assert win32event.WaitForSingleObject(process, 10000) == win32con.WAIT_OBJECT_0
        cleaned = True
        # Generations never overlap. Keep containment and preferences, but record
        # the replacement identity before allowing it to execute application code.
        for handle in (thread, process):
            handle.Close()
        process = thread = None
        if paths is not None:
            inputs = list(paths)
        launch(empty=empty)

    try:
        # Seed only once: restarting must exercise preferences saved by the GUI.
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER, config_root) as key:
            winreg.SetValueEx(key, 'Language', 0, winreg.REG_SZ, 'en-US')
        launch()
        app.restart = restart
        yield app
    except BaseException:
        if app is not None:
            app.evidence(output / 'failure')
        raise
    finally:
        try:
            if process is not None:
                # Job termination is per-test containment, never a global process-name kill.
                if assigned:
                    win32job.TerminateJobObject(job, 1)
                elif win32event.WaitForSingleObject(process, 0) != win32con.WAIT_OBJECT_0:
                    win32process.TerminateProcess(process, 1)
                cleaned = win32event.WaitForSingleObject(process, 10000) == win32con.WAIT_OBJECT_0
                if not cleaned:
                    pytest.exit('GUI process cleanup failed; stop the desktop batch', returncode=2)
        finally:
            for handle in (thread, process, job):
                if handle is not None:
                    handle.Close()
            if cleaned:
                remove_private_config(config_root)
            (output / 'process.json').write_text(json.dumps(dict(
                pid=pid, created=created, config_root=config_root, cleanup_complete=cleaned,
                elapsed=time.monotonic() - started)), encoding='utf-8')


@pytest.fixture
def metadata(request):
    return Path(request.config.getoption('--winmd')).resolve()


@pytest.fixture
def ui_text(request):
    """Read the executable's English resources, matching the private preference."""
    import win32api
    library = win32api.LoadLibraryEx(str(Path(request.config.getoption('--exe')).resolve()), 0, 2)
    def text(identifier):
        block = win32api.LoadResource(library, 6, identifier // 16 + 1, 0x409)
        offset = 0
        for index in range(16):
            length = struct.unpack_from('<H', block, offset)[0] * 2
            offset += 2
            if index == identifier % 16:
                return block[offset:offset + length].decode('utf-16-le')
            offset += length
    try:
        yield text
    finally:
        win32api.FreeLibrary(library)

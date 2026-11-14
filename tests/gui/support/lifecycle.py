"""Global per-case progress, deadlines and retained failure evidence."""
import json
from pathlib import Path
import time
import pytest
import win32api
import win32con
import win32service
from . import deadline


def _phase_started(item, phase):
    output = Path(item.config.getoption('--artifacts'))
    marker = item.get_closest_marker('phase_timeout')
    budget = marker.args[0] if marker and phase == 'call' else {'setup': 30, 'call': 120, 'teardown': 30}[phase]
    deadline.phase(item.nodeid, phase, budget)
    with (output / 'progress.jsonl').open('a', encoding='utf-8') as stream:
        stream.write(json.dumps({'nodeid': item.nodeid, 'phase': phase,
                                 'start': time.time(), 'monotonic': time.monotonic()}) + '\n')


@pytest.hookimpl(tryfirst=True)
def pytest_runtest_setup(item):
    _phase_started(item, 'setup')
    if not item.get_closest_marker('interactive'):
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
        if station_name.lower() != 'winsta0' or current_name != active_name:
            pytest.fail('Interactive tests require the active input desktop', pytrace=False)
    except Exception as error:
        pytest.fail('Cannot access the active input desktop: %s' % error, pytrace=False)


@pytest.hookimpl(tryfirst=True)
def pytest_runtest_call(item):
    _phase_started(item, 'call')


@pytest.hookimpl(tryfirst=True)
def pytest_runtest_teardown(item):
    _phase_started(item, 'teardown')


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    output = Path(item.config.getoption('--artifacts'))
    with (output / 'timings.jsonl').open('a', encoding='utf-8') as stream:
        stream.write(json.dumps({'nodeid': report.nodeid, 'phase': report.when,
                                 'outcome': report.outcome, 'duration': report.duration,
                                 'start': report.start, 'stop': report.stop,
                                 'message': report.longreprtext if not report.passed else ''}) + '\n')
    if report.when == 'call':
        item.test_failed = report.failed
        if report.failed:
            captured = ''.join('\n\n%s\n%s' % section for section in report.sections)
            (output / (item.name + '-failure.txt')).write_text(report.longreprtext + captured, encoding='utf-8')

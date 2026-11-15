"""Publish phase transitions/results into the worker's one event journal.

Deadline owns the journal and diagnostics: reporting a result ends the active
phase, rather than leaving an old deadline live until the next phase starts.
Failure attachments remain separate evidence, not another progress ledger.
"""
from pathlib import Path
import pytest
import win32api
import win32con
import win32service
from . import deadline
from .policy import Policy


def _phase_started(item, phase):
    policy = Policy.load()
    marker = item.get_closest_marker('phase_timeout')
    budget = marker.args[0] if marker and phase == 'call' else policy.phase(phase)
    deadline.phase(item.nodeid, phase, budget)


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


def pytest_collectreport(report):
    if not report.passed:
        deadline.collection_result(report.nodeid, report.outcome, str(report.longrepr))


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    output = Path(item.config.getoption('--artifacts'))
    deadline.result(report.nodeid, report.when, report.outcome, report.duration,
                    report.start, report.stop, report.longreprtext if not report.passed else '')
    if report.when == 'call':
        item.test_failed = report.failed
        if report.failed:
            captured = ''.join('\n\n%s\n%s' % section for section in report.sections)
            (output / (item.name + '-failure.txt')).write_text(
                report.longreprtext + captured, encoding='utf-8', errors='backslashreplace')

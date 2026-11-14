"""Exercise timeout diagnostics and case failure behavior through the real runner."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
import xml.etree.ElementTree as ET

import gui_probe


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.name == 'nt' and importlib.util.find_spec('uiautomation'),
                     'requires the Windows GUI test dependencies')
class GuiDeadlineContracts(unittest.TestCase):
    def test_probe_collection_ignores_disappearing_neighbor_directories(self):
        stop = threading.Event()

        def churn():
            while not stop.is_set():
                neighbors = [Path(tempfile.mkdtemp(prefix='collection-neighbor-', dir=ROOT / 'tmp'))
                             for _ in range(32)]
                for neighbor in neighbors:
                    neighbor.rmdir()

        thread = threading.Thread(target=churn)
        thread.start()
        try:
            with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                self.run_probe(Path(directory), '''
def test_first_deadline_contract_probe():
    pass
''', exit_code=0)
        finally:
            stop.set()
            thread.join(timeout=5)
        self.assertFalse(thread.is_alive())

    def test_native_held_gil_hang_retains_stack_and_safe_continuation(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'continued-after-native-hang'
            output, result = self.run_probe(folder, '''
import ctypes
from pathlib import Path
from support import deadline
def blocked_native_with_gil():
    sleep = ctypes.PyDLL('kernel32').Sleep
    sleep.argtypes = [ctypes.c_ulong]
    sleep.restype = None
    sleep(60000)
def test_first_deadline_contract_probe():
    with deadline.operation('native call holding the GIL', .1):
        blocked_native_with_gil()
def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertTrue(sentinel.exists())
            self.assertTrue(result['cleanup_complete'])
            self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=0, skipped=0))
            stack = (output / 'timeout-stacks.log').read_text()
            self.assertIn('blocked_native_with_gil', stack)

    def supervise_probe(self, folder, script, timeout=5):
        started = time.monotonic()
        output, safe, remote = gui_probe.supervise(folder, script, timeout=timeout)
        self.assertFalse(safe, (folder / 'wrapper.log').read_text(encoding='utf-8'))
        self.assertLess(time.monotonic() - started, 15)
        status = remote.rpc(folder, {'action': 'status', 'run_id': '0' * 32})
        self.assertFalse(status['cleanup_complete'])
        self.assertFalse(status['worker_available'])
        return output, status

    def test_real_runner_cleanup_stall_is_forced_and_never_claims_clean(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            probe = folder / 'test_probe.py'
            probe.write_text('def test_deadline_contract_probe():\n    pass\n', encoding='utf-8')
            output, status = self.supervise_probe(folder, '''
import time
import run
original_phase = run.operation_deadline.phase
def phase(nodeid, name, seconds):
    original_phase(nodeid, name, .2 if name == 'cleanup' else seconds)
run.operation_deadline.phase = phase
run.win32api.CloseHandle = lambda handle: time.sleep(60)
sys.argv = ['run.py', '--output', sys.argv[1], '--exe', sys.executable, %r,
            '--ignore-glob=*', '-k', 'deadline_contract_probe', '-q']
raise SystemExit(run.main())
''' % str(probe))
            self.assertEqual(status['state'], 'supervisor_failed', status)
            self.assertTrue(status['runner_terminated'], status)
            self.assertFalse(status['cleanup_complete'], status)
            self.assertFalse(status['worker_available'], status)

    def test_denied_input_mutex_never_authorizes_ledger_recovery(self):
        import win32api
        import win32con
        import win32event
        mutex = win32event.CreateMutex(None, False, 'Local\\Winlex.Tests.Input')
        owned = win32event.WaitForSingleObject(mutex, 0) in (win32con.WAIT_OBJECT_0, win32con.WAIT_ABANDONED)
        if not owned:
            win32api.CloseHandle(mutex)
            self.skipTest('another test owns the input mutex')
        try:
            with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                folder = Path(directory)
                output, _ = self.supervise_probe(folder, '''
import json
from pathlib import Path
import run
output = Path(sys.argv[1])
(output / 'profiles').mkdir()
ledger = output / 'profiles' / 'input.json'
ledger.write_text(json.dumps({'held': [], 'foreground': 0, 'cursor': [0, 0]}))
sys.argv = ['run.py', '--output', str(output), '--desktop', 'current', '--real-input']
raise SystemExit(run.main())
''')
                ledger = output / 'profiles' / 'input.json'
                self.assertTrue(ledger.exists(), 'denied ownership must not consume the ledger')
                self.assertEqual(json.loads(ledger.read_text(encoding='utf-8')),
                                 {'held': [], 'foreground': 0, 'cursor': [0, 0]})
                report = gui_probe.read_run(output)
                self.assertIn('Another test owns input', report['error'])
                self.assertFalse(report['cleanup_complete'])
        finally:
            win32event.ReleaseMutex(mutex)
            win32api.CloseHandle(mutex)

    def test_cleanup_cancellation_and_original_budget_gate_continuation(self):
        for cancel in (True, False):
            with self.subTest(cancel=cancel), tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                folder = Path(directory)
                probe = folder / 'test_cleanup_gate.py'
                sentinel = folder / 'unexpected-following-test'
                probe.write_text('''
import time
from pathlib import Path
from support import deadline
def test_first_deadline_contract_probe():
    with deadline.operation('blocked before cleanup gate', .1):
        time.sleep(60)
def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), encoding='utf-8')
                output = folder / 'artifacts'
                process = gui_probe.run_wrapper(folder, '''
import time
from pathlib import Path
import run
original_close = run.win32api.CloseHandle
first = True
def close(handle):
    global first
    if first:
        first = False
        if %r:
            Path(%r).touch()
        else:
            time.sleep(3)
    original_close(handle)
run.win32api.CloseHandle = close
raise SystemExit(run.main())
''' % (cancel, str(output / 'cancel')), [
                    '--output', str(output), '--timeout', '2',
                    '--exe', sys.executable, str(probe), '--ignore-glob=*',
                    '-k', 'deadline_contract_probe', '-q',
                ])
                self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
                report = gui_probe.read_run(output)
                self.assertTrue(report['cleanup_complete'], report)
                self.assertEqual(report['cancelled'], cancel, report)
                self.assertFalse(sentinel.exists())
                self.assertFalse((output / 'continued').exists())

    def run_probe(self, folder, code, cleanup_complete=True, timeout=10, exit_code=None):
        output, process = gui_probe.run_probe(folder, code, timeout=timeout)
        if exit_code is None:
            self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
        else:
            self.assertEqual(process.returncode, exit_code, process.stdout + process.stderr)
        self.assertTrue((output / 'run.json').exists(),
                        'Runner exit %s without report:\n%s%s' %
                        (process.returncode, process.stdout, process.stderr))
        result = gui_probe.read_run(output)
        self.assertEqual(result['cleanup_complete'], cleanup_complete,
                         '%r\n%s%s' % (result, process.stdout, process.stderr))
        return output, result

    def test_blocked_callback_is_terminated_at_operation_deadline_with_stack(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            preceding = folder / 'preceding-test-executions'
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import native as n

def test_before_deadline_contract_probe():
    with Path(%r).open('a') as stream:
        stream.write('once')

def test_first_deadline_contract_probe():
    n.wait(lambda: time.sleep(60), 'blocked callback contract', timeout=.5)

def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % (str(preceding), str(sentinel)))
            self.assertTrue(result['timed_out'], result)
            self.assertEqual(result['timeout']['operation'], 'blocked callback contract')
            self.assertEqual(result['timeout']['budget'], .5)
            stack = (output / 'timeout-stacks.log').read_text(encoding='utf-8')
            self.assertIn('test_first_deadline_contract_probe', stack)
            self.assertTrue(sentinel.exists(), 'blocked case prevented following test from running')
            self.assertEqual(preceding.read_text(), 'once', 'completed case ran again after restart')
            suite = ET.parse(output / 'tests.xml').getroot().find('testsuite')
            self.assertEqual(suite.get('tests'), '3')
            self.assertEqual(suite.get('failures'), '1')

    def test_case_failure_does_not_stop_following_test(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            _, result = self.run_probe(folder, '''
from pathlib import Path

def test_first_deadline_contract_probe():
    raise AssertionError('intentional contract failure')

def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertFalse(result['timed_out'], result)
            self.assertEqual(result['exit_code'], 1)
            self.assertTrue(sentinel.exists(), 'a case failure stopped subsequent tests')

    def test_late_success_is_a_case_failure_and_following_test_runs(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline

def test_first_deadline_contract_probe():
    with deadline.operation('late successful callback', .1):
        time.sleep(.2)

def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertEqual(result['exit_code'], 1)
            self.assertTrue(sentinel.exists())
            suite = ET.parse(output / 'tests.xml').getroot().find('testsuite')
            self.assertEqual(suite.get('tests'), '2')
            self.assertEqual(suite.get('failures'), '1')
            self.assertIn('late successful callback', ET.tostring(suite, encoding='unicode'))

    def test_late_truthy_wait_predicate_fails_and_following_test_runs(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import native as n

def test_first_deadline_contract_probe():
    def late_result():
        time.sleep(.2)
        return True
    n.wait(late_result, 'late truthy predicate', .1)

def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertEqual(result['exit_code'], 1)
            self.assertTrue(sentinel.exists())
            suite = ET.parse(output / 'tests.xml').getroot().find('testsuite')
            self.assertEqual(suite.get('failures'), '1')
            self.assertIn('late truthy predicate', ET.tostring(suite, encoding='unicode'))

    def test_nested_operations_preserve_and_restore_outer_deadline(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            _, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline

def test_first_deadline_contract_probe():
    with deadline.operation('outer bounds longer inner', .2):
        with deadline.operation('longer inner', 5):
            time.sleep(60)

def test_second_deadline_contract_probe():
    with deadline.operation('outer restored after inner', .4):
        with deadline.operation('completed shorter inner', .2):
            pass
        time.sleep(60)

def test_third_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertEqual(result['exit_code'], 1)
            self.assertTrue(sentinel.exists())
            timeouts = result['case_timeouts']
            self.assertEqual([(row['operation'], row['budget']) for row in timeouts],
                             [('outer bounds longer inner', .2), ('outer restored after inner', .4)])
            self.assertEqual(result['case_counts'], dict(tests=3, failures=2, errors=0, skipped=0))

    def test_multiple_timeouts_preserve_failures_skips_and_completed_cases(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            executions = folder / 'executions'
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
import pytest
from support import deadline

def record(name):
    with Path(%r).open('a') as stream:
        stream.write(name + '\\n')

def test_before_deadline_contract_probe():
    record('before')
    raise AssertionError('ordinary failure retained')

def test_first_deadline_contract_probe():
    record('first')
    with deadline.operation('first blocked operation', .1):
        time.sleep(60)

def test_skipped_deadline_contract_probe():
    record('skipped')
    pytest.skip('intentional capability skip')

def test_second_deadline_contract_probe():
    record('second')
    with deadline.operation('second blocked operation', .1):
        time.sleep(60)

def test_after_deadline_contract_probe():
    record('after')
''' % str(executions))
            self.assertEqual(executions.read_text().splitlines(),
                             ['before', 'first', 'skipped', 'second', 'after'])
            self.assertEqual(len(result['case_timeouts']), 2)
            self.assertEqual(result['case_counts'], dict(tests=5, failures=3, errors=0, skipped=1))
            suite = ET.parse(output / 'tests.xml').getroot().find('testsuite')
            self.assertEqual(len(suite.findall('testcase')), 5)
            self.assertIn('ordinary failure retained', ET.tostring(suite, encoding='unicode'))

    def test_setup_and_teardown_timeouts_continue_with_phase_errors(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
import pytest
from support import deadline

@pytest.fixture
def blocked_setup():
    with deadline.operation('blocked fixture setup', .1):
        time.sleep(60)

@pytest.fixture
def blocked_teardown():
    yield
    with deadline.operation('blocked fixture teardown', .1):
        time.sleep(60)

def test_setup_deadline_contract_probe(blocked_setup):
    raise AssertionError('setup should not complete')

def test_teardown_deadline_contract_probe(blocked_teardown):
    raise AssertionError('call failure before teardown timeout')

def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertTrue(sentinel.exists())
            self.assertEqual([row['phase'] for row in result['case_timeouts']], ['setup', 'teardown'])
            self.assertEqual(result['case_counts'], dict(tests=3, failures=1, errors=2, skipped=0))
            text = ET.tostring(ET.parse(output / 'tests.xml').getroot(), encoding='unicode')
            self.assertIn('call failure before teardown timeout', text)
            self.assertIn('blocked fixture teardown', text)

    def test_cancellation_does_not_continue_remaining_cases(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'unexpected-following-test'
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline

def test_cancel_deadline_contract_probe(request):
    (Path(request.config.getoption('--artifacts')) / 'cancel').touch()
    with deadline.operation('cancelled operation', 2):
        time.sleep(60)

def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertTrue(result['cancelled'], result)
            self.assertFalse(result['timed_out'], result)
            self.assertFalse(sentinel.exists())
            self.assertEqual(result['case_counts'], dict(tests=1, failures=0, errors=1, skipped=0))
            case = ET.parse(output / 'tests.xml').getroot().find('.//testcase')
            self.assertEqual(case.attrib['name'], 'test_cancel_deadline_contract_probe')
            self.assertIsNotNone(case.find('error'))

    def test_cancellation_after_worker_exit_is_not_success(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            output = folder / 'artifacts'
            probe = gui_probe.write_probe(
                folder, 'def test_completed_deadline_contract_probe():\n    pass\n')
            process = gui_probe.run_wrapper(folder, '''
import run
original_attempt = run.run_attempt

def cancel_after_exit(args, tests, attempt, root, excluded):
    result = original_attempt(args, tests, attempt, root, excluded)
    assert result['exit_code'] == 0 and result['cleanup_complete'], result
    # Request cancellation at the real worker-exit/whole-run publication boundary.
    (root / 'cancel').touch()
    return result

run.run_attempt = cancel_after_exit
sys.argv = ['run.py', '--timeout', '10', '--output', %r, '--exe', sys.executable,
            %r, '-o', 'python_files=test_deadline_probe.py', '--ignore-glob=*', '-q']
raise SystemExit(run.main())
''' % (str(output), str(probe)))
            result = gui_probe.read_run(output)
            self.assertTrue(result['cancelled'], result)
            self.assertFalse(result['timed_out'], result)
            self.assertTrue(result['cleanup_complete'], result)
            self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
            case = ET.parse(output / 'tests.xml').getroot().find('.//testcase')
            self.assertEqual(case.attrib['name'], 'test_completed_deadline_contract_probe')
            self.assertIsNone(case.find('error'))
            self.assertIsNone(case.find('failure'))

    def test_cleanup_failure_prevents_continuation(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'unexpected-following-test'
            _, result = self.run_probe(folder, '''
import json
import time
from pathlib import Path
from support import deadline

def test_cleanup_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    (output / 'injected-cleanup.json').write_text(json.dumps({
        'cleanup_complete': False, 'errors': ['injected recovery failure']}))
    with deadline.operation('blocked before failed recovery', .1):
        time.sleep(60)

def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), cleanup_complete=False)
            self.assertTrue(result['timed_out'], result)
            self.assertTrue(result['cleanup_errors'], result)
            self.assertFalse(sentinel.exists())

    def test_collection_timeout_has_no_resumable_case_boundary(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            output, result = self.run_probe(folder, '''
import time
from support import deadline

with deadline.operation('blocked module collection', .1):
    time.sleep(60)

def test_after_deadline_contract_probe():
    raise AssertionError('collection must not finish')
''')
            self.assertTrue(result['timed_out'], result)
            self.assertEqual(result['timeout']['phase'], 'collection')
            self.assertFalse((output / 'continued').exists())

    def test_whole_run_budget_retains_active_context_and_blocked_stack_after_progress(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'unexpected-following-test'
            started = time.monotonic()
            output, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline
def blocked_budget_navigation():
    time.sleep(60)
def test_completed_deadline_contract_probe():
    pass
def test_active_deadline_contract_probe():
    with deadline.operation('navigation beyond whole-run budget', 45):
        for _ in range(10):
            with deadline.operation('navigation progress delivery', 5):
                time.sleep(.05)
        blocked_budget_navigation()
def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), timeout=2.5)
            self.assertLess(time.monotonic() - started, 6, result)
            self.assertEqual(result['termination_reason'], 'budget_exhausted')
            self.assertTrue(result['timed_out'])
            self.assertNotIn('timeout', result, 'whole-run exhaustion became a resumable case timeout')
            self.assertFalse(sentinel.exists())
            self.assertEqual(result['case_counts'], dict(tests=2, failures=0, errors=1, skipped=0))
            cases = {case.attrib['name']: case
                     for case in ET.parse(output / 'tests.xml').getroot().findall('.//testcase')}
            self.assertIsNone(cases['test_completed_deadline_contract_probe'].find('error'))
            self.assertIsNone(cases['test_completed_deadline_contract_probe'].find('failure'))
            self.assertIsNotNone(cases['test_active_deadline_contract_probe'].find('error'))
            self.assertFalse((output / 'continued').exists())
            active = result['active_context']
            self.assertTrue(active['nodeid'].endswith('::test_active_deadline_contract_probe'), active)
            self.assertEqual(active['phase'], 'call')
            self.assertEqual(active['operation'], 'navigation beyond whole-run budget')
            self.assertEqual(active['budget'], 45)
            self.assertGreater(active['deadline'], result['budget_deadline'])
            stack = Path(result['pytest_stack']).read_text(encoding='utf-8')
            self.assertIn('blocked_budget_navigation', stack)
            self.assertIn('test_active_deadline_contract_probe', stack)

    def test_parent_deadline_survives_caught_nested_exception(self):
        for child_budget in (.2, 5):
            with self.subTest(child_budget=child_budget), tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                folder = Path(directory)
                sentinel = folder / 'continued-after-parent-timeout'
                _, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline

def test_first_deadline_contract_probe():
    with deadline.operation('parent after caught child exception', .4):
        try:
            with deadline.operation('failing nested operation', %r):
                raise ValueError('ordinary child failure')
        except ValueError:
            pass
        time.sleep(60)

def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % (child_budget, str(sentinel)))
                self.assertTrue(sentinel.exists())
                self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=0, skipped=0))
                self.assertEqual(len(result['case_timeouts']), 1)
                self.assertEqual(result['case_timeouts'][0]['operation'], 'parent after caught child exception')
                self.assertEqual(result['case_timeouts'][0]['budget'], .4)

    def test_whole_run_budget_is_not_reset_and_interrupted_case_is_not_passed(self):
        for phase in ('setup', 'call'):
            with self.subTest(interrupted_phase=phase), tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                folder = Path(directory)
                sentinel = folder / 'unexpected-following-test'
                output, result = self.run_probe(folder, '''
import os
import time
from pathlib import Path
import pytest
from support import deadline

def record_budget(request):
    (Path(request.config.getoption('--artifacts')) / 'run-budget').write_text(os.environ['WINLEX_TEST_DEADLINE'])

def block_remaining_budget(request):
    record_budget(request)
    with deadline.operation('operation exceeding remaining run budget', 10):
        time.sleep(60)

@pytest.fixture(autouse=True)
def interrupt_setup(request):
    if %r == 'setup' and request.node.name == 'test_second_deadline_contract_probe':
        block_remaining_budget(request)

def test_first_deadline_contract_probe(request):
    record_budget(request)
    with deadline.operation('first blocked operation', .1):
        time.sleep(60)

def test_second_deadline_contract_probe(request):
    block_remaining_budget(request)

def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % (phase, str(sentinel)), timeout=3)
                self.assertFalse(sentinel.exists())
                self.assertEqual(len(result['case_timeouts']), 1)
                self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=1, skipped=0))
                first_budget = (output / 'run-budget').read_text()
                self.assertEqual((output / 'continued/1/run-budget').read_text(), first_budget)
                self.assertEqual(result['budget_deadline'], float(first_budget))

    def test_last_selected_case_timeout_does_not_launch_empty_worker(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            output, result = self.run_probe(Path(directory), '''
import time
from support import deadline

def test_only_deadline_contract_probe():
    with deadline.operation('last selected case blocks', .1):
        time.sleep(60)
''')
            self.assertTrue(result['timed_out'], result)
            self.assertFalse((output / 'continued').exists(),
                             'a completed selection launched a no-tests worker')

    def test_empty_isolated_desktop_is_a_valid_empty_enumeration(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            self.run_probe(Path(directory), '''
from support import native as n

def test_empty_deadline_contract_probe():
    assert n.top_level_windows() == []
    assert n.find_window(0, 'Winlex.Frame') is None
''', exit_code=0)


if __name__ == '__main__':
    unittest.main()

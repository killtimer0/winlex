"""Exercise timeout diagnostics and case failure behavior through the real runner."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.name == 'nt' and importlib.util.find_spec('uiautomation'),
                     'requires the Windows GUI test dependencies')
class GuiDeadlineContracts(unittest.TestCase):
    def supervise_probe(self, folder, script, timeout=5):
        gui = ROOT / 'tests' / 'gui'
        wrapper = folder / 'runner_wrapper.py'
        wrapper.write_text('import sys\nsys.path.insert(0, %r)\n' % str(gui) + script, encoding='utf-8')
        output = folder / ('0' * 32)
        output.mkdir(exist_ok=True)
        spec = importlib.util.spec_from_file_location('deadline_remote', gui / 'remote.py')
        remote = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(remote)
        started = time.monotonic()
        with (folder / 'wrapper.log').open('w', encoding='utf-8') as log:
            process = subprocess.Popen([sys.executable, str(wrapper), str(output)], cwd=ROOT,
                                       stdout=log, stderr=subprocess.STDOUT)
            sys.path.insert(0, str(gui))
            try:
                safe = remote.supervise_runner(process, output, timeout, started, lambda: None)
            finally:
                sys.path.remove(str(gui))
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
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
            '-k', 'deadline_contract_probe', '-q']
raise SystemExit(run.main())
''' % str(probe))
            self.assertTrue(status['runner_terminated'], status)
            self.assertIn('cleanup', status['error'])
            self.assertTrue((output / 'runner-cleanup.jsonl').exists())
            self.assertIn('lambda', (output / 'runner-timeout-stacks.log').read_text(encoding='utf-8'))

    def test_frozen_cleanup_plan_beats_renewed_operations_and_old_success(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            output, status = self.supervise_probe(folder, '''
import json
import time
from pathlib import Path
from support import deadline
output = Path(sys.argv[1])
(output / 'run.json').write_text(json.dumps({'exit_code': 0, 'cleanup_complete': True}))
deadline.configure(output, prefix='runner-')
(output / 'runner-cleanup.jsonl').write_text(json.dumps({
    'attempt': '.', 'deadline': time.monotonic() + .3, 'budget': .3}) + '\\n')
while True:
    deadline.phase('runner', 'cleanup', 10)
    time.sleep(.05)
''')
            self.assertEqual(status['state'], 'supervisor_failed')
            self.assertTrue(json.loads((output / 'run.json').read_text())['cleanup_complete'])

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
                output, _ = self.supervise_probe(Path(directory), '''
import json
import types
from pathlib import Path
import run
output = Path(sys.argv[1])
(output / 'profiles').mkdir()
(output / 'profiles' / 'input.json').write_text(json.dumps({
    'held': [], 'foreground': 0, 'cursor': [0, 0]}))
fake = types.ModuleType('support.input_state')
fake.recover = lambda path: (output / 'unauthorized-recovery').touch()
sys.modules['support.input_state'] = fake
sys.argv = ['run.py', '--output', str(output), '--desktop', 'current', '--real-input']
raise SystemExit(run.main())
''')
                self.assertFalse((output / 'unauthorized-recovery').exists())
                report = json.loads((output / 'run.json').read_text())
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
                wrapper = folder / 'runner_wrapper.py'
                wrapper.write_text('''
import sys
sys.path.insert(0, %r)
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
''' % (str(ROOT / 'tests' / 'gui'), cancel, str(output / 'cancel')), encoding='utf-8')
                process = subprocess.run([
                    sys.executable, str(wrapper), '--output', str(output), '--timeout', '2',
                    '--exe', sys.executable, str(probe), '-k', 'deadline_contract_probe', '-q',
                ], cwd=ROOT, capture_output=True, text=True, encoding='utf-8', timeout=20)
                self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
                report = json.loads((output / 'run.json').read_text())
                self.assertTrue(report['cleanup_complete'], report)
                self.assertEqual(report['cancelled'], cancel, report)
                self.assertFalse(sentinel.exists())
                self.assertEqual(len((output / 'attempts.jsonl').read_text().splitlines()), 1)

    def test_unrelated_observer_markers_are_not_watchdog_obligations(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            self.run_probe(Path(directory), '''
import json
from pathlib import Path
def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    for name in ('observer-pump.ready', 'plain.ready', 'observer.not-a-task-id.ready'):
        (output / name).write_text('ready')
    (output / 'observer.result.json').write_text('unrelated observer output')
    (output / 'plain.recovery.json').write_text(json.dumps({'ok': False}))
''', exit_code=0)

    def test_malformed_owned_watchdog_receipt_without_snapshot_is_not_ignored(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            _, result = self.run_probe(Path(directory), '''
from pathlib import Path
def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    (output / 'contrast.0123456789abcdef0123456789abcdef.result.json').write_text('{"ok":')
''', cleanup_complete=False)
            self.assertTrue(result['cleanup_errors'])

    def test_malformed_registered_watchdog_snapshot_is_not_ignored(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            _, result = self.run_probe(Path(directory), '''
import json
from pathlib import Path
def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    (output / 'contrast.json').write_text('{"flags":')
    (output / 'contrast.0123456789abcdef0123456789abcdef.result.json').write_text(
        json.dumps({'ok': True}))
''', cleanup_complete=False)
            self.assertTrue(result['cleanup_errors'])

    def test_registered_snapshot_requires_owned_watchdog_receipt(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            _, result = self.run_probe(Path(directory), '''
import json
from pathlib import Path
def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    (output / 'contrast.json').write_text(json.dumps({
        'flags': 0, 'scheme': '', 'context': {}}))
    (output / 'contrast.result.json').write_text(json.dumps({'ok': True}))
''', cleanup_complete=False)
            self.assertTrue(any('High-contrast recovery incomplete' in error for error in result['cleanup_errors']))

    def test_watchdog_failure_receipt_without_ready_prevents_continuation(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'unexpected-following-test'
            _, result = self.run_probe(folder, '''
import json
import time
from pathlib import Path
from support import deadline
def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    (output / 'contrast-restore.json').write_text(json.dumps({
        'flags': 0, 'scheme': '', 'context': {}}))
    (output / 'contrast-restore.0123456789abcdef0123456789abcdef.result.json').write_text(
        json.dumps({'ok': False, 'error': 'injected watchdog failure'}))
    with deadline.operation('blocked after watchdog receipt', .1):
        time.sleep(60)
def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), cleanup_complete=False)
            self.assertFalse(sentinel.exists())
            self.assertTrue(any('High-contrast recovery incomplete' in error for error in result['cleanup_errors']))

    def test_recovery_marker_waits_for_final_receipt_native_and_task_cleanup_budget(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            output = folder / 'artifacts'
            recovery = output / 'contrast.0123456789abcdef0123456789abcdef.recovery.json'
            final = output / 'contrast.0123456789abcdef0123456789abcdef.result.json'
            def publish_final():
                until = time.monotonic() + 5
                while not recovery.exists() and time.monotonic() < until:
                    time.sleep(.05)
                if recovery.exists():
                    time.sleep(10.5)
                    temporary = final.with_suffix('.new')
                    temporary.write_text(json.dumps({'ok': True}), encoding='utf-8')
                    temporary.replace(final)
            publisher = threading.Thread(target=publish_final, daemon=True)
            publisher.start()
            try:
                _, result = self.run_probe(folder, '''
import json
import time
from pathlib import Path
from support import deadline
def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    (output / 'contrast.0123456789abcdef0123456789abcdef.recovery.json').write_text(
        json.dumps({'ok': True}))
    with deadline.operation('blocked with only native recovery evidence', .1):
        time.sleep(60)
''')
                self.assertTrue(result['cleanup_complete'])
                self.assertTrue(final.exists(), 'native recovery evidence was accepted without final task cleanup')
            finally:
                publisher.join(timeout=16)
            self.assertFalse(publisher.is_alive())

    def run_probe(self, folder, code, cleanup_complete=True, timeout=10, exit_code=None):
        probe = folder / 'test_deadline_probe.py'
        probe.write_text(code, encoding='utf-8')
        output = folder / 'artifacts'
        process = subprocess.run([
            sys.executable, 'tests/gui/run.py', '--timeout', str(timeout), '--output', str(output),
            '--exe', sys.executable, str(probe), '-k', 'deadline_contract_probe', '-q',
        ], cwd=ROOT, capture_output=True, text=True, encoding='utf-8', timeout=20)
        if exit_code is None:
            self.assertNotEqual(process.returncode, 0, process.stdout + process.stderr)
        else:
            self.assertEqual(process.returncode, exit_code, process.stdout + process.stderr)
        result = json.loads((output / 'run.json').read_text(encoding='utf-8'))
        self.assertEqual(result['cleanup_complete'], cleanup_complete, result)
        return output, result

    def test_picker_preflight_timeout_and_unconfirmed_cleanup_queue_boundary(self):
        import win32api
        import win32con
        import win32event

        for denied in (False, True):
            with self.subTest(termination_denied=denied), tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                output, result = self.run_probe(Path(directory), '''
import json
from pathlib import Path
import subprocess
import sys
import pytest
from support import picker_environment

def test_first_deadline_contract_probe(request, monkeypatch):
    output = Path(request.config.getoption('--artifacts'))
    ready = output / 'owned-probe-ready.json'
    real_popen = subprocess.Popen
    program = ('import os,threading; from pathlib import Path; '
               'Path(%%r).write_text(str(os.getpid())); threading.Event().wait(60)' %% str(ready))
    class StalledProbe(real_popen):
        def __init__(self, command, *args, **kwargs):
            super().__init__([sys.executable, '-c', program], *args, **kwargs)
        def kill(self):
            if %r:
                raise PermissionError(5, 'Injected child termination denial')
            return super().kill()
    with monkeypatch.context() as patch:
        patch.setattr(picker_environment.subprocess, 'Popen', StalledProbe)
        if %r:
            picker_environment.require_picker_environment(output / 'preflight.json')
            pytest.fail('Unconfirmed cleanup did not stop the queue')
        with pytest.raises(RuntimeError, match='exceeded 1.5 seconds'):
            picker_environment.require_picker_environment(output / 'preflight.json')

def test_second_deadline_contract_probe(request):
    (Path(request.config.getoption('--artifacts')) / 'following-case-ran').touch()
''' % (denied, denied), cleanup_complete=not denied, exit_code=None if denied else 0)
                evidence = json.loads((output / 'preflight.json').read_text())
                self.assertLess(evidence['elapsed_seconds'], 3)
                self.assertEqual(evidence['status'], 'cleanup_unconfirmed' if denied else 'preflight_failed')
                self.assertEqual((output / 'following-case-ran').exists(), not denied)
                self.assertFalse(result['timed_out'], result)
                pid = int((output / 'owned-probe-ready.json').read_text())
                try:
                    process = win32api.OpenProcess(win32con.SYNCHRONIZE, False, pid)
                except win32api.error as error:
                    self.assertEqual(error.winerror, 87)  # ERROR_INVALID_PARAMETER: PID no longer exists.
                else:
                    try:
                        self.assertEqual(win32event.WaitForSingleObject(process, 0), win32con.WAIT_OBJECT_0)
                    finally:
                        win32api.CloseHandle(process)

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

    def test_unverified_cooperative_restore_stops_despite_successful_structure_recovery(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'unexpected-following-test'
            _, result = self.run_probe(folder, '''
import json
from pathlib import Path
import pytest

def test_first_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    snapshot = {'flags': 0, 'scheme': '', 'context': {}}
    (output / 'contrast.json').write_text(json.dumps(snapshot))
    (output / 'contrast.0123456789abcdef0123456789abcdef.result.json').write_text(
        json.dumps({'ok': True, 'expected': snapshot, 'context': {},
                    'restored': {'flags': 0, 'scheme': ''},
                    'emergency': True, 'recovery_needed': False,
                    'task_cleanup_complete': True}))
    (output / 'contrast-hc-cleanup.json').write_text(json.dumps({
        'cleanup_complete': False,
        'errors': ['Cooperative native palette/cover restoration is unverified']}))
    pytest.exit('Cooperative high-contrast restoration failed', returncode=1)

def test_second_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), cleanup_complete=False, timeout=5)
            self.assertEqual(result['exit_code'], 1)
            self.assertFalse(result['timed_out'], result)
            self.assertFalse(sentinel.exists(), 'unverified desktop allowed another case')
            self.assertIn('Cooperative native palette/cover restoration is unverified',
                          result['cleanup_errors'])

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
            _, result = self.run_probe(folder, '''
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
            self.assertIn('injected recovery failure', result['cleanup_errors'])
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

    def test_interrupted_timing_write_does_not_lose_completed_results(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'following-test-ran'
            _, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline

def test_before_deadline_contract_probe():
    pass

def test_blocked_deadline_contract_probe(request):
    output = Path(request.config.getoption('--artifacts'))
    with (output / 'timings.jsonl').open('ab') as stream:
        stream.write(b'{"message": "partial UTF-8 \\xe4')
    with deadline.operation('interrupted timing write', .1):
        time.sleep(60)

def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel))
            self.assertTrue(sentinel.exists())
            self.assertEqual(result['case_counts'], dict(tests=3, failures=1, errors=0, skipped=0))

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
def test_active_deadline_contract_probe(request):
    with deadline.operation('navigation beyond whole-run budget', 45):
        for _ in range(10):
            with deadline.operation('navigation progress delivery', 5):
                time.sleep(.05)
        output = Path(request.config.getoption('--artifacts'))
        with (output / 'deadlines.jsonl').open('ab') as stream:
            stream.write(b'{"operation": "partial UTF-8 \\xe4')
        blocked_budget_navigation()
def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), timeout=2.5)
            self.assertLess(time.monotonic() - started, 6, result)
            self.assertEqual(result['termination_reason'], 'budget_exhausted')
            self.assertTrue(result['timed_out'])
            self.assertNotIn('timeout', result, 'whole-run exhaustion became a resumable case timeout')
            self.assertFalse(sentinel.exists())
            self.assertEqual(len((output / 'attempts.jsonl').read_text().splitlines()), 1)
            active = result['active_context']
            self.assertTrue(active['nodeid'].endswith('::test_active_deadline_contract_probe'), active)
            self.assertEqual(active['phase'], 'call')
            self.assertEqual(active['operation'], 'navigation beyond whole-run budget')
            self.assertEqual(active['budget'], 45)
            self.assertGreater(active['deadline'], result['budget_deadline'])
            stack = Path(result['pytest_stack']).read_text(encoding='utf-8')
            self.assertIn('blocked_budget_navigation', stack)
            self.assertIn('test_active_deadline_contract_probe', stack)

    def test_whole_run_budget_is_not_reset_and_interrupted_case_is_not_passed(self):
        with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
            folder = Path(directory)
            sentinel = folder / 'unexpected-following-test'
            _, result = self.run_probe(folder, '''
import time
from pathlib import Path
from support import deadline

def test_first_deadline_contract_probe():
    with deadline.operation('first blocked operation', .1):
        time.sleep(60)

def test_second_deadline_contract_probe():
    with deadline.operation('operation exceeding remaining run budget', 10):
        time.sleep(60)

def test_after_deadline_contract_probe():
    Path(%r).touch()
''' % str(sentinel), timeout=3)
            self.assertFalse(sentinel.exists())
            self.assertEqual(len(result['case_timeouts']), 1)
            self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=1, skipped=0))

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
            selection = json.loads((output / 'selection.json').read_text())
            self.assertEqual(len(selection), 1)
            attempts = (output / 'attempts.jsonl').read_text().splitlines()
            self.assertEqual(len(attempts), 1, 'a completed selection launched a no-tests worker')

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

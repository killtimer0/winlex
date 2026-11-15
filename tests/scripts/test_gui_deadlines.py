"""Public GUI runner fault contracts. Execute only on the remote test target."""
import json
from pathlib import Path
import time
import unittest
import xml.etree.ElementTree as ET

import gui_probe


# No app, settings fixture, foreground input or UIAutomation calls are needed.
# The subprocess still loads the real GUI conftest and lifecycle plugin.
PRELUDE = '''
import ctypes
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import pytest
from support import deadline

def output(request):
    return Path(request.config.getoption('--artifacts'))

def record(request, name):
    root = output(request).parents[1]
    with (root / 'executions').open('a', encoding='utf-8') as stream:
        stream.write(name + '\\n')

def blocked_native_with_gil():
    sleep = ctypes.PyDLL('kernel32').Sleep
    sleep.argtypes = [ctypes.c_ulong]
    sleep.restype = None
    sleep(60000)

def tree(request):
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
    (output(request) / 'processes.json').write_text(json.dumps({
        'pytest': os.getpid(), 'custody': os.getppid(), 'descendant': child.pid}), encoding='utf-8')
'''


@unittest.skipUnless(gui_probe.dependencies(), 'requires Windows, pytest, UIAutomation and comtypes')
class GuiDeadlineContracts(unittest.TestCase):
    def runner(self, source, **options):
        return gui_probe.Runner(PRELUDE + source, **options)

    def final(self, runner, *, state, cleanup=True):
        result = runner.wait()
        self.assertEqual(result['state'], state, runner.diagnostic())
        self.assertEqual(result['cleanup_complete'], cleanup, result)
        self.assertEqual(result['worker_available'], cleanup, result)
        self.assertEqual(runner.process.returncode, 0 if state == 'passed' else 1, result)
        return result

    def executions(self, runner):
        return (runner.output / 'executions').read_text(encoding='utf-8').splitlines()

    def handles(self, attempt):
        path = attempt / 'processes.json'
        gui_probe.wait_for(path.exists, 10, 'Probe did not publish its process tree')
        pids = json.loads(path.read_text(encoding='utf-8'))
        handles = {name: gui_probe.ProcessHandle(pid) for name, pid in pids.items()}
        for handle in handles.values():
            self.addCleanup(handle.close)
            self.assertTrue(handle.alive(), pids)
        return handles

    def assert_tree_dead(self, handles):
        for name, handle in handles.items():
            self.assertTrue(handle.dead(), name + ' survived recovery')

    def test_success_and_assertion_failure_finalize_after_fixture_cleanup(self):
        for fail in (False, True):
            with self.subTest(fail=fail), self.runner('''
@pytest.fixture
def cleaned(request):
    yield
    (output(request) / 'fixture-cleanup').write_text('complete', encoding='utf-8')

def test_first(request, cleaned):
    record(request, 'first')
    assert not %r, 'intentional contract failure'

def test_after(request):
    record(request, 'after')
''' % fail) as runner:
                runner.start()
                result = self.final(runner, state='failed' if fail else 'passed')
                self.assertFalse(result['timed_out'])
                self.assertFalse(result['cancelled'])
                self.assertEqual(self.executions(runner), ['first', 'after'])
                self.assertEqual(result['case_counts'], dict(tests=2, failures=int(fail), errors=0, skipped=0))
                for attempt in gui_probe.attempts(runner.output):
                    self.assertEqual((attempt / 'fixture-cleanup').read_text(), 'complete')
                if fail:
                    self.assertIn('intentional contract failure', (runner.output / 'tests.xml').read_text())

    def test_native_utf16_failure_retains_diagnostics_and_continues(self):
        with self.runner('''
def test_native_path_failure(request):
    record(request, 'failed')
    text = 'native-path-' + chr(0xd800) + '-control-' + chr(1) + '-end'
    print(text)
    pytest.fail(text)

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            result = self.final(runner, state='failed')
            self.assertEqual(self.executions(runner), ['failed', 'after'])
            self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=0, skipped=0))
            failures = ET.parse(runner.output / 'tests.xml').getroot().findall('.//failure')
            self.assertEqual(len(failures), 1)
            self.assertIn(r'native-path-\ud800-control-\x01-end', failures[0].text)
            attempts = gui_probe.attempts(runner.output)
            self.assertEqual(len(attempts), 1)
            diagnostic = (attempts[0] / 'test_native_path_failure-failure.txt').read_text(encoding='utf-8')
            self.assertIn(r'native-path-\ud800-control-', diagnostic)

    def test_long_failure_attachment_survives_and_next_case_runs(self):
        name = 'test_' + 'long_diagnostic_' * 13
        with self.runner('''
def %s(request):
    pytest.fail('intentional long-path diagnostic')

def test_after(request):
    from support.paths import extended
    path = output(request) / %r
    assert len(str(path)) > 260, 'probe must exercise an extended artifact path'
    assert 'intentional long-path diagnostic' in extended(path).read_text(encoding='utf-8')
''' % (name, name + '-failure.txt')) as runner:
            runner.start()
            result = self.final(runner, state='failed')
            self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=0, skipped=0))

    def test_native_held_gil_timeout_retains_stack_kills_tree_and_never_replays(self):
        with self.runner('''
def test_before(request):
    record(request, 'before')

def test_blocked(request):
    record(request, 'blocked')
    tree(request)
    with deadline.operation('held GIL contract', 2):
        blocked_native_with_gil()

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            attempt, _ = runner.event('call', operation='held GIL contract')
            handles = self.handles(attempt)
            result = self.final(runner, state='timed_out')
            self.assertEqual(result['termination_reason'], 'case_timeout')
            self.assertEqual(result['timeout']['operation'], 'held GIL contract')
            self.assertEqual(result['timeout']['phase'], 'call')
            self.assertEqual(self.executions(runner), ['before', 'blocked', 'after'])
            self.assertEqual(result['case_counts'], dict(tests=3, failures=1, errors=0, skipped=0))
            self.assert_tree_dead(handles)
            self.assertIn('blocked_native_with_gil', (attempt / 'timeout-stacks.log').read_text())
            cases = ET.parse(runner.output / 'tests.xml').getroot().findall('.//testcase')
            self.assertEqual([case.get('name') for case in cases], ['test_before', 'test_blocked', 'test_after'])
            self.assertIsNotNone(cases[1].find('failure'))

    def test_absolute_budget_survives_replacement_custody(self):
        for phase in ('setup', 'call'):
            with self.subTest(interrupted_phase=phase), self.runner('''
def test_first(request):
    record(request, 'first')
    (output(request) / 'run-budget').write_text(os.environ['WINLEX_TEST_DEADLINE'])
    with deadline.operation('first case timeout', .5):
        time.sleep(60)

def exhaust_budget(request):
    record(request, 'second')
    (output(request) / 'run-budget').write_text(os.environ['WINLEX_TEST_DEADLINE'])
    with deadline.operation('remaining whole-run budget', 20):
        for _ in range(10):
            with deadline.operation('navigation progress delivery', 5):
                time.sleep(.05)
        time.sleep(60)

@pytest.fixture
def interrupt_setup(request):
    if %r == 'setup':
        exhaust_budget(request)

def test_second(request, interrupt_setup):
    exhaust_budget(request)

def test_after(request):
    record(request, 'after')
''' % phase, timeout=10) as runner:
                runner.start()
                result = self.final(runner, state='timed_out')
                self.assertEqual(result['termination_reason'], 'budget_exhausted')
                self.assertEqual(self.executions(runner), ['first', 'second'])
                attempts = gui_probe.attempts(runner.output)
                self.assertEqual(len(attempts), 2)
                plans = [json.loads((attempt / 'invocation.json').read_text()) for attempt in attempts]
                self.assertEqual(plans[0]['until'], plans[1]['until'])
                self.assertEqual(result['budget_deadline'], plans[0]['until'])
                for attempt in attempts:
                    self.assertEqual(float((attempt / 'run-budget').read_text()), plans[0]['until'])
                self.assertEqual(result['case_counts'], dict(tests=2, failures=1, errors=1, skipped=0))
                active = result['active_context']
                self.assertTrue(active['nodeid'].endswith('::test_second'), active)
                self.assertEqual(active['phase'], phase)
                self.assertEqual(active['operation'], 'remaining whole-run budget')
                self.assertEqual(active['budget'], 20)
                self.assertGreater(active['deadline'], result['budget_deadline'])
                stack = Path(result['pytest_stack']).read_text()
                self.assertIn('exhaust_budget', stack)
                self.assertIn('test_second' if phase == 'call' else 'interrupt_setup', stack)

    def test_cancellation_during_execution_is_not_success_and_stops_selection(self):
        with self.runner('''
def test_active(request):
    record(request, 'active')
    tree(request)
    with deadline.operation('cancel barrier', 30):
        time.sleep(60)

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            attempt, _ = runner.event('call', operation='cancel barrier')
            handles = self.handles(attempt)
            (runner.output / 'cancel').touch()
            result = self.final(runner, state='cancelled')
            self.assertTrue(result['cancelled'])
            self.assertFalse(result['timed_out'])
            self.assertEqual(self.executions(runner), ['active'])
            self.assertEqual(len(gui_probe.attempts(runner.output)), 1)
            self.assert_tree_dead(handles)

    def test_cancellation_before_start_creates_no_custody(self):
        with self.runner('def test_never():\n    raise AssertionError("must not start")\n') as runner:
            (runner.output / 'cancel').touch()
            runner.start()
            result = self.final(runner, state='cancelled')
            self.assertTrue(result['cancelled'])
            self.assertEqual(gui_probe.records(runner.output / 'attempts.jsonl'), [])
            self.assertFalse((runner.output / 'attempts').exists())

    def test_coordinator_death_recovers_held_gil_tree_and_profile_without_final_report(self):
        import winreg

        with self.runner('''
from support.config import ConfigProfile

def test_active(request):
    with ConfigProfile() as profile:
        (output(request) / 'resource-profile.json').write_text(
            json.dumps(profile.root), encoding='utf-8')
        record(request, 'active')
        tree(request)
        with deadline.operation('coordinator death barrier', 30):
            blocked_native_with_gil()

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            attempt, _ = runner.event('call', operation='coordinator death barrier')
            handles = self.handles(attempt)
            owner = gui_probe.ProcessHandle(runner.process.pid)
            self.addCleanup(owner.close)
            self.assertTrue(owner.alive())
            profile = json.loads((attempt / 'resource-profile.json').read_text(encoding='utf-8'))
            ledger = attempt / 'profiles' / (profile.rsplit('\\', 1)[-1] + '.profile')
            self.assertTrue(ledger.exists())
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, profile):
                pass

            # Kill only the coordinator, not /T: independent custody must recover.
            owner.kill()
            self.assertTrue(owner.dead(), runner.diagnostic())
            self.assertNotEqual(runner.process.wait(timeout=10), 0)
            self.assert_tree_dead(handles)
            evidence = attempt / 'custody-result.json'
            gui_probe.wait_for(evidence.exists, 10, 'Owner-loss recovery did not publish evidence: ' + runner.diagnostic())
            result = json.loads(evidence.read_text(encoding='utf-8'))
            self.assertTrue(result['cleanup_complete'], result)
            self.assertEqual(result['termination_reason'], 'owner_lost')
            self.assertTrue(result['cancelled'], result)
            self.assertEqual(result['cleanup_errors'], [])
            self.assertFalse(ledger.exists(), 'Orphaned test profile ledger was not consumed')
            with self.assertRaises(FileNotFoundError):
                winreg.OpenKey(winreg.HKEY_CURRENT_USER, profile)
            self.assertEqual(self.executions(runner), ['active'])
            self.assertEqual(len(gui_probe.attempts(runner.output)), 1)
            self.assertFalse((runner.output / 'run.json').exists(), 'Dead coordinator cannot publish a final result')

    def test_first_setup_process_crash_is_an_interrupted_case_error(self):
        for exit_code in (0, 73):
            with self.subTest(pytest_exit=exit_code), self.runner('''
@pytest.fixture
def crash_setup(request):
    record(request, 'setup')
    os._exit(%d)

def test_first(request, crash_setup):
    record(request, 'first body')

def test_after(request):
    record(request, 'after')
''' % exit_code) as runner:
                runner.start()
                result = self.final(runner, state='failed')
                self.assertEqual(result['exit_code'], exit_code or 1, result)
                self.assertFalse(result['timed_out'])
                self.assertFalse(result['cancelled'])
                self.assertEqual(self.executions(runner), ['setup'])
                self.assertEqual(result['case_counts'], dict(tests=1, failures=0, errors=1, skipped=0))
                attempt, = gui_probe.attempts(runner.output)
                journal = gui_probe.records(attempt / 'events.jsonl')
                self.assertTrue(any(row.get('event') == 'phase' and row.get('phase') == 'setup'
                                    and row.get('nodeid', '').endswith('::test_first') for row in journal))
                self.assertFalse(any(row.get('event') == 'result' for row in journal),
                                 'Crash must precede the first pytest phase report')
                cases = ET.parse(runner.output / 'tests.xml').getroot().findall('.//testcase')
                self.assertEqual([case.get('name') for case in cases], ['test_first'])
                self.assertIsNotNone(cases[0].find('error'), 'First setup crash was omitted or reported as passed')
                self.assertIsNone(cases[0].find('failure'))

    def test_module_collection_error_is_retained_without_running_cases(self):
        with self.runner('''
raise ImportError('intentional module collection failure')

def test_never(request):
    record(request, 'unexpected body')
''') as runner:
            runner.start()
            result = self.final(runner, state='failed')
            self.assertFalse(result['timed_out'])
            self.assertFalse(result['cancelled'])
            self.assertFalse((runner.output / 'executions').exists())
            attempt, = gui_probe.attempts(runner.output)
            self.assertTrue(any(row.get('event') == 'collection_result' and row.get('outcome') == 'failed'
                                for row in gui_probe.records(attempt / 'events.jsonl')))
            cases = ET.parse(runner.output / 'tests.xml').getroot().findall('.//testcase')
            self.assertEqual(len(cases), 1)
            self.assertIsNotNone(cases[0].find('error'))
            self.assertIn('intentional module collection failure', ET.tostring(cases[0], encoding='unicode'))
            self.assertEqual(result['case_counts'], dict(tests=1, failures=0, errors=1, skipped=0))

    def test_custody_death_during_execution_or_cleanup_is_unsafe(self):
        for phase in ('execution', 'cleanup'):
            with self.subTest(phase=phase), self.runner('''
def test_active(request):
    record(request, 'active')
    tree(request)
    # A real registered receipt keeps cleanup observable without replacing code.
    (output(request) / 'snapshot.json').write_text(json.dumps({
        'flags': 0, 'scheme': '', 'context': {}}), encoding='utf-8')
    (output(request) / 'recoveries.jsonl').write_text(json.dumps({
        'kind': 'high_contrast', 'snapshot': 'snapshot.json',
        'receipt': 'unresolved-watchdog-result.json'}) + '\\n', encoding='utf-8')
    with deadline.operation('custody death barrier', 30):
        time.sleep(60)

def test_after(request):
    record(request, 'after')
''', policy={'cleanup_receipt': 10}) as runner:
                runner.start()
                attempt, _ = runner.event('call', operation='custody death barrier')
                handles = self.handles(attempt)
                if phase == 'cleanup':
                    (runner.output / 'cancel').touch()
                    runner.event('cleanup', prefix='runner-', operation='verify high-contrast recovery receipt')
                handles['custody'].kill()
                result = self.final(runner, state='unsafe', cleanup=False)
                self.assertEqual(self.executions(runner), ['active'])
                self.assertEqual(len(gui_probe.attempts(runner.output)), 1)
                self.assertTrue(result['error'])
                self.assert_tree_dead(handles)

    def test_custody_report_is_not_final_until_process_exit(self):
        with self.runner('def test_success():\n    pass\n', debug=True) as runner:
            runner.start()

            def inspect(pid):
                attempts = gui_probe.attempts(runner.output)
                evidence = json.loads((attempts[0] / 'custody-result.json').read_text())
                self.assertTrue(evidence['cleanup_complete'], evidence)
                # The OS is holding the real custody exit, after evidence publication.
                for _ in range(5):
                    self.assertFalse((runner.output / 'run.json').exists(), 'evidence was promoted before custody exit')
                    self.assertIsNone(runner.process.poll())
                    time.sleep(.1)

            gui_probe.hold_custody_exit(runner, inspect)
            self.final(runner, state='passed')

    def test_cleanup_failure_rejects_continuation(self):
        with self.runner('''
def test_active(request):
    record(request, 'active')
    (output(request) / 'fault-cleanup.json').write_text(json.dumps({
        'cleanup_complete': False, 'errors': ['contract recovery failure']}), encoding='utf-8')
    with deadline.operation('timeout before failed recovery', .5):
        time.sleep(60)

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            result = self.final(runner, state='unsafe', cleanup=False)
            self.assertTrue(result['timed_out'])
            self.assertIn('contract recovery failure', result['cleanup_errors'])
            self.assertEqual(self.executions(runner), ['active'])
            self.assertEqual(len(gui_probe.attempts(runner.output)), 1)

    def test_native_hc_requires_appearance_before_registering_or_changing_state(self):
        with self.runner('''
from driver.high_contrast import temporary_high_contrast
from driver.high_contrast_watchdog import high_contrast

def test_invalid_hc_request(request):
    artifact = output(request) / 'rejected-hc.json'
    before = high_contrast()
    with pytest.raises(ValueError):
        with temporary_high_contrast(before, artifact, None):
            raise AssertionError('missing appearance must not enter a native transition')
    assert high_contrast() == before
    assert not artifact.exists()
    assert not (output(request) / 'recoveries.jsonl').exists()
''') as runner:
            runner.start()
            self.final(runner, state='passed')

    def test_completed_recovery_receipt_accepts_removed_snapshot(self):
        # Ledger evidence acceptance only; no native HC recovery or setting change.
        with self.runner('''
from driver.high_contrast import register_recovery

def test_completed_recovery(request):
    artifact = output(request) / 'completed-snapshot.json'
    expected = {'flags': 0, 'scheme': '', 'context': {}}
    register_recovery(artifact, 'Winlex.Tests.HighContrast.' + 'a' * 32)
    artifact.write_text(json.dumps(expected), encoding='utf-8')
    obligation = json.loads((output(request) / 'recoveries.jsonl').read_text(encoding='utf-8'))
    receipt = output(request) / obligation['receipt']
    receipt.write_text(json.dumps({'ok': True, 'task_cleanup_complete': True,
                                  'expected': expected}), encoding='utf-8')
    artifact.unlink()
    record(request, 'completed recovery')
''') as runner:
            runner.start()
            result = self.final(runner, state='passed')
            self.assertFalse(result['timed_out'])
            self.assertEqual(result['case_counts'], dict(tests=1, failures=0, errors=0, skipped=0))
            attempt, = gui_probe.attempts(runner.output)
            obligation, = gui_probe.records(attempt / 'recoveries.jsonl')
            self.assertFalse((attempt / obligation['snapshot']).exists())
            receipt = json.loads((attempt / obligation['receipt']).read_text(encoding='utf-8'))
            self.assertEqual(receipt['expected'], {'flags': 0, 'scheme': '', 'context': {}})
            self.assertTrue(receipt['ok'])
            self.assertTrue(receipt['task_cleanup_complete'])
            self.assertEqual(self.executions(runner), ['completed recovery'])

    def test_older_recovery_receipt_cannot_discharge_new_unready_registration(self):
        # Ledger obligations only: no task is scheduled and no HC state is changed.
        with self.runner('''
from driver.high_contrast import register_recovery

def test_active(request):
    artifact = output(request) / 'shared-snapshot.json'
    expected = {'flags': 0, 'scheme': '', 'context': {}}
    register_recovery(artifact, 'Winlex.Tests.HighContrast.' + 'a' * 32)
    artifact.write_text(json.dumps(expected), encoding='utf-8')
    old = json.loads((output(request) / 'recoveries.jsonl').read_text(encoding='utf-8'))
    (output(request) / old['receipt']).write_text(json.dumps({
        'ok': True, 'task_cleanup_complete': True, 'expected': expected}), encoding='utf-8')
    register_recovery(artifact, 'Winlex.Tests.HighContrast.' + 'b' * 32)
    record(request, 'active')
    with deadline.operation('timeout after new recovery registration', .5):
        time.sleep(60)

def test_after(request):
    record(request, 'after')
''', policy={'cleanup_receipt': 1}) as runner:
            runner.start()
            result = self.final(runner, state='unsafe', cleanup=False)
            self.assertTrue(result['timed_out'])
            self.assertEqual(result['timeout']['operation'], 'timeout after new recovery registration')
            attempt, = gui_probe.attempts(runner.output)
            old, new = gui_probe.records(attempt / 'recoveries.jsonl')
            self.assertEqual(old['snapshot'], new['snapshot'])
            self.assertNotEqual(old['receipt'], new['receipt'])
            receipt = json.loads((attempt / old['receipt']).read_text(encoding='utf-8'))
            self.assertTrue(receipt['ok'])
            self.assertTrue(receipt['task_cleanup_complete'])
            self.assertFalse((attempt / new['receipt']).exists())
            self.assertEqual(list(attempt.glob('*.ready')), [])
            self.assertTrue(any(new['receipt'] in error for error in result['cleanup_errors']), result)
            self.assertEqual(self.executions(runner), ['active'])

    def test_stall_limit_stops_unproductive_attempts_but_completed_work_resets_it(self):
        for progress in (False, True):
            middle = '''
def test_middle(request):
    record(request, 'middle')
''' if progress else ''
            with self.subTest(completed_work=progress), self.runner('''
def test_first(request):
    record(request, 'first')
    blocked_native_with_gil()
''' + middle + '''
def test_second(request):
    record(request, 'second')
    blocked_native_with_gil()

def test_after(request):
    record(request, 'after')
''', extra=('--stall-after', '.5', '--case-timeout', '30', '--consecutive-stalls', '2')) as runner:
                runner.start()
                result = self.final(runner, state='timed_out')
                self.assertEqual(result['termination_reason'], 'stalled' if progress else 'repeated_stalls')
                self.assertEqual(self.executions(runner),
                                 ['first', 'middle', 'second', 'after'] if progress else ['first', 'second'])
                self.assertEqual(result['remaining'], 0 if progress else 1)
                self.assertEqual(len(result['case_timeouts']), 2)
                self.assertTrue(all(row['stalled'] for row in result['case_timeouts']))
                self.assertEqual(result['case_counts'],
                                 dict(tests=4 if progress else 2, failures=2, errors=0, skipped=0))
                for attempt in gui_probe.attempts(runner.output):
                    if (attempt / 'stop.json').exists():
                        self.assertIn('blocked_native_with_gil', (attempt / 'timeout-stacks.log').read_text())

    def test_aggregate_preserves_failure_skip_and_timeout_phase_errors_exactly_once(self):
        with self.runner('''
@pytest.fixture
def blocked_setup(request):
    record(request, 'setup')
    with deadline.operation('blocked fixture setup', .5):
        time.sleep(60)

@pytest.fixture
def blocked_teardown(request):
    yield
    record(request, 'teardown')
    with deadline.operation('blocked fixture teardown', .5):
        time.sleep(60)

def test_failure(request):
    record(request, 'failure')
    raise AssertionError('ordinary failure retained')

def test_skip(request):
    record(request, 'skip')
    pytest.skip('intentional capability skip')

def test_blocked_call(request):
    record(request, 'call timeout')
    with deadline.operation('blocked call', .5):
        time.sleep(60)

def test_blocked_setup(request, blocked_setup):
    record(request, 'unexpected setup body')

def test_blocked_teardown(request, blocked_teardown):
    record(request, 'teardown body')
    raise AssertionError('call failure before teardown timeout')

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            result = self.final(runner, state='timed_out')
            self.assertEqual(self.executions(runner),
                             ['failure', 'skip', 'call timeout', 'setup', 'teardown body', 'teardown', 'after'])
            self.assertEqual(result['case_counts'], dict(tests=6, failures=3, errors=2, skipped=1))
            self.assertEqual([(row['phase'], row['operation']) for row in result['case_timeouts']],
                             [('call', 'blocked call'), ('setup', 'blocked fixture setup'),
                              ('teardown', 'blocked fixture teardown')])
            cases = ET.parse(runner.output / 'tests.xml').getroot().findall('.//testcase')
            self.assertEqual([case.get('name') for case in cases],
                             ['test_failure', 'test_skip', 'test_blocked_call', 'test_blocked_setup',
                              'test_blocked_teardown', 'test_after'])
            by_name = {case.get('name'): case for case in cases}
            self.assertIn('ordinary failure retained', ET.tostring(by_name['test_failure'], encoding='unicode'))
            self.assertIsNotNone(by_name['test_skip'].find('skipped'))
            self.assertIsNotNone(by_name['test_blocked_setup'].find('error'))
            self.assertIsNotNone(by_name['test_blocked_teardown'].find('error'))
            self.assertIn('call failure before teardown timeout',
                          ET.tostring(by_name['test_blocked_teardown'].find('failure'), encoding='unicode'))
            self.assertIsNone(by_name['test_after'].find('error'))
            self.assertIsNone(by_name['test_after'].find('failure'))

    def test_nested_deadlines_bound_longer_children_and_restore_after_normal_or_caught_exit(self):
        with self.runner('''
def test_longer_inner(request):
    record(request, 'longer inner')
    with deadline.operation('outer bounds longer inner', .2):
        with deadline.operation('longer inner', 5):
            time.sleep(60)

def test_completed_shorter_inner(request):
    record(request, 'completed shorter inner')
    with deadline.operation('outer restored after inner', .4):
        with deadline.operation('completed shorter inner', .2):
            pass
        time.sleep(60)

@pytest.mark.parametrize('child_budget', [.2, 5])
def test_caught_inner_exception(request, child_budget):
    record(request, 'caught ' + str(child_budget))
    with deadline.operation('parent after caught child exception', .4):
        try:
            with deadline.operation('failing nested operation', child_budget):
                raise ValueError('ordinary child failure')
        except ValueError:
            pass
        time.sleep(60)

def test_after(request):
    record(request, 'after')
''') as runner:
            runner.start()
            result = self.final(runner, state='timed_out')
            self.assertEqual(self.executions(runner),
                             ['longer inner', 'completed shorter inner', 'caught 0.2', 'caught 5', 'after'])
            self.assertEqual([(row['operation'], row['budget']) for row in result['case_timeouts']],
                             [('outer bounds longer inner', .2), ('outer restored after inner', .4),
                              ('parent after caught child exception', .4),
                              ('parent after caught child exception', .4)])
            self.assertEqual(result['case_counts'], dict(tests=5, failures=4, errors=0, skipped=0))

    def test_collection_and_last_case_timeouts_have_no_replacement_selection(self):
        scenarios = {
            'collection': '''
with deadline.operation('blocked module collection', .5):
    time.sleep(60)

def test_never(request):
    record(request, 'unexpected body')
''',
            'last case': '''
def test_only(request):
    record(request, 'only')
    with deadline.operation('last selected case blocks', .5):
        time.sleep(60)
''',
        }
        for scenario, source in scenarios.items():
            with self.subTest(scenario=scenario), self.runner(source) as runner:
                runner.start()
                result = self.final(runner, state='timed_out')
                attempt, = gui_probe.attempts(runner.output)
                if scenario == 'collection':
                    self.assertEqual(result['timeout']['phase'], 'collection')
                    self.assertFalse((attempt / 'selection.json').exists())
                    self.assertFalse((runner.output / 'executions').exists())
                else:
                    self.assertEqual(self.executions(runner), ['only'])
                    self.assertEqual(result['remaining'], 0)
                    self.assertEqual(result['case_counts'], dict(tests=1, failures=1, errors=0, skipped=0))

    def test_late_successful_callback_and_truthy_wait_are_failures_not_success(self):
        with self.runner('''
from support import native as n

def test_late_callback(request):
    record(request, 'callback')
    with deadline.operation('late successful callback', .1):
        time.sleep(.2)

def test_late_truthy_wait(request):
    record(request, 'wait')
    def late_result():
        time.sleep(.2)
        return True
    n.wait(late_result, 'late truthy predicate', .1)

def test_after(request):
    record(request, 'after')
    assert n.top_level_windows() == []
    assert n.find_window(0, 'Winlex.Frame') is None
''') as runner:
            runner.start()
            result = self.final(runner, state='failed')
            self.assertFalse(result['timed_out'], result)
            self.assertEqual(self.executions(runner), ['callback', 'wait', 'after'])
            self.assertEqual(result['case_counts'], dict(tests=3, failures=2, errors=0, skipped=0))
            cases = ET.parse(runner.output / 'tests.xml').getroot().findall('.//testcase')
            self.assertIn('late successful callback', ET.tostring(cases[0].find('failure'), encoding='unicode'))
            self.assertIn('late truthy predicate', ET.tostring(cases[1].find('failure'), encoding='unicode'))
            self.assertIsNone(cases[2].find('failure'))
            self.assertIsNone(cases[2].find('error'))

    def test_custody_exit_cancellation_or_expired_budget_gates_continuation(self):
        for mode in ('completed cancellation', 'interrupted cancellation', 'interrupted budget'):
            interrupted = mode != 'completed cancellation'
            cancelled = mode != 'interrupted budget'
            source = '''
def test_interrupted():
    with deadline.operation('timeout before cleanup cancellation', .5):
        time.sleep(60)

def test_after(request):
    record(request, 'unexpected following case')
''' if interrupted else 'def test_completed():\n    pass\n'
            with self.subTest(mode=mode), self.runner(source, debug=True, timeout=30 if cancelled else 8) as runner:
                runner.start()

                def cancel(pid):
                    attempt, = gui_probe.attempts(runner.output)
                    evidence = json.loads((attempt / 'custody-result.json').read_text(encoding='utf-8'))
                    self.assertTrue(evidence['cleanup_complete'], evidence)
                    self.assertFalse((runner.output / 'run.json').exists())
                    if cancelled:
                        (runner.output / 'cancel').touch()
                    else:
                        invocation = json.loads((attempt / 'invocation.json').read_text(encoding='utf-8'))
                        gui_probe.wait_for(lambda: time.monotonic() >= invocation['until'], 10,
                                           'Original execution budget did not expire at the OS exit barrier')

                gui_probe.hold_custody_exit(runner, cancel)
                result = self.final(runner, state='cancelled' if cancelled else 'timed_out')
                self.assertEqual(result['cancelled'], cancelled)
                if not cancelled:
                    self.assertEqual(result['termination_reason'], 'budget_exhausted')
                self.assertEqual(result['timed_out'], interrupted)
                self.assertEqual(result['case_counts'], dict(tests=1, failures=int(interrupted), errors=0, skipped=0))
                self.assertEqual(len(gui_probe.attempts(runner.output)), 1)
                self.assertFalse((runner.output / 'executions').exists())
                case, = ET.parse(runner.output / 'tests.xml').getroot().findall('.//testcase')
                self.assertEqual(case.get('name'), 'test_interrupted' if interrupted else 'test_completed')
                self.assertIsNone(case.find('error'))
                if interrupted:
                    self.assertIsNotNone(case.find('failure'))
                else:
                    self.assertIsNone(case.find('failure'))

    def test_suspended_cleanup_is_killed_at_parent_deadline_without_continuation(self):
        import ctypes
        from ctypes import wintypes
        import win32api

        with self.runner('''
def test_active(request):
    record(request, 'active')
    tree(request)
    (output(request) / 'snapshot.json').write_text(json.dumps({
        'flags': 0, 'scheme': '', 'context': {}}), encoding='utf-8')
    (output(request) / 'recoveries.jsonl').write_text(json.dumps({
        'kind': 'high_contrast', 'snapshot': 'snapshot.json',
        'receipt': 'missing-recovery-result.json'}) + '\\n', encoding='utf-8')
    with deadline.operation('timeout before cleanup suspension', 2):
        time.sleep(60)

def test_after(request):
    record(request, 'after')
''', policy={'cleanup_receipt': 1}) as runner:
            runner.start()
            attempt, _ = runner.event('call', operation='timeout before cleanup suspension')
            handles = self.handles(attempt)
            pids = json.loads((attempt / 'processes.json').read_text(encoding='utf-8'))
            suspend_handle = win32api.OpenProcess(0x0800, False, pids['custody'])
            ntdll = ctypes.WinDLL('ntdll')
            suspend = ntdll.NtSuspendProcess
            resume = ntdll.NtResumeProcess
            for function in (suspend, resume):
                function.argtypes = [wintypes.HANDLE]
                function.restype = ctypes.c_long
            try:
                runner.event('cleanup', prefix='runner-', operation='verify high-contrast recovery receipt')
                self.assertEqual(suspend(int(suspend_handle)), 0, 'Cannot suspend real custody process')
                result = self.final(runner, state='unsafe', cleanup=False)
                self.assertIn('deadline', result['error'])
                self.assertTrue(result['timed_out'])
                self.assertEqual(self.executions(runner), ['active'])
                self.assertEqual(len(gui_probe.attempts(runner.output)), 1)
                self.assert_tree_dead(handles)
            finally:
                if handles['custody'].alive():
                    resume(int(suspend_handle))
                suspend_handle.Close()

    def test_denied_input_ownership_never_consumes_protected_ledger(self):
        import win32api
        import win32event
        mutex = win32event.CreateMutex(None, False, 'Local\\Winlex.Tests.Input')
        owned = win32event.WaitForSingleObject(mutex, 0) in (0, 128)
        if not owned:
            mutex.Close()
            self.skipTest('another test owns the real input mutex')
        try:
            with self.runner('''
@pytest.mark.interactive
def test_never():
    raise AssertionError('denied input owner must never start pytest')
''', extra=('--desktop', 'current', '--real-input'), debug=True) as runner:
                protected = b'{"held": [], "foreground": 0, "cursor": [0, 0]}'

                def prepare(pid):
                    attempt, = gui_probe.attempts(runner.output)
                    ledger = attempt / 'profiles' / 'input.json'
                    ledger.parent.mkdir(parents=True, exist_ok=True)
                    ledger.write_bytes(protected)

                runner.start()
                gui_probe.hold_custody_exit(runner, lambda pid: None, on_create=prepare)
                result = self.final(runner, state='unsafe', cleanup=False)
                attempt, = gui_probe.attempts(runner.output)
                ledger = attempt / 'profiles' / 'input.json'
                self.assertIn('Another test owns input', result['custody_error'])
                self.assertEqual(ledger.read_bytes(), protected)
                self.assertFalse((ledger.parent.parent / 'events.jsonl').exists())
        finally:
            win32event.ReleaseMutex(mutex)
            win32api.CloseHandle(mutex)


if __name__ == '__main__':
    unittest.main()

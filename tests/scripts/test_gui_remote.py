"""Public queue status follows worker liveness and whole-run completion."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.name == 'nt', 'requires Windows process handles')
class WorkerAvailability(unittest.TestCase):
    def test_fresh_heartbeat_stops_advertising_worker_after_process_exit(self):
        sys.path.insert(0, str(ROOT / 'tests/gui'))
        try:
            import remote
            with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                root = Path(directory)
                process = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
                try:
                    (root / 'capabilities.json').write_text(json.dumps({
                        'pid': process.pid, 'heartbeat': time.time(), 'worker_available': True,
                    }), encoding='utf-8')
                    self.assertTrue(remote.rpc(root, {'action': 'capabilities'})['worker_available'])
                    process.terminate()
                    process.wait(timeout=5)
                    status = remote.rpc(root, {'action': 'capabilities'})
                    self.assertLess(status['heartbeat_age'], 10)
                    self.assertFalse(status['worker_available'])
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait(timeout=5)
        finally:
            sys.path.pop(0)


@unittest.skipUnless(os.name == 'nt' and importlib.util.find_spec('uiautomation'),
                     'requires the Windows GUI test dependencies')
class WholeRunCompletion(unittest.TestCase):
    def test_terminal_deadline_preserves_frozen_cleanup_ceiling(self):
        sys.path.insert(0, str(ROOT / 'tests/gui'))
        try:
            import remote
            for hang in (False, True):
                with self.subTest(hang_after_terminal=hang), tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                    output = Path(directory)
                    started = time.monotonic()
                    ceiling = started + (.4 if hang else 5)
                    (output / 'runner-cleanup.jsonl').write_text(
                        json.dumps({'attempt': '.', 'deadline': ceiling}) + '\n')
                    (output / 'runner-deadlines.jsonl').write_text(json.dumps({
                        'phase': 'cleanup', 'operation': 'cleanup', 'deadline': ceiling}) + '\n')
                    script = '''
import json, sys, time
from pathlib import Path
output = Path(sys.argv[1])
time.sleep(.25)
with (output / 'runner-deadlines.jsonl').open('a') as stream:
    stream.write('null\\n')
(output / 'terminal-published').touch()
time.sleep(60 if sys.argv[2] == 'True' else .25)
(output / 'run.json').write_text(json.dumps({'exit_code': 0, 'cleanup_complete': True}))
'''
                    process = subprocess.Popen([sys._base_executable, '-c', script, str(output), str(hang)])
                    try:
                        # Cleanup may legitimately outlive execution/startup.
                        safe = remote.supervise_runner(process, output, .1, started - 20, lambda: None)
                        self.assertEqual(safe, not hang)
                        self.assertTrue((output / 'terminal-published').exists())
                        if hang:
                            self.assertGreaterEqual(time.monotonic(), ceiling)
                            self.assertLess(time.monotonic() - started, 2.5)
                            failure = json.loads((output / 'supervisor.json').read_text())
                            self.assertTrue(failure['runner_terminated'])
                            self.assertFalse(failure['cleanup_complete'])
                        else:
                            self.assertEqual(process.returncode, 0)
                    finally:
                        if process.poll() is None:
                            process.kill()
                        process.wait(timeout=5)
        finally:
            sys.path.pop(0)

    def test_uncommitted_deadline_does_not_override_supervision(self):
        sys.path.insert(0, str(ROOT / 'tests/gui'))
        try:
            import remote
            with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                output = Path(directory)
                script = '''
import json, sys, time
from pathlib import Path
output = Path(sys.argv[1])
now = time.monotonic()
journal = output / 'runner-deadlines.jsonl'
journal.write_text(json.dumps({'phase': 'startup', 'operation': 'startup', 'deadline': now + 5}) + '\\n')
with journal.open('a') as stream:
    stream.write(json.dumps({'phase': 'cleanup-inventory', 'operation': 'uncommitted', 'deadline': now + .1}))
time.sleep(.7)
(output / 'run.json').write_text(json.dumps({'exit_code': 0, 'cleanup_complete': True}))
'''
                started = time.monotonic()
                process = subprocess.Popen([sys.executable, '-c', script, str(output)])
                try:
                    self.assertTrue(remote.supervise_runner(process, output, 2, started, lambda: None))
                    self.assertEqual(process.returncode, 0)
                finally:
                    if process.poll() is None:
                        process.kill()
                    process.wait(timeout=5)
        finally:
            sys.path.pop(0)

    def test_status_and_watch_wait_for_real_timeout_continuation(self):
        sys.path.insert(0, str(ROOT / 'tests/gui'))
        try:
            import remote
            with tempfile.TemporaryDirectory(dir=ROOT / 'tmp') as directory:
                root = Path(directory)
                output = root / ('0' * 32)
                output.mkdir()
                # The queue worker's claim is independent of runner completion.
                (output / 'started.json').write_text('{}', encoding='utf-8')
                entered, release = root / 'entered', root / 'release'
                probe = root / 'test_probe.py'
                probe.write_text('''
import time
from pathlib import Path
from support import deadline
def test_first_completion_contract_probe():
    with deadline.operation('blocked completion probe', .1):
        time.sleep(60)
def test_second_completion_contract_probe():
    Path(%r).touch()
    until = time.monotonic() + 15
    while not Path(%r).exists():
        assert time.monotonic() < until, 'parent did not release continuation'
        time.sleep(.02)
''' % (str(entered), str(release)), encoding='utf-8')
                with (root / 'runner.log').open('w', encoding='utf-8') as log:
                    process = subprocess.Popen([
                        sys.executable, str(ROOT / 'tests/gui/run.py'),
                        '--output', str(output), '--timeout', '40', '--exe', sys.executable,
                        str(probe), '--ignore-glob=*', '-k', 'completion_contract_probe', '-q',
                    ], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
                    try:
                        until = time.monotonic() + 25
                        while not entered.exists() and process.poll() is None and time.monotonic() < until:
                            time.sleep(.02)
                        self.assertTrue(entered.exists(), (root / 'runner.log').read_text(encoding='utf-8'))
                        request = {'run_id': '0' * 32}
                        self.assertEqual(remote.rpc(root, dict(request, action='status'))['state'], 'running')
                        watching = remote.rpc(root, dict(request, action='watch', timeout=.1, slow_after=60))
                        self.assertEqual(watching['state'], 'running')
                        self.assertEqual(watching['watch_event'], 'watch_timeout')
                        self.assertFalse((output / 'run.json').exists())
                        release.touch()
                        process.wait(timeout=15)
                        final = remote.rpc(root, dict(request, action='watch', timeout=1, slow_after=60))
                        self.assertEqual(final['watch_event'], 'finished')
                        self.assertEqual(final['exit_code'], 1)
                        self.assertTrue(final['cleanup_complete'], final)
                        self.assertEqual(final['case_counts'], {'tests': 2, 'failures': 1, 'errors': 0, 'skipped': 0})
                        self.assertTrue(json.loads((output / 'attempt-result.json').read_text())['timed_out'])
                        self.assertEqual(json.loads((output / 'continued/1/run.json').read_text())['exit_code'], 0)
                        self.assertIn('test_first_completion_contract_probe',
                                      (output / 'timeout-stacks.log').read_text(encoding='utf-8'))
                    finally:
                        release.touch()
                        if process.poll() is None:
                            (output / 'cancel').touch()
                            try:
                                process.wait(timeout=10)
                            except subprocess.TimeoutExpired:
                                process.kill()
                                process.wait(timeout=5)
        finally:
            sys.path.pop(0)


if __name__ == '__main__':
    unittest.main()

"""Queue behavior through real worker processes, not mocked launch functions."""
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
GUI = ROOT / 'tests/gui'
AVAILABLE = os.name == 'nt' and all(importlib.util.find_spec(name) for name in
                                   ('win32api', 'pytest', 'uiautomation', 'comtypes'))


def remote_module():
    sys.path.insert(0, str(GUI))
    spec = importlib.util.spec_from_file_location('queue_contract_remote', GUI / 'remote.py')
    remote = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(remote)
    return remote


def evidence():
    root = ROOT / 'tmp'
    root.mkdir(exist_ok=True)
    folder = Path(tempfile.mkdtemp(prefix='gui-queue-contract-', dir=root))
    print('Queue evidence:', folder, flush=True)
    return folder


def worker(root):
    with (root / 'worker.log').open('w', encoding='utf-8') as log:
        process = subprocess.run([sys.executable, str(GUI / 'remote.py'), 'worker',
                                  '--root', str(root), '--once'], cwd=ROOT,
                                 stdout=log, stderr=subprocess.STDOUT, timeout=45)
    if process.returncode:
        raise AssertionError((root / 'worker.log').read_text(encoding='utf-8'))


def submit(remote, root, run_id, source='def test_probe():\n    pass\n'):
    probe = root / ('probe_' + run_id + '.py')
    probe.write_text(source, encoding='utf-8')
    request = {'action': 'submit', 'run_id': run_id, 'timeout': 15,
               'tests': ['--exe', sys.executable, str(probe), '-o',
                         'python_files=' + probe.name, '--ignore-glob=*', '-q'],
               'desktop': 'current'}
    remote.rpc(root, request)
    return request


@unittest.skipUnless(AVAILABLE, 'requires Windows GUI execution dependencies')
class QueueContracts(unittest.TestCase):
    def test_completed_process_publishes_one_final_result(self):
        remote, root = remote_module(), evidence()
        request = submit(remote, root, '0' * 32)
        worker(root)
        status = remote.rpc(root, dict(request, action='watch', slow_after=60))
        self.assertEqual(status['watch_event'], 'finished')
        self.assertEqual(status['state'], 'passed', status)
        self.assertTrue(status['cleanup_complete'], status)
        self.assertFalse((root / request['run_id'] / 'supervisor.json').exists())
        self.assertFalse(remote.rpc(root, {'action': 'capabilities'})['worker_available'])

    def test_claim_owner_exit_and_pid_reuse_are_interrupted_not_running(self):
        import win32api
        import win32con
        import win32process
        remote, root = remote_module(), evidence()
        request = submit(remote, root, '0' * 32)
        folder = root / request['run_id']
        process = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(60)'])
        try:
            handle = win32api.OpenProcess(win32con.PROCESS_QUERY_INFORMATION, False, process.pid)
            try:
                created = win32process.GetProcessTimes(handle)['CreationTime'].timestamp()
            finally:
                win32api.CloseHandle(handle)
            claim = {'worker': process.pid, 'created': created, 'time': time.time()}
            (folder / 'started.json').write_text(json.dumps(claim), encoding='utf-8')
            self.assertEqual(remote.rpc(root, dict(request, action='status'))['state'], 'running')
            # Historical claims lack a birth identity. Use the known birth as
            # their timestamp: time.time() can lag GetProcessTimes on Windows.
            legacy = {'worker': process.pid, 'time': created}
            (folder / 'started.json').write_text(json.dumps(legacy), encoding='utf-8')
            self.assertEqual(remote.rpc(root, dict(request, action='status'))['state'], 'running')
            process.terminate()
            process.wait(timeout=5)
            status = remote.rpc(root, dict(request, action='watch', timeout=.1, slow_after=60))
            self.assertEqual(status['state'], 'interrupted')
            self.assertFalse(status['cleanup_complete'])
            # An actual live PID with a mismatched birth identity is not owner.
            created = win32process.GetProcessTimes(win32api.GetCurrentProcess())['CreationTime'].timestamp()
            claim.update(worker=os.getpid(), created=created - 1)
            (folder / 'started.json').write_text(json.dumps(claim), encoding='utf-8')
            self.assertEqual(remote.rpc(root, dict(request, action='status'))['state'], 'interrupted')
            (root / 'capabilities.json').write_text(json.dumps({
                'pid': os.getpid(), 'created': created - 1, 'heartbeat': time.time(),
                'worker_available': True}), encoding='utf-8')
            self.assertFalse(remote.rpc(root, {'action': 'capabilities'})['worker_available'])
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)

    def test_orphan_blocks_even_earlier_sorted_request_without_replay(self):
        remote, root = remote_module(), evidence()
        pending = submit(remote, root, '0' * 32)
        orphan = submit(remote, root, 'f' * 32)
        (root / orphan['run_id'] / 'started.json').write_text(json.dumps({
            'worker': os.getpid(), 'time': 0}), encoding='utf-8')
        worker(root)
        caps = remote.rpc(root, {'action': 'capabilities'})
        self.assertFalse(caps['worker_available'], caps)
        self.assertEqual(remote.rpc(root, dict(orphan, action='status'))['state'], 'interrupted')
        self.assertEqual(remote.rpc(root, dict(pending, action='status'))['state'], 'queued')
        self.assertFalse((root / pending['run_id'] / 'started.json').exists())
        self.assertFalse((root / orphan['run_id'] / 'attempts.jsonl').exists())

    def test_startup_failure_stops_queue_and_restart_is_not_recovery(self):
        remote, root = remote_module(), evidence()
        broken = submit(remote, root, '0' * 32)
        pending = submit(remote, root, 'f' * 32)
        (root / broken['run_id'] / 'request.json').write_text('{', encoding='utf-8')
        worker(root)
        status = remote.rpc(root, dict(broken, action='status'))
        self.assertEqual(status['state'], 'unsafe', status)
        self.assertFalse(status['cleanup_complete'], status)
        self.assertEqual(remote.rpc(root, dict(pending, action='status'))['state'], 'queued')
        worker(root)
        self.assertEqual(remote.rpc(root, dict(pending, action='status'))['state'], 'queued')
        self.assertFalse((root / pending['run_id'] / 'started.json').exists())

    def test_cancel_before_claim_does_not_launch_custody(self):
        remote, root = remote_module(), evidence()
        request = submit(remote, root, '0' * 32)
        remote.rpc(root, dict(request, action='cancel'))
        worker(root)
        status = remote.rpc(root, dict(request, action='status'))
        self.assertTrue(status['cancelled'], status)
        self.assertTrue(status['cleanup_complete'], status)
        self.assertFalse((root / request['run_id'] / 'attempts.jsonl').exists())

    def test_historical_supervisor_failure_still_overrides_old_run_success(self):
        remote, root = remote_module(), evidence()
        request = submit(remote, root, '0' * 32)
        folder = root / request['run_id']
        (folder / 'run.json').write_text(json.dumps({'exit_code': 0, 'cleanup_complete': True}), encoding='utf-8')
        (folder / 'supervisor.json').write_text(json.dumps({
            'state': 'supervisor_failed', 'cleanup_complete': False}), encoding='utf-8')
        status = remote.rpc(root, dict(request, action='status'))
        self.assertEqual(status['state'], 'supervisor_failed')
        self.assertFalse(status['cleanup_complete'])


if __name__ == '__main__':
    unittest.main()

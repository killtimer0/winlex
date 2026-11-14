"""Shared bounded probes for the GUI runner and supervisor contracts."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parents[2]
GUI = ROOT / 'tests' / 'gui'
REMOTE = GUI / 'remote.py'

sys.path.insert(0, str(GUI))
from support import bounded  # noqa: E402


def load_remote():
    spec = importlib.util.spec_from_file_location('deadline_remote', REMOTE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_probe(folder, source, name='test_deadline_probe.py'):
    path = Path(folder) / name
    path.write_text(source, encoding='utf-8')
    return path


def runner_command(probe, output, *, timeout=10, extra=()):
    command = [
        sys.executable, str(GUI / 'run.py'), '--output', str(output),
        '--timeout', str(timeout), '--exe', sys.executable, str(probe),
        '-o', 'python_files=' + Path(probe).name, '--ignore-glob=*', '-q',
    ]
    command.extend(extra)
    return command


def run_runner(probe, output, *, timeout=10, extra=(), deadline=20):
    return subprocess.run(runner_command(probe, output, timeout=timeout, extra=extra),
                          cwd=ROOT, capture_output=True, text=True,
                          encoding='utf-8', timeout=deadline)


def run_probe(folder, source, *, timeout=10, extra=(), deadline=20):
    folder = Path(folder)
    output = folder / 'artifacts'
    probe = write_probe(folder, source)
    process = run_runner(probe, output, timeout=timeout, extra=extra, deadline=deadline)
    return output, process


def supervise(folder, source, *, timeout=5, extra=(), run_id='0' * 32):
    folder = Path(folder)
    output = folder / run_id
    output.mkdir(exist_ok=True)
    wrapper = folder / 'runner_wrapper.py'
    wrapper.write_text('import sys\nsys.path.insert(0, %r)\n' % str(GUI) + source,
                       encoding='utf-8')
    remote = load_remote()
    started = time.monotonic()
    with (folder / 'wrapper.log').open('w', encoding='utf-8') as log:
        process = subprocess.Popen([sys.executable, str(wrapper), str(output), *extra],
                                   cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        try:
            safe = remote.supervise_runner(process, output, timeout, started, lambda: None)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
    return output, safe, remote


def write_wrapper(folder, source):
    path = Path(folder) / 'runner_wrapper.py'
    path.write_text('import sys\nsys.path.insert(0, %r)\n' % str(GUI) + source,
                    encoding='utf-8')
    return path


def run_wrapper(folder, source, args=(), *, deadline=20):
    wrapper = write_wrapper(folder, source)
    return subprocess.run([sys.executable, str(wrapper), *args], cwd=ROOT,
                          capture_output=True, text=True, encoding='utf-8',
                          timeout=deadline)


def read_run(output):
    return json.loads((Path(output) / 'run.json').read_text(encoding='utf-8'))


def wait_for(predicate, seconds, description):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        value = predicate()
        if value:
            return value
        time.sleep(.02)
    raise AssertionError(description)

"""Real CLI probes; journals and Windows process handles are the barriers.

Run these on the configured remote Windows test target, not the local desktop.
Failed probes retain their sources, console log and artifacts under tmp/.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
GUI = ROOT / 'tests' / 'gui'
REMOTE = GUI / 'remote.py'


def dependencies():
    return os.name == 'nt' and all(importlib.util.find_spec(name) is not None
                                   for name in ('pytest', 'uiautomation', 'comtypes', 'win32api'))


def load_remote():
    spec = importlib.util.spec_from_file_location('deadline_remote', REMOTE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_probe(folder, source, name='test_deadline_probe.py'):
    path = Path(folder) / name
    path.write_text(source, encoding='utf-8')
    return path


def runner_command(probe, output, *, timeout=30, extra=()):
    return [sys.executable, str(GUI / 'run.py'), '--output', str(output),
            '--timeout', str(timeout), '--exe', sys.executable, str(probe),
            '-o', 'python_files=' + Path(probe).name, '--ignore-glob=*', '-q', *extra]


def read_run(output):
    return json.loads((Path(output) / 'run.json').read_text(encoding='utf-8'))


def records(path):
    """Ignore only an uncommitted final line, never malformed committed evidence."""
    try:
        text = Path(path).read_text(encoding='utf-8')
    except FileNotFoundError:
        return []
    return [json.loads(line) for line in text.splitlines(keepends=True) if line.endswith('\n')]


def attempts(output):
    return [Path(output) / row['directory'] for row in records(Path(output) / 'attempts.jsonl')]


def wait_for(predicate, seconds, description):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        value = predicate()
        if value:
            return value
        time.sleep(.02)
    raise AssertionError(description)


class Runner:
    """One public CLI invocation, with no automatic retry or implementation patch."""

    def __init__(self, source, *, timeout=30, extra=(), policy=None, debug=False):
        base = ROOT / 'tmp' / 'gui-rewrite' / 'probes'
        base.mkdir(parents=True, exist_ok=True)
        self.folder = Path(tempfile.mkdtemp(prefix='deadline-', dir=base))
        # The GUI conftest must apply to the explicitly selected probe.
        self.probe = GUI / ('probe_' + self.folder.name.replace('-', '_') + '.py')
        self.probe.write_text(source, encoding='utf-8')
        (self.folder / 'probe.py').write_text(source, encoding='utf-8')
        self.output = self.folder / 'artifacts'
        self.output.mkdir()
        self.command = runner_command(self.probe, self.output, timeout=timeout, extra=extra)
        self.environment = dict(os.environ)
        if policy:
            self.environment['WINLEX_TEST_POLICY'] = json.dumps(policy)
        self.debug = debug
        self.process = None
        self.log = None

    def __enter__(self):
        return self

    def start(self):
        print('GUI probe: ' + str(self.folder), flush=True)
        self.log = (self.folder / 'console.log').open('w', encoding='utf-8')
        self.process = subprocess.Popen(self.command, cwd=ROOT, env=self.environment,
                                        stdout=self.log, stderr=subprocess.STDOUT,
                                        creationflags=1 if self.debug else 0)
        return self

    def diagnostic(self):
        return str(self.folder) + '\n' + (self.folder / 'console.log').read_text(encoding='utf-8')

    def wait(self, seconds=45):
        self.process.wait(timeout=seconds)
        if not (self.output / 'run.json').exists():
            raise AssertionError('CLI exited without final run.json: ' + self.diagnostic())
        return read_run(self.output)

    def event(self, phase, *, prefix='', operation=None, seconds=15):
        def find():
            for attempt in attempts(self.output):
                for row in records(attempt / (prefix + 'events.jsonl')):
                    if (row.get('event') in ('phase', 'operation') and row.get('phase') == phase
                            and (operation is None or row.get('operation') == operation)):
                        return attempt, row
            if self.process.poll() is not None:
                raise AssertionError('CLI exited before ' + phase + ': ' + self.diagnostic())
        return wait_for(find, seconds, 'No ' + phase + ' event: ' + str(self.folder))

    def __exit__(self, kind, value, traceback):
        if self.process is not None and self.process.poll() is None:
            subprocess.run(['taskkill', '/PID', str(self.process.pid), '/T', '/F'],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
            self.process.wait(timeout=15)
        if self.log:
            self.log.close()
        self.probe.unlink(missing_ok=True)
        if kind:
            print('Probe evidence retained: ' + str(self.folder), file=sys.stderr)


class ProcessHandle:
    def __init__(self, pid):
        import win32api
        self.handle = win32api.OpenProcess(0x100000 | 1, False, pid)

    def alive(self):
        import win32event
        return win32event.WaitForSingleObject(self.handle, 0) == 258

    def kill(self):
        import win32process
        win32process.TerminateProcess(self.handle, 91)

    def dead(self, seconds=10):
        import win32event
        return win32event.WaitForSingleObject(self.handle, int(seconds * 1000)) == 0

    def close(self):
        self.handle.Close()


def launch(source, **options):
    """Return a started Runner; use ``with launch(source) as runner: ...``."""
    return Runner(source, **options).start()


def invoke(source, **options):
    """Return (artifact directory, final result, CLI exit code), retaining evidence."""
    with launch(source, **options) as runner:
        result = runner.wait()
        return runner.output, result, runner.process.returncode


def hold_custody_exit(runner, inspect, *, on_create=None):
    """OS debugger barrier: inspect exit evidence before allowing custody to exit.

    DEBUG_PROCESS debugs real descendants too. An EXIT_PROCESS_DEBUG_EVENT is
    withheld until ContinueDebugEvent; no Python entry point is substituted.
    Must be called by the thread which started the debugged CLI.
    """
    import ctypes as c
    from ctypes import wintypes as w

    class Create(c.Structure):
        _fields_ = [('file', w.HANDLE), ('process', w.HANDLE), ('thread', w.HANDLE),
                    ('base', c.c_void_p), ('offset', w.DWORD), ('size', w.DWORD),
                    ('tls', c.c_void_p), ('start', c.c_void_p), ('image', c.c_void_p),
                    ('unicode', w.WORD)]

    class Data(c.Union):
        _fields_ = [('create', Create), ('code', w.DWORD), ('raw', c.c_byte * 160)]

    class Event(c.Structure):
        _fields_ = [('kind', w.DWORD), ('pid', w.DWORD), ('tid', w.DWORD), ('data', Data)]

    kernel = c.WinDLL('kernel32', use_last_error=True)
    kernel.WaitForDebugEvent.argtypes = [c.POINTER(Event), w.DWORD]
    kernel.ContinueDebugEvent.argtypes = [w.DWORD, w.DWORD, w.DWORD]
    kernel.CloseHandle.argtypes = [w.HANDLE]
    custody = None
    held = False
    failure = None
    until = time.monotonic() + 45
    while time.monotonic() < until:
        event = Event()
        if not kernel.WaitForDebugEvent(c.byref(event), 100):
            if c.get_last_error() == 121:
                continue
            raise c.WinError(c.get_last_error())
        status = 0x10002  # DBG_CONTINUE
        try:
            if event.kind == 3:  # CREATE_PROCESS_DEBUG_EVENT
                if event.pid != runner.process.pid and custody is None:
                    custody = event.pid
                    if on_create:
                        try:
                            on_create(custody)
                        except BaseException as error:
                            failure = error
                if event.data.create.file:
                    kernel.CloseHandle(event.data.create.file)
            elif event.kind == 6:  # LOAD_DLL_DEBUG_EVENT: first union member is hFile
                if event.data.create.file:
                    kernel.CloseHandle(event.data.create.file)
            elif event.kind == 1 and event.data.code != 0x80000003:
                status = 0x80010001  # DBG_EXCEPTION_NOT_HANDLED
            elif event.kind == 5 and event.pid == custody:
                held = True
                try:
                    inspect(custody)
                except BaseException as error:
                    if failure is None:
                        failure = error
        finally:
            if not kernel.ContinueDebugEvent(event.pid, event.tid, status):
                raise c.WinError(c.get_last_error())
        if event.kind == 5 and event.pid == runner.process.pid:
            if failure:
                raise failure
            if not held:
                raise AssertionError('CLI exited without a custody exit barrier: ' + runner.diagnostic())
            return
    raise AssertionError('Debugged CLI did not exit: ' + runner.diagnostic())

"""Publish operation deadlines for the supervising process; retain blocked stacks."""
from contextlib import contextmanager
import faulthandler
import json
import os
from pathlib import Path
import threading
import time

DIAGNOSTIC_GRACE = .25

_path = None
_log = None
_current = None
_until = None
_condition = threading.Condition()
_watchdog = None
_pending = None


def configure(output, prefix=''):
    global _path, _log, _until
    close()
    with _condition:
        _path = Path(output) / (prefix + 'deadlines.jsonl')
        _log = (Path(output) / (prefix + 'timeout-stacks.log')).open('a', encoding='utf-8')
        # The runner has its own cleanup timers. Only pytest's diagnostics share
        # its absolute execution ceiling; published operation budgets stay intact.
        _until = float(os.environ['WINLEX_TEST_DEADLINE']) if not prefix and 'WINLEX_TEST_DEADLINE' in os.environ else None


def _diagnose():
    global _pending
    while True:
        with _condition:
            if _pending is None:
                _condition.wait()
                continue
            remaining = _pending - time.monotonic()
            if remaining > 0:
                _condition.wait(remaining)
                continue
            # Native asynchronous frame traversal can race active Python frames.
            # Dump with the GIL when possible; retain the native timer for a
            # thread stuck in a native call that prevents this thread running.
            faulthandler.cancel_dump_traceback_later()
            faulthandler.dump_traceback(file=_log)
            _pending = None


def _publish(value):
    global _current, _pending, _watchdog
    with _condition:
        _current = value
        if _path is None:
            return
        faulthandler.cancel_dump_traceback_later()
        _pending = None
        if value is not None:
            _pending = min(value['deadline'], _until) if _until is not None else value['deadline']
            now = time.monotonic()
            if _pending <= now:
                # Restoring an overdue enclosing operation must not arm an
                # immediate asynchronous dump while Python is active again.
                faulthandler.dump_traceback(file=_log)
                _pending = None
            else:
                # Stay inside the diagnostic grace, leaving time to write a
                # native held-GIL stack before external termination.
                faulthandler.dump_traceback_later(_pending + .1 - now, file=_log)
                if _watchdog is None:
                    watchdog = threading.Thread(target=_diagnose, daemon=True)
                    watchdog.start()
                    _watchdog = watchdog
        try:
            # Append rather than replace a live file: Windows readers can temporarily
            # prevent renaming/deleting a file even when the payload is tiny.
            with _path.open('a', encoding='utf-8') as stream:
                stream.write(json.dumps(value) + '\n')
        finally:
            _condition.notify()


def phase(nodeid, name, seconds):
    _publish({'nodeid': nodeid, 'phase': name, 'operation': name,
              'deadline': time.monotonic() + seconds, 'budget': seconds})


@contextmanager
def operation(description, seconds):
    if _path is None:
        yield
        return
    previous = _current
    value = dict(previous or {}, operation=description,
                 deadline=time.monotonic() + seconds, budget=seconds)
    if previous and previous['deadline'] < value['deadline']:
        value = dict(previous)
    # A longer nested operation leaves the effective plan unchanged. Keep its
    # existing journal record and armed diagnostics instead of doing file I/O
    # and cancelling/rearming the same timers on every polling iteration.
    changed = value != previous
    if changed:
        _publish(value)
    try:
        yield
        if time.monotonic() > value['deadline']:
            raise AssertionError('Timeout: ' + value['operation'])
    finally:
        if previous and changed:
            _publish(previous)


def current_record(path):
    try:
        with Path(path).open('rb') as stream:
            stream.seek(0, 2)
            size = stream.tell()
            stream.seek(max(0, size - 65536))
            lines = stream.read().splitlines(keepends=True)
    except FileNotFoundError:
        return None
    complete = [line for line in lines if line.endswith(b'\n')]
    if not complete:
        return None
    return json.loads(complete[-1])


def expired(value, now=None):
    if value is None:
        return None
    # Give the in-process timer time to finish writing its stack before killing
    # a blocked COM call. This is diagnostic grace, not an operation extension.
    return value if (time.monotonic() if now is None else now) > value['deadline'] + DIAGNOSTIC_GRACE else None


def close():
    global _path, _log, _current, _until
    with _condition:
        try:
            if _path is not None:
                try:
                    _publish(None)
                finally:
                    _log.close()
        finally:
            _path = _log = _current = _until = None

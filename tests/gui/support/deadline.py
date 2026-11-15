"""Publish effective deadlines to events.jsonl and retain blocked stacks.

Phase/operation events are full effective deadline snapshots, including restores
after nested operations. Result/idle events clear that deadline in the same
journal; readers never have to reconcile independent files. The heartbeat is
liveness evidence only, not a second copy of progress (see events.py).
"""
from contextlib import contextmanager
import faulthandler
import os
from pathlib import Path
import threading
import time

from .policy import Policy
from . import events

DIAGNOSTIC_GRACE = .25
_HEARTBEAT_INTERVAL = .25

_path = None
_log = None
_heartbeat = None
_last_beat = 0.0
_stall = None
_current = None
_until = None
_condition = threading.Condition()
_watchdog = None
_pending = None


def configure(output, prefix=''):
    global _path, _log, _until, _heartbeat, _last_beat, _stall
    close()
    with _condition:
        _path = events.path(output, prefix)
        _log = (Path(output) / (prefix + 'timeout-stacks.log')).open('a', encoding='utf-8')
        _heartbeat = Path(output) / (prefix + 'heartbeat')
        _heartbeat.touch()
        _last_beat = time.monotonic()
        _stall = Policy.load().stall
        # The runner has its own cleanup timers. Only pytest's diagnostics share
        # its absolute execution ceiling; published operation budgets stay intact.
        _until = float(os.environ['WINLEX_TEST_DEADLINE']) if not prefix and 'WINLEX_TEST_DEADLINE' in os.environ else None


def progress(force=False):
    """Refresh the liveness heartbeat from a polling test thread."""
    global _last_beat
    if _heartbeat is None:
        return
    now = time.monotonic()
    if not force and now - _last_beat < _HEARTBEAT_INTERVAL:
        return
    _last_beat = now
    try:
        os.utime(_heartbeat, None)
    except OSError:
        pass


def heartbeat_age(path, now=None):
    try:
        return max(0.0, (time.time() if now is None else now) - Path(path).stat().st_mtime)
    except OSError:
        return None


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


def _publish(value, event='operation', **fields):
    global _current, _pending, _watchdog
    # Publishing a phase or operation is progress by the running test; only a
    # thread blocked inside one call leaves the heartbeat stale.
    # A new phase/operation rearms diagnostics from now. Its heartbeat must
    # have the same origin, rather than a throttled earlier poll that could
    # let the coordinator kill a held-GIL call before its stack is written.
    progress(force=True)
    with _condition:
        _current = value
        if _path is None:
            return
        faulthandler.cancel_dump_traceback_later()
        _pending = None
        if value is not None:
            now = time.monotonic()
            # Sample a long-running operation once. The supervisor decides
            # whether the stale heartbeat is a stall; this timer only retains
            # the stack needed to tell a blocked call from a slow one.
            soft = now + _stall if _stall else value['deadline']
            _pending = min(value['deadline'], soft, _until) if _until is not None else min(value['deadline'], soft)
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
            events.append(_path, event if value is not None or fields else 'idle',
                          **(value if value is not None else fields))
        finally:
            _condition.notify()


def phase(nodeid, name, seconds):
    _publish({'nodeid': nodeid, 'phase': name, 'operation': name,
              'deadline': time.monotonic() + seconds, 'budget': seconds}, event='phase')


def result(nodeid, name, outcome, duration, start, stop, message=''):
    """Commit a pytest phase report and disarm its completed deadline."""
    _publish(None, event='result', nodeid=nodeid, phase=name, outcome=outcome,
              duration=duration, start=start, stop=stop, message=message)


def collection_result(nodeid, outcome, message):
    """Retain collection failures/skips without ending the collection deadline."""
    if _path is not None:
        progress()
        events.append(_path, 'collection_result', nodeid=nodeid, outcome=outcome,
                      message=message)


@contextmanager
def operation(description, seconds=None):
    """Name the current operation. Only an explicit budget ends the process."""
    if _path is None:
        yield
        return
    previous = _current
    if seconds is None:
        if previous is None:
            value = {'operation': description, 'deadline': time.monotonic() + 3600,
                     'budget': 3600}
        else:
            value = dict(previous, operation=description)
        changed = value != previous
        if changed:
            _publish(value)
        try:
            yield
        finally:
            if changed:
                _publish(previous)
        return
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
        if changed:
            _publish(previous)


def expired(value, now=None):
    if value is None:
        return None
    # Give the in-process timer time to finish writing its stack before killing
    # a blocked COM call. This is diagnostic grace, not an operation extension.
    return value if (time.monotonic() if now is None else now) > value['deadline'] + DIAGNOSTIC_GRACE else None


def close():
    global _path, _log, _current, _until, _heartbeat, _stall
    with _condition:
        try:
            if _path is not None:
                try:
                    _publish(None)
                finally:
                    _log.close()
        finally:
            _path = _log = _current = _until = None
            _heartbeat = None
            _stall = None

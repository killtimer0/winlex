"""Publish operation deadlines for the supervising process; retain blocked stacks."""
from contextlib import contextmanager
import faulthandler
import json
import os
from pathlib import Path
import time

_path = None
_log = None
_current = None
_until = None


def configure(output, prefix=''):
    global _path, _log, _until
    _path = Path(output) / (prefix + 'deadlines.jsonl')
    _log = (Path(output) / (prefix + 'timeout-stacks.log')).open('a', encoding='utf-8')
    # The runner has its own cleanup timers. Only pytest's diagnostics share
    # its absolute execution ceiling; published operation budgets stay intact.
    _until = float(os.environ['WINLEX_TEST_DEADLINE']) if not prefix and 'WINLEX_TEST_DEADLINE' in os.environ else None


def _publish(value):
    global _current
    _current = value
    if _path is None:
        return
    faulthandler.cancel_dump_traceback_later()
    # Append rather than replace a live file: Windows readers can temporarily
    # prevent renaming/deleting a file even when the payload is tiny.
    with _path.open('a', encoding='utf-8') as stream:
        stream.write(json.dumps(value) + '\n')
    if value is not None:
        until = min(value['deadline'], _until) if _until is not None else value['deadline']
        faulthandler.dump_traceback_later(max(.001, until - time.monotonic()), file=_log)


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
    _publish(value)
    try:
        yield
        if time.monotonic() > value['deadline']:
            raise AssertionError('Timeout: ' + value['operation'])
    finally:
        if previous:
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


def expired(path, now=None):
    value = current_record(path)
    if value is None:
        return None
    # Give the in-process timer time to finish writing its stack before killing
    # a blocked COM call. This is diagnostic grace, not an operation extension.
    return value if (time.monotonic() if now is None else now) > value['deadline'] + .25 else None


def close():
    global _path, _log, _current, _until
    if _path is not None:
        _publish(None)
        _log.close()
    _path = _log = _current = _until = None

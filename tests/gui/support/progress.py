"""Derive live progress and case summaries from the authoritative events.jsonl.

Phase records establish elapsed time; operation records change only the current
operation; matching results finish phases. No status cache or mirrored journal
is written. Across resumed attempts, the latest completed call wins per nodeid.
"""
import time

from . import events, jsonl


def active(path):
    started = completed = operation = None
    for record in jsonl.records(events.path(events.attempt_directories(path)[-1])):
        kind = record.get('event')
        if kind == 'phase':
            started, completed, operation = record, None, record
        elif kind == 'operation':
            operation = record
        elif kind == 'idle':
            operation = None
        elif kind == 'result' and started and all(
                record.get(key) == started[key] for key in ('nodeid', 'phase')):
            completed, operation = record, None
    if started is None:
        return None
    running = completed is None
    now = time.monotonic()
    result = {'nodeid': started['nodeid'], 'phase': started['phase'], 'active': running,
              'elapsed': max(0, now - started['monotonic']) if running else completed['duration']}
    if running and operation and all(operation.get(key) == started[key] for key in ('nodeid', 'phase')):
        result.update(operation=operation['operation'], budget=operation['budget'],
                      remaining=max(0, operation['deadline'] - now))
    return result


def _calls(path):
    """Latest completed call per case, across resumed attempts."""
    cases = {}
    for directory in events.attempt_directories(path):
        for row in events.phase_reports(directory):
            if row['phase'] != 'call':
                continue
            cases[row['nodeid']] = row
    return sorted(cases.values(), key=lambda row: row['duration'], reverse=True)


def summary(path, slow_after):
    calls = _calls(path)
    return {
        'cases': len(calls),
        'seconds': sum(row['duration'] for row in calls),
        'slow': sum(1 for row in calls if row['duration'] >= slow_after),
        'slowest': calls[:10],
    }

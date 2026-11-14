"""Read attempt progress once for local output and remote status."""
from pathlib import Path
import time

from . import jsonl


def _attempt(path):
    """Directory of the latest attempt, following `attempts.jsonl`."""
    path = Path(path)
    attempt = jsonl.last(path / 'attempts.jsonl')
    return path / attempt['directory'] if attempt else path


def active(path):
    path = _attempt(path)
    started = jsonl.last(path / 'progress.jsonl')
    if started is None:
        return None
    completed = jsonl.last(path / 'timings.jsonl')
    running = completed is None or any(completed[key] != started[key] for key in ('nodeid', 'phase'))
    result = {'nodeid': started['nodeid'], 'phase': started['phase'], 'active': running,
              'elapsed': max(0, time.monotonic() - started['monotonic']) if running else completed['duration']}
    operation = jsonl.last(path / 'deadlines.jsonl')
    if running and operation and all(operation.get(key) == started[key] for key in ('nodeid', 'phase')):
        result['operation'] = operation['operation']
        result['budget'] = operation['budget']
        result['remaining'] = max(0, operation['deadline'] - time.monotonic())
    return result


def _calls(path):
    """Latest completed call per case, across resumed attempts."""
    cases = {}
    for timing in Path(path).rglob('timings.jsonl'):
        for row in jsonl.records(timing):
            if row.get('phase') != 'call':
                continue
            previous = cases.get(row['nodeid'])
            if previous is None or row.get('start', 0) >= previous.get('start', 0):
                cases[row['nodeid']] = row
    return sorted(cases.values(), key=lambda row: row.get('duration', 0), reverse=True)


def summary(path, slow_case):
    calls = _calls(path)
    return {
        'cases': len(calls),
        'seconds': sum(row.get('duration', 0) for row in calls),
        'slow': sum(1 for row in calls if row.get('duration', 0) >= slow_case),
        'slowest': calls[:10],
    }

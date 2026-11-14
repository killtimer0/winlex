"""Read attempt progress once for local output and remote status."""
import json
from pathlib import Path
import time


def _last_record(path):
    try:
        with path.open('rb') as stream:
            stream.seek(0, 2)
            stream.seek(max(0, stream.tell() - 65536))
            lines = stream.read().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if isinstance(record, dict):
            return record
    return None


def _attempt_path(path):
    path = Path(path)
    if (path / 'actual' / 'progress.jsonl').exists():
        path = path / 'actual'
    attempt = _last_record(path / 'attempts.jsonl')
    return path / attempt['directory'] if attempt else path


def active(path):
    path = _attempt_path(path)
    started = _last_record(path / 'progress.jsonl')
    if started is None:
        return None
    completed = _last_record(path / 'timings.jsonl')
    running = completed is None or any(completed[key] != started[key] for key in ('nodeid', 'phase'))
    result = {'nodeid': started['nodeid'], 'phase': started['phase'], 'active': running,
              'elapsed': max(0, time.monotonic() - started['monotonic']) if running else completed['duration']}
    operation = _last_record(path / 'deadlines.jsonl')
    if running and operation and all(operation.get(key) == started[key] for key in ('nodeid', 'phase')):
        result['operation'] = operation['operation']
        result['budget'] = operation['budget']
        result['remaining'] = max(0, operation['deadline'] - time.monotonic())
    return result


def slowest(path, limit=10):
    cases = {}
    for timing in Path(path).rglob('timings.jsonl'):
        try:
            lines = timing.read_text(encoding='utf-8').splitlines()
        except OSError:
            continue
        for line in lines:
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if row.get('phase') != 'call':
                continue
            cases.setdefault(row['nodeid'], row)
    return sorted(cases.values(), key=lambda row: row.get('duration', 0), reverse=True)[:limit]


def summary(path, slow_case):
    cases = slowest(path, limit=10 ** 9)
    return {
        'cases': len(cases),
        'seconds': sum(row.get('duration', 0) for row in cases),
        'slow': sum(1 for row in cases if row.get('duration', 0) >= slow_case),
        'slowest': slowest(path),
    }

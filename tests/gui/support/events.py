"""The worker's authoritative, append-only event journal.

Each newline commits one flat object with ``event``, wall-clock ``time`` and
``monotonic`` timestamps. ``phase`` starts a phase; ``operation`` changes or
restores its effective operation/deadline. Both carry the complete effective
deadline, so a supervisor needs only the last committed record. ``result``
ends a phase and carries pytest's outcome/timing/evidence; ``idle`` clears an
operation outside a phase or closes the journal. Neither has an active deadline.
``collection_result`` retains import errors/skips without changing the active
collection deadline.
Phase starts and results remain in this same history for progress and summaries;
there are no separately synchronized progress, deadline or timing files.
"""
from pathlib import Path
import time

from . import jsonl


def path(output, prefix=''):
    return Path(output) / (prefix + 'events.jsonl')


def append(journal, event, **fields):
    jsonl.append(journal, dict(fields, event=event, time=time.time(),
                               monotonic=time.monotonic()))


def active_event(directory, prefix=''):
    """Original deadline fields, or None after a result/idle or before startup.

    ``directory`` names one attempt, not a run containing multiple attempts.
    Journal metadata is deliberately excluded from the returned operation.
    """
    record = jsonl.committed(path(directory, prefix))
    if record and record.get('event') == 'collection_result':
        record = next((row for row in reversed(jsonl.records(path(directory, prefix)))
                       if row.get('event') != 'collection_result'), None)
    if not record or record.get('event') not in ('phase', 'operation'):
        return None
    return {key: record[key] for key in ('nodeid', 'phase', 'operation', 'deadline', 'budget')
            if key in record}


def phase_reports(directory):
    """Completed phases of one attempt, in journal order, with pytest fields only."""
    keys = ('nodeid', 'phase', 'outcome', 'duration', 'start', 'stop', 'message')
    return [{key: record[key] for key in keys}
            for record in jsonl.records(path(directory)) if record.get('event') == 'result']


def attempt_directories(directory):
    """Attempt Paths in attempts.jsonl order, or [directory] for a single attempt.

    The manifest is authoritative: unrelated child directories are not attempts.
    Repeated directory entries are read only once, at their latest position.
    """
    directory = Path(directory)
    attempts = []
    for record in jsonl.records(directory / 'attempts.jsonl'):
        attempt = directory / record['directory']
        if attempt in attempts:
            attempts.remove(attempt)
        attempts.append(attempt)
    return attempts or [directory]

"""One supervision loop for local commands and remote workers."""
import json
from pathlib import Path
import time

from . import deadline, progress
from .policy import Policy


def monitor_pytest(process, attempt, policy, budget_until, cancel, on_tick=None, poll=None):
    """Watch one pytest worker; never extend the caller's absolute budget."""
    poll = poll or process.poll
    attempt = Path(attempt)
    deadlines = attempt / 'deadlines.jsonl'
    heartbeat = attempt / 'heartbeat'
    while True:
        if on_tick:
            on_tick()
        if Path(cancel).exists():
            return {'cancelled': True}
        code = poll()
        if code is not None:
            return {'exit_code': code}
        now = time.monotonic()
        record = deadline.current_record(deadlines)
        if record:
            if record.get('phase') == 'call':
                age = deadline.heartbeat_age(heartbeat, time.time())
                if age is not None and age > policy.stall + deadline.DIAGNOSTIC_GRACE:
                    context = dict(record, deadline=now, budget=policy.stall, stalled=True,
                                   heartbeat_age=age)
                    return {'timed_out': True, 'stalled': True, 'timeout': context}
            overdue = deadline.expired(record, now)
            if overdue:
                return {'timed_out': True, 'timeout': overdue}
        if now >= budget_until:
            return {'timed_out': True, 'budget_exhausted': True}
        time.sleep(.05)


def supervise_runner(process, path, timeout, started, heartbeat=None, on_tick=None, policy=None):
    """Observe the engine; never replay input or take over its cleanup."""
    path = Path(path)
    policy = policy or Policy(run=timeout)
    plans = {}
    startup = True
    phase = 'startup'
    ceiling = started + policy.startup
    reason = None
    while process.poll() is None:
        if heartbeat:
            heartbeat()
        now = time.monotonic()
        if on_tick:
            on_tick(path, now)
        operation = deadline.current_record(path / 'runner-deadlines.jsonl')
        if operation:
            phase = operation.get('phase', phase)
            if phase != 'startup':
                startup = False
            ceiling = started + policy.startup + policy.run
            if startup:
                ceiling = started + policy.startup
            elif phase == 'cleanup-inventory':
                ceiling = operation['deadline']
            elif phase == 'cleanup':
                plan = _last_record(path / 'runner-cleanup.jsonl')
                if plan:
                    ceiling = plans.setdefault(plan['attempt'], plan['deadline'])
                else:
                    ceiling = min(ceiling, operation['deadline'])
        if not startup and phase == 'execution':
            age = deadline.heartbeat_age(path / 'runner-heartbeat', time.time())
            if age is not None and age > policy.stall + deadline.DIAGNOSTIC_GRACE:
                reason = 'Runner stalled: %.1fs without progress' % age
                break
        overdue = deadline.expired(operation, now)
        if overdue or now > ceiling + deadline.DIAGNOSTIC_GRACE:
            reason = 'Runner deadline exceeded'
            if overdue:
                reason += ': ' + overdue['operation']
            break
        time.sleep(.05)
    else:
        if (path / 'run.json').exists():
            try:
                report = json.loads((path / 'run.json').read_text(encoding='utf-8'))
                if isinstance(report, dict) and report.get('cleanup_complete'):
                    return True
                reason = 'Runner cleanup incomplete'
            except (ValueError, OSError) as error:
                reason = 'Invalid runner cleanup report: ' + str(error)
        else:
            reason = 'Runner exited without cleanup report'
    # Killing the runner only closes its job; it does not establish recovery of
    # input, profiles or independent watchdogs. Preserve run.json as evidence,
    # but never let an old success override this authoritative terminal status.
    killed = False
    try:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=10)
        killed = True
    except (OSError, Exception) as error:
        reason += ': ' + str(error)
    _atomic(path / 'supervisor.json', {
        'state': 'supervisor_failed', 'exit_code': process.returncode,
        'cleanup_complete': False, 'error': reason, 'runner_terminated': killed,
        'worker_available': False, 'active': progress.active(path),
    })
    return False


def _last_record(path):
    try:
        with path.open('rb') as stream:
            stream.seek(0, 2)
            stream.seek(max(0, stream.tell() - 8192))
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


def _atomic(path, value):
    temporary = path.with_suffix('.new')
    temporary.write_text(json.dumps(value, indent=2), encoding='utf-8')
    for attempt in range(10):
        try:
            temporary.replace(path)
            return
        except PermissionError:
            if attempt == 9:
                raise
            time.sleep(.02)

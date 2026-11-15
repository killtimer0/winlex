"""Whole-run authority shared by the CLI and queue worker.

The coordinator decides when to stop and whether another attempt is safe. A
disposable custodian owns resources, never continuation. Its report is evidence
until its process has exited; only this module publishes the final run.json.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from . import atomic, deadline, events, jsonl
from .policy import Policy
from .results import merge, succeeded


class UnconfirmedExit(RuntimeError):
    """Supervision stopped without the process-exit barrier needed for a result."""


def _read(path):
    return json.loads(path.read_text(encoding='utf-8'))


def reject(output, reason):
    """Publish a claimed request that could not enter the execution lifecycle."""
    result = {'state': 'unsafe', 'exit_code': 1, 'cleanup_complete': False,
              'worker_available': False, 'error': reason}
    atomic.write_json(Path(output) / 'run.json', result)
    return result


def _unsafe(process, folder, reason, stopped):
    """Kill the resource owner, but never mistake kill-on-close for recovery."""
    if process is not None:
        try:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
        except Exception as error:
            reason += '; cannot confirm custodian exit: ' + str(error)
            atomic.write_json(folder / 'supervision-error.json', {
                'error': reason, 'pid': process.pid, 'cleanup_complete': False,
            })
            raise UnconfirmedExit(reason) from error
    evidence = {}
    try:
        recorded = _read(folder / 'custody-result.json')
        if isinstance(recorded, dict):
            evidence = recorded
    except FileNotFoundError:
        pass
    except (ValueError, OSError) as error:
        reason += '; invalid custody evidence: ' + str(error)
    return dict(evidence, **{**stopped, 'exit_code': 1, 'cleanup_complete': False,
                           'state': 'unsafe', 'error': reason,
                           'worker_available': False})


def _observe(process, folder, root, policy, until, started, heartbeat, on_tick):
    stopped = {}
    phase = 'startup'
    ceiling = started + policy.startup
    planned = False
    inventory_ceiling = None
    while process.poll() is None:
        try:
            if heartbeat:
                heartbeat()
            now = time.monotonic()
            if on_tick:
                on_tick(folder, now)
            custody = events.active_event(folder, prefix='runner-')
            if custody:
                phase = custody.get('phase', phase)
                if phase == 'cleanup-inventory':
                    # Effective operation deadlines can be shorter than the
                    # phase. A finished stop-tree operation must not shorten
                    # the following inventory operation's lifetime.
                    if inventory_ceiling is None:
                        phase_events = [row for row in jsonl.records(events.path(folder, 'runner-'))
                                        if row.get('event') == 'phase' and row.get('phase') == phase]
                        inventory_ceiling = phase_events[0]['deadline']
                    ceiling = inventory_ceiling
                elif phase == 'cleanup':
                    # One immutable plan, published only after writers are dead.
                    plan = folder / 'cleanup-plan.json'
                    if plan.exists():
                        if not planned:
                            ceiling = _read(plan)['deadline']
                            planned = True
                    else:
                        ceiling = min(ceiling, custody['deadline'])
                elif phase == 'execution' and not stopped:
                    ceiling = until + policy.inventory
            if phase not in ('cleanup-inventory', 'cleanup') and not stopped:
                active = events.active_event(folder)
                if (root / 'cancel').exists():
                    stopped = {'cancelled': True, 'termination_reason': 'cancelled',
                               'active_context': active}
                elif now >= until:
                    stopped = {'timed_out': True, 'termination_reason': 'budget_exhausted',
                               'budget_deadline': until, 'active_context': active,
                               'pytest_stack': str(folder / 'timeout-stacks.log')}
                elif active:
                    age = deadline.heartbeat_age(folder / 'heartbeat')
                    if active.get('phase') == 'call' and age is not None and age > policy.stall + deadline.DIAGNOSTIC_GRACE:
                        context = dict(active, stalled=True, heartbeat_age=age,
                                       deadline=now, budget=policy.stall)
                        stopped = {'timed_out': True, 'stalled': True, 'timeout': context,
                                   'termination_reason': 'stalled'}
                    elif deadline.expired(active, now):
                        stopped = {'timed_out': True, 'timeout': active,
                                   'termination_reason': 'case_timeout'}
                if stopped:
                    atomic.write_json(folder / 'stop.json', stopped)
                    # Reuse the inventory bound for the handoff into recovery.
                    ceiling = now + policy.inventory
            if phase == 'execution':
                age = deadline.heartbeat_age(folder / 'runner-heartbeat')
                if age is not None and age > policy.stall + deadline.DIAGNOSTIC_GRACE:
                    return _unsafe(process, folder, 'Resource custodian stopped responding', stopped)
            if now > ceiling + deadline.DIAGNOSTIC_GRACE or (
                    phase in ('cleanup-inventory', 'cleanup') and deadline.expired(custody, now)):
                return _unsafe(process, folder, 'Resource custodian exceeded ' + phase + ' deadline', stopped)
            time.sleep(.05)
        except KeyboardInterrupt:
            (root / 'cancel').touch()
        except UnconfirmedExit:
            raise
        except Exception as error:
            return _unsafe(process, folder, 'Cannot supervise resource custody: ' + str(error), stopped)
    try:
        report = _read(folder / 'custody-result.json')
        if not isinstance(report, dict) or not report.get('cleanup_complete'):
            return _unsafe(process, folder, 'Resource recovery unverified', stopped)
        if process.returncode != 0:
            return _unsafe(process, folder, 'Custodian exit disagrees with recovery evidence', stopped)
        context = events.active_event(folder)
        if context:
            report.setdefault('active_context', context)
        return {**report, **stopped}
    except (ValueError, OSError) as error:
        return _unsafe(process, folder, 'Custodian exited without valid recovery evidence: ' + str(error), stopped)


def execute(output, tests, policy=None, desktop='isolated', heartbeat=None, on_tick=None, stream=None):
    """Run a selection once, replacing only safely recovered timed-out workers."""
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    if (output / 'run.json').exists() or (output / 'attempts.jsonl').exists():
        raise ValueError('Select a fresh run directory')
    policy = policy or Policy.load()
    until = time.monotonic() + policy.run
    attempts, excluded = [], set()
    unproductive = 0
    result = {'exit_code': 1, 'timed_out': False, 'cancelled': False,
              'cleanup_complete': True}
    if (output / 'cancel').exists():
        result.update(cancelled=True, state='cancelled_before_start')
    else:
        import win32api
        import win32process
        owner = {'pid': os.getpid(), 'created': win32process.GetProcessTimes(
            win32api.GetCurrentProcess())['CreationTime'].timestamp()}
        while True:
            if (output / 'cancel').exists():
                result = dict(result, cancelled=True, termination_reason='cancelled')
                break
            folder = output / 'attempts' / str(len(attempts) + 1)
            folder.mkdir(parents=True)
            atomic.write_json(folder / 'invocation.json', {
                'output': str(folder), 'root': str(output), 'tests': list(tests),
                'desktop': desktop, 'until': until, 'excluded': sorted(excluded),
                'policy': policy.dump(),
                'owner': owner,
            })
            with (output / 'attempts.jsonl').open('a', encoding='utf-8') as manifest:
                manifest.write(json.dumps({'directory': folder.relative_to(output).as_posix()}) + '\n')
            process = None
            try:
                if (output / 'cancel').exists():
                    result = dict(result, cancelled=True, termination_reason='cancelled')
                    break
                started = time.monotonic()
                process = subprocess.Popen([
                    sys.executable, '-X', 'utf8',
                    str(Path(__file__).with_name('custody.py')),
                    '--request', str(folder / 'invocation.json'),
                ], stdin=subprocess.DEVNULL, stdout=stream,
                   stderr=subprocess.STDOUT if stream is not None else None)
                result = _observe(process, folder, output, policy, until, started, heartbeat, on_tick)
            except UnconfirmedExit:
                raise
            except Exception as error:
                result = _unsafe(process, folder, 'Cannot start resource custody: ' + str(error), {})
            attempts.append((folder, result))
            if (output / 'cancel').exists():
                result['cancelled'] = True
            if not result.get('timeout') or not result['cleanup_complete'] or result.get('cancelled'):
                break
            selection_file = folder / 'selection.json'
            if not selection_file.exists():
                break
            selection = _read(selection_file)
            if result['timeout'].get('nodeid') not in selection:
                break
            reports = events.phase_reports(folder)
            completed = {row['nodeid'] for row in reports if row['phase'] == 'teardown'}
            excluded.update(completed)
            excluded.add(result['timeout']['nodeid'])
            if not any(case not in excluded for case in selection):
                break
            if until - time.monotonic() <= deadline.DIAGNOSTIC_GRACE:
                result['termination_reason'] = 'budget_exhausted'
                break
            unproductive = unproductive + 1 if result.get('stalled') and not completed else 0
            if unproductive >= policy.consecutive_stalls:
                result['termination_reason'] = 'repeated_stalls'
                break
    if attempts:
        result = dict(result)
        result['case_counts'] = merge(attempts, output)
        if result.get('exit_code') == 0 and any(result['case_counts'][key] for key in ('failures', 'errors')):
            result['exit_code'] = 1
        timeouts = [row['timeout'] for _, row in attempts if row.get('timeout')]
        if any(row.get('timed_out') for _, row in attempts):
            result.update(timed_out=True, exit_code=1)
        if timeouts:
            result.update(timeout=timeouts[0], case_timeouts=timeouts,
                          stalled=any(row.get('stalled') for _, row in attempts))
            result.setdefault('termination_reason', next(
                row['termination_reason'] for _, row in attempts if row.get('timeout')))
        result['attempts'] = len(attempts)
        selection_file = attempts[0][0] / 'selection.json'
        if selection_file.exists():
            completed = [row['nodeid'] for folder, _ in attempts
                         for row in events.phase_reports(folder) if row['phase'] == 'teardown']
            if completed:
                result['last_completed'] = completed[-1]
            interrupted = {row['nodeid'] for row in timeouts}
            interrupted.update(row['nodeid'] for folder, _ in attempts
                               for row in jsonl.records(events.path(folder))
                               if row.get('event') == 'phase'
                               and row.get('phase') in ('setup', 'call', 'teardown'))
            result['remaining'] = sum(case not in completed and case not in interrupted
                                      for case in _read(selection_file))
    result['worker_available'] = bool(result['cleanup_complete'])
    if not result['cleanup_complete']:
        result['state'] = 'unsafe'
    elif result.get('cancelled'):
        result['state'] = 'cancelled'
    elif result.get('timed_out'):
        result['state'] = 'timed_out'
    else:
        result['state'] = 'passed' if succeeded(result) else 'failed'
    if (result.get('exit_code') is None or result.get('timed_out')
            or result.get('cancelled') or not result['cleanup_complete']):
        result['exit_code'] = 1
    atomic.write_json(output / 'run.json', result)
    return result

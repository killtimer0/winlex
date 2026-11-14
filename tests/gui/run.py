"""Whole-run isolation, process-tree cleanup and a machine-readable final result."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import uuid
import winreg

import support
import win32api
import win32con
import win32event
import win32job
import win32process
import win32service
from support import atomic, engine
from support import deadline as operation_deadline
from support import jsonl, progress, supervisor
from support.policy import Policy
from support.results import merge, records


def arguments():
    parser = argparse.ArgumentParser()
    parser.add_argument("--desktop", choices=("isolated", "current"), default="isolated")
    parser.add_argument("--timeout", type=float)
    parser.add_argument("--case-timeout", type=float)
    parser.add_argument("--stall-after", type=float)
    parser.add_argument("--consecutive-timeouts", type=int)
    parser.add_argument("--engine", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--output")
    args, tests = parser.parse_known_args()
    interactive = "--real-input" in tests
    if interactive and args.desktop != "current":
        parser.error("--real-input requires --desktop current")
    output = Path(args.output or ("tmp/gui-tests/" + uuid.uuid4().hex)).resolve()
    if not args.engine and (output / "run.json").exists():
        parser.error("Output contains a completed run; select a fresh directory")
    output.mkdir(parents=True, exist_ok=True)
    base = Policy.load()
    args.policy = base.overrides(run=args.timeout, call=args.case_timeout,
                                 stall=args.stall_after,
                                 consecutive=args.consecutive_timeouts)
    if min(args.policy.run, args.policy.call, args.policy.stall,
           args.policy.consecutive) <= 0:
        parser.error("timeout budgets must be positive")
    args.output_path = output
    return args, tests


def main():
    args, tests = arguments()
    if args.engine:
        return engine_main(args, tests)
    return supervised(args, tests)


def supervised(args, tests):
    policy = args.policy
    output = args.output_path
    # The engine must watch the directory this supervisor already resolved.
    command = engine.command(output, tests, policy, args.desktop)
    started = time.monotonic()
    process = subprocess.Popen(command, env=os.environ.copy())
    state = {'at': 0.0, 'key': None}

    def report(path, now):
        current = progress.active(path)
        if not current or not current.get('active') or current['elapsed'] < policy.slow_case:
            return
        key = (current['nodeid'], current['phase'], current.get('operation'))
        if key == state['key'] and now - state['at'] < 5:
            return
        state.update(at=now, key=key)
        print('[slow] %6.1fs %s %s: %s' % (current['elapsed'], current['nodeid'],
                                           current['phase'], current.get('operation', '')),
              flush=True)

    try:
        safe = supervisor.supervise_runner(process, output, policy.run, started,
                                           on_tick=report, policy=policy)
    except KeyboardInterrupt:
        (output / 'cancel').touch()
        try:
            safe = supervisor.supervise_runner(process, output, policy.run, started,
                                               on_tick=report, policy=policy)
        except KeyboardInterrupt:
            process.kill()
            process.wait()
            raise
    if not safe:
        record = output / 'supervisor.json'
        if record.exists():
            print(json.loads(record.read_text(encoding='utf-8')).get('error', 'Runner failed'),
                  file=sys.stderr, flush=True)
        return 1
    report_file = output / 'run.json'
    if not report_file.exists():
        print('Runner exited without run.json', file=sys.stderr, flush=True)
        return 1
    result = json.loads(report_file.read_text(encoding='utf-8'))
    timing = progress.summary(output, policy.slow_case)
    if timing['cases']:
        print('GUI: %d cases, %.1fs, %d slow (>=%.0fs)' %
              (timing['cases'], timing['seconds'], timing['slow'], policy.slow_case), flush=True)
        for row in timing['slowest'][:10]:
            print('  %6.1fs %s' % (row.get('duration', 0), row['nodeid']), flush=True)
    return 0 if (result.get('exit_code') == 0 and result.get('cleanup_complete')
                 and not result.get('cancelled') and not result.get('timed_out')) else 1


def _cleanup(result, description, budget, action, *args):
    """Run one bounded cleanup step and retain its failure."""
    try:
        with operation_deadline.operation(description, budget):
            action(*args)
    except Exception as error:
        result['cleanup_errors'].append(str(error))


def remove_profile(entry):
    profile_root = entry.read_text(encoding='utf-8')
    assert profile_root == "Software\\Winlex\\Tests\\" + entry.stem
    try:
        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, profile_root)
    except FileNotFoundError:
        pass
    entry.unlink()


def recover_ledger(path):
    from support.input_state import recover
    recover(path)


def read_report(result, report):
    recorded = json.loads(report.read_text(encoding='utf-8'))
    if not recorded['cleanup_complete']:
        result['cleanup_errors'].extend(recorded['errors'])


def verify_receipt(receipt, budget):
    until = time.monotonic() + budget
    while not receipt.exists() and time.monotonic() < until:
        time.sleep(.05)
    if not receipt.exists() or not json.loads(receipt.read_text(encoding='utf-8')).get('ok'):
        raise RuntimeError('High-contrast recovery incomplete: ' + str(receipt))


def release_input(mutex, owned):
    if owned:
        win32event.ReleaseMutex(mutex)
    win32api.CloseHandle(mutex)


def engine_main(args, tests):
    policy = args.policy
    output = args.output_path
    args.until = time.monotonic() + policy.run
    os.environ['WINLEX_TEST_POLICY'] = policy.dump()
    attempts, excluded = [], set()
    folder = output
    operation_deadline.configure(output, prefix='runner-')
    operation_deadline.phase('runner', 'startup', policy.startup)
    while True:
        with (output / 'attempts.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps({'directory': str(folder.relative_to(output))}) + '\n')
        result = run_attempt(args, tests, folder, output, excluded, policy)
        attempts.append((folder, result))
        timeout = result.get('timeout')
        if (output / "cancel").exists():
            result['cancelled'] = True
        if not timeout or not result['cleanup_complete'] or result['cancelled']:
            break
        selection_path = folder / 'selection.json'
        if not selection_path.exists():
            break
        selection = json.loads(selection_path.read_text(encoding='utf-8'))
        if timeout['nodeid'] not in selection:
            break  # A collection failure has no safely resumable case boundary.
        excluded.update(row['nodeid'] for row in records(folder / 'timings.jsonl') if row['phase'] == 'teardown')
        excluded.add(timeout['nodeid'])
        remaining = args.until - time.monotonic()
        if not any(nodeid not in excluded for nodeid in selection) or remaining <= operation_deadline.DIAGNOSTIC_GRACE:
            break
        if sum(1 for _, attempt in attempts if attempt.get('timed_out')) >= policy.consecutive:
            result['termination_reason'] = 'repeated_timeouts'
            break
        folder = output / 'continued' / str(len(attempts))
        operation_deadline.phase('runner', 'startup', min(policy.startup, remaining))
    if any(result.get('timeout') for _, result in attempts):
        counts = merge(attempts, output)
        timeouts = [row['timeout'] for _, row in attempts if row.get('timeout')]
        source = next((row for _, row in attempts
                       if row.get('timeout') and row.get('termination_reason')), None)
        result = dict(result, exit_code=1, timed_out=True, case_counts=counts,
                      timeout=timeouts[0], case_timeouts=timeouts,
                      termination_reason=(result.get('termination_reason') or
                                          (source or {}).get('termination_reason') or
                                          'case_timeout'),
                      stalled=any(row.get('stalled') for _, row in attempts))
    elif result['cleanup_complete'] and (result['timed_out'] or result['cancelled']):
        # A stopped first worker cannot flush pytest's JUnit. Retain committed
        # phases and mark its interrupted case without authorizing continuation.
        result['case_counts'] = merge(attempts, output)
    if result.get('timed_out') or result.get('cancelled'):
        selection_path = output / 'selection.json'
        if selection_path.exists():
            selection = json.loads(selection_path.read_text(encoding='utf-8'))
            completed = [row['nodeid'] for folder, _ in attempts
                         for row in records(folder / 'timings.jsonl')
                         if row['phase'] == 'teardown']
            if completed:
                result['last_completed'] = completed[-1]
            timed_out = {row['timeout']['nodeid'] for _, row in attempts if row.get('timeout')}
            result['remaining'] = sum(1 for nodeid in selection
                                      if nodeid not in completed and nodeid not in timed_out)
    atomic.write_json(output / 'run.json', result)
    operation_deadline.close()
    return 0 if (result['exit_code'] == 0 and result['cleanup_complete']
                 and not result['cancelled'] and not result['timed_out']) else 1


def run_attempt(args, tests, output, root, excluded, policy):
    interactive = '--real-input' in tests
    output.mkdir(parents=True, exist_ok=True)
    job = win32job.CreateJobObject(None, "")
    # CreateProcess inherits standard handles below, but must never inherit the
    # kill-on-close job: a wedged descendant must not keep its own job alive.
    win32api.SetHandleInformation(job, win32con.HANDLE_FLAG_INHERIT, 0)
    limits = win32job.QueryInformationJobObject(job, win32job.JobObjectExtendedLimitInformation)
    limits["BasicLimitInformation"]["LimitFlags"] = win32job.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    win32job.SetInformationJobObject(job, win32job.JobObjectExtendedLimitInformation, limits)
    desktop = None
    process = thread = None
    assigned = False
    input_mutex = None
    input_owned = False
    result = {"exit_code": None, "timed_out": False, "cancelled": False, "cleanup_complete": False, "cleanup_errors": []}
    try:
        if interactive:
            input_mutex = win32event.CreateMutex(None, False, "Local\\Winlex.Tests.Input")
            if win32event.WaitForSingleObject(input_mutex, 0) not in (win32con.WAIT_OBJECT_0, win32con.WAIT_ABANDONED):
                win32api.CloseHandle(input_mutex)
                input_mutex = None
                raise RuntimeError("Another test owns input on this logon session")
            input_owned = True
            from support.input_state import InputState
            InputState(output / "profiles" / "run-input.json").__enter__()
        startup = win32process.STARTUPINFO()
        if args.desktop == "isolated":
            name = "Winlex.Test." + uuid.uuid4().hex
            desktop = win32service.CreateDesktop(name, 0, 0x1ff, None)
            startup.lpDesktop = "winsta0\\" + name
        # pytest is a console program. Pass this runner's redirected handles
        # explicitly: inheriting a console that a detached worker no longer has
        # leaves pytest without usable stdout and exits before writing evidence.
        startup.dwFlags |= win32process.STARTF_USESTDHANDLES
        startup.hStdInput = win32api.GetStdHandle(win32api.STD_INPUT_HANDLE)
        startup.hStdOutput = win32api.GetStdHandle(win32api.STD_OUTPUT_HANDLE)
        startup.hStdError = win32api.GetStdHandle(win32api.STD_ERROR_HANDLE)
        command = subprocess.list2cmdline([
            sys.executable, "-X", "utf8", "-m", "pytest",
            "--rootdir", str(Path(__file__).resolve().parents[2]), str(Path(__file__).parent),
            "--artifacts", str(output), "--junitxml", str(output / "tests.xml"),
            "-o", "cache_dir=" + str(output / "cache"), *tests,
        ])
        environment = {**os.environ, "WINLEX_TEST_LEDGER": str(output / "profiles"),
                       "WINLEX_TEST_POLICY": policy.dump()}
        environment['WINLEX_TEST_DEADLINE'] = str(args.until)
        exclusion_file = output / 'excluded.json'
        exclusion_file.write_text(json.dumps(sorted(excluded)), encoding='utf-8')
        environment['WINLEX_TEST_EXCLUDE'] = str(exclusion_file)
        environment.pop("WINLEX_TEST_REAL_INPUT", None)
        if interactive:
            environment["WINLEX_TEST_REAL_INPUT"] = "1"
        # pytest inherits this runner's redirected handles. Handles received from
        # the worker lost the inherit flag, so restore it for the child spawns.
        console_handles = {int(value) for value in
                           (startup.hStdInput, startup.hStdOutput, startup.hStdError) if value}
        saved = {handle: win32api.GetHandleInformation(handle) for handle in console_handles}
        for handle in console_handles:
            win32api.SetHandleInformation(handle, win32con.HANDLE_FLAG_INHERIT,
                                          win32con.HANDLE_FLAG_INHERIT)
        try:
            process, thread, _, _ = win32process.CreateProcess(
                None, command, None, None, True, win32process.CREATE_SUSPENDED,
                environment, str(Path(__file__).resolve().parents[2]), startup)
        finally:
            for handle, flags in saved.items():
                win32api.SetHandleInformation(handle, win32con.HANDLE_FLAG_INHERIT, flags)
        win32job.AssignProcessToJobObject(job, process)
        assigned = True
        win32process.ResumeThread(thread)
        operation_deadline.phase('runner', 'execution', max(.001, args.until - time.monotonic()))
        outcome = supervisor.monitor_pytest(
            process, output, policy, args.until, root / 'cancel',
            on_tick=operation_deadline.progress,
            poll=lambda: (None if win32event.WaitForSingleObject(process, 0) == win32con.WAIT_TIMEOUT
                          else win32process.GetExitCodeProcess(process)))
        if outcome.get('cancelled'):
            result['cancelled'] = True
        elif outcome.get('timed_out'):
            result['timed_out'] = True
            result['stalled'] = bool(outcome.get('stalled'))
            result['termination_reason'] = ('stalled' if outcome.get('stalled') else
                                            'budget_exhausted' if outcome.get('budget_exhausted') else
                                            'case_timeout')
            if outcome.get('timeout'):
                result['timeout'] = outcome['timeout']
            if outcome.get('budget_exhausted'):
                result['budget_deadline'] = args.until
                result['active_context'] = jsonl.committed(output / 'deadlines.jsonl')
                result['pytest_stack'] = str(output / 'timeout-stacks.log')
        else:
            result['exit_code'] = outcome.get('exit_code')
    except KeyboardInterrupt:
        result["cancelled"] = True
    except Exception as error:
        result["error"] = str(error)
    finally:
        # Stop writers before freezing the cleanup inventory. This preparation
        # has its own absolute ceiling; it cannot renew the execution budget.
        cleanup_started = time.monotonic()
        inventory_budget = policy.inventory + (policy.inventory if process and not assigned else 0) + policy.inventory
        operation_deadline.phase('runner', 'cleanup-inventory', inventory_budget)
        try:
            if result.get('termination_reason') == 'budget_exhausted':
                # Give the pytest timer its diagnostic grace before stopping
                # the job. This neither renews execution nor resumes a case.
                time.sleep(max(0, args.until + operation_deadline.DIAGNOSTIC_GRACE - time.monotonic()))
            if process and not assigned:
                win32process.TerminateProcess(process, 1)
                if win32event.WaitForSingleObject(process, int(policy.cleanup_wait * 1000)) != win32con.WAIT_OBJECT_0:
                    raise RuntimeError("Unassigned test process did not terminate")
            win32job.TerminateJobObject(job, 1)
            deadline = time.monotonic() + policy.cleanup_wait
            while win32job.QueryInformationJobObject(job, win32job.JobObjectBasicAccountingInformation)["ActiveProcesses"]:
                if time.monotonic() > deadline:
                    raise RuntimeError("Job still has active processes")
                time.sleep(.05)
        except Exception as error:
            result["cleanup_errors"].append(str(error))
        profiles = list((output / 'profiles').glob('*.profile'))
        reports = list(output.glob('*-cleanup.json'))
        receipts, registered = set(), {}
        for marker in output.iterdir():
            # Other probes also publish ready/result files. Only this owned
            # task-ID naming scheme denotes a high-contrast watchdog receipt.
            match = re.fullmatch(r'(.+)\.([0-9a-f]{32})\.(?:ready|result\.json|recovery\.json)', marker.name)
            if match:
                stem, task_id = match.groups()
                receipt = output / (stem + '.' + task_id + '.result.json')
                receipts.add(receipt)
                registered.setdefault(output / (stem + '.json'), set()).add(receipt)
        # Ready files are transient. A registered snapshot or a completed result
        # remains an obligation even after the watchdog removes its ready file.
        for snapshot in output.glob('*.json'):
            try:
                saved = json.loads(snapshot.read_text(encoding='utf-8'))
                if not isinstance(saved, dict) or not {'flags', 'scheme', 'context'} <= saved.keys():
                    if snapshot in registered:
                        raise RuntimeError('Invalid registered high-contrast snapshot: ' + str(snapshot))
                    continue
                if snapshot not in registered:
                    result['cleanup_errors'].append('High-contrast recovery incomplete: no owned watchdog receipt for ' + str(snapshot))
            except Exception as error:
                if snapshot in registered:
                    result['cleanup_errors'].append(str(error))
        ledgers = [path for path in (output / 'profiles' / name for name in ('input.json', 'run-input.json'))
                   if path.exists()]
        ledger_budgets = {}
        for path in ledgers:
            try:
                state = json.loads(path.read_text(encoding='utf-8'))
                ledger_budgets[path] = policy.ledger_budget(len(state['held']), state.get('ime'))
            except Exception as error:
                result['cleanup_errors'].append(str(error))
        handles = [handle for handle in (thread, process, job) if handle]
        # A watchdog may need its native recovery cap (10s) followed by task
        # cleanup (5s). A recovery marker alone is not a final success receipt.
        cleanup_budget = inventory_budget + policy.cleanup_receipt * len(receipts) + policy.cleanup_step * (len(profiles) + len(reports) + len(handles)
                               + bool(input_mutex) + bool(desktop) + 1) + sum(ledger_budgets.values())
        plan = {'attempt': str(output.relative_to(root)), 'deadline': cleanup_started + cleanup_budget,
                'budget': cleanup_budget, 'profiles': len(profiles), 'receipts': len(receipts),
                'ledgers': len(ledgers), 'reports': len(reports), 'handles': len(handles),
                'inventory_budget': inventory_budget}
        with (root / 'runner-cleanup.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps(plan) + '\n')
        operation_deadline.phase('runner', 'cleanup', max(.001, plan['deadline'] - time.monotonic()))
        for entry in profiles:
            _cleanup(result, 'remove test profile', policy.cleanup_step, remove_profile, entry)
        if input_owned and ledgers:
            for path in ledgers:
                if path in ledger_budgets:
                    _cleanup(result, 'recover owned input ledger', ledger_budgets[path],
                             recover_ledger, path)
        elif ledgers:
            result["cleanup_errors"].append("Input ledger exists without ownership of the active desktop")
        for report in reports:
            _cleanup(result, 'read cleanup report', policy.cleanup_step, read_report, result, report)
        for receipt in receipts:
            _cleanup(result, 'verify high-contrast recovery receipt', policy.cleanup_receipt,
                     verify_receipt, receipt, policy.cleanup_receipt)
        for handle in handles:
            _cleanup(result, 'close test process/job handle', policy.cleanup_step,
                     win32api.CloseHandle, handle)
        if input_mutex:
            _cleanup(result, 'release input ownership', policy.cleanup_step,
                     release_input, input_mutex, input_owned)
        if desktop:
            _cleanup(result, 'close test desktop', policy.cleanup_step, desktop.CloseDesktop)
        result["cleanup_complete"] = not result["cleanup_errors"]
        # The root run.json is the queue's whole-run completion signal. Keep
        # first-attempt evidence without publishing completion before resumes.
        report = output / ('attempt-result.json' if output == root else 'run.json')
        atomic.write_json(report, result)
    return result


if __name__ == "__main__":
    raise SystemExit(main())

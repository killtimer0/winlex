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
from support import deadline as operation_deadline
from support.results import merge, records


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--desktop", choices=("isolated", "current"), default="isolated")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--output")
    args, tests = parser.parse_known_args()
    interactive = "--real-input" in tests
    if interactive and args.desktop != "current":
        parser.error("--real-input requires --desktop current")
    output = Path(args.output or ("tmp/gui-tests/" + uuid.uuid4().hex)).resolve()
    if (output / "run.json").exists():
        parser.error("Output contains a completed run; select a fresh directory")
    started = time.monotonic()
    budget = args.timeout
    args.until = started + budget
    attempts, excluded = [], set()
    folder = output
    output.mkdir(parents=True, exist_ok=True)
    operation_deadline.configure(output, prefix='runner-')
    operation_deadline.phase('runner', 'startup', 15)
    while True:
        with (output / 'attempts.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps({'directory': str(folder.relative_to(output))}) + '\n')
        args.timeout = max(.1, budget - (time.monotonic() - started))
        result = run_attempt(args, tests, folder, output, excluded)
        attempts.append((folder, result))
        timeout = result.get('timeout')
        if (output / 'cancel').exists():
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
        if not any(nodeid not in excluded for nodeid in selection) or time.monotonic() - started >= budget:
            break
        folder = output / 'continued' / str(len(attempts))
        operation_deadline.phase('runner', 'startup', min(15, budget - (time.monotonic() - started)))
    if any(result.get('timeout') for _, result in attempts):
        counts = merge(attempts, output)
        result = dict(result, exit_code=1, timed_out=True, case_counts=counts,
                      timeout=next(row['timeout'] for _, row in attempts if row.get('timeout')),
                      case_timeouts=[row['timeout'] for _, row in attempts if row.get('timeout')])
    publication = output / 'run.new'
    publication.write_text(json.dumps(result, indent=2), encoding='utf-8')
    publication.replace(output / 'run.json')
    operation_deadline.close()
    return 0 if result['exit_code'] == 0 and result['cleanup_complete'] else 1


def run_attempt(args, tests, output, root, excluded):
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
        command = subprocess.list2cmdline([
            sys.executable, "-X", "utf8", "-m", "pytest",
            "--rootdir", str(Path(__file__).resolve().parents[2]), str(Path(__file__).parent),
            "--artifacts", str(output), "--junitxml", str(output / "tests.xml"),
            "-o", "cache_dir=" + str(output / "cache"), *tests,
        ])
        environment = {**os.environ, "WINLEX_TEST_LEDGER": str(output / "profiles")}
        environment['WINLEX_TEST_DEADLINE'] = str(args.until)
        exclusion_file = output / 'excluded.json'
        exclusion_file.write_text(json.dumps(sorted(excluded)), encoding='utf-8')
        environment['WINLEX_TEST_EXCLUDE'] = str(exclusion_file)
        environment.pop("WINLEX_TEST_REAL_INPUT", None)
        if interactive:
            environment["WINLEX_TEST_REAL_INPUT"] = "1"
        # Keep redirected standard handles available to pytest.
        process, thread, _, _ = win32process.CreateProcess(None, command, None, None, True,
            win32process.CREATE_SUSPENDED, environment, str(Path(__file__).resolve().parents[2]), startup)
        win32job.AssignProcessToJobObject(job, process)
        assigned = True
        win32process.ResumeThread(thread)
        operation_deadline.phase('runner', 'execution', max(.001, args.until - time.monotonic()))
        while win32event.WaitForSingleObject(process, 100) == win32con.WAIT_TIMEOUT:
            if (root / "cancel").exists():
                result["cancelled"] = True
                break
            if time.monotonic() >= args.until:
                result.update(timed_out=True, termination_reason='budget_exhausted',
                              budget_deadline=args.until,
                              active_context=operation_deadline.current_record(output / 'deadlines.jsonl'),
                              pytest_stack=str(output / 'timeout-stacks.log'))
                break
            overdue = operation_deadline.expired(output / 'deadlines.jsonl')
            if overdue:
                result['timed_out'] = True
                result['timeout'] = overdue
                break
        if not result["timed_out"] and not result["cancelled"]:
            result["exit_code"] = win32process.GetExitCodeProcess(process)
    except KeyboardInterrupt:
        result["cancelled"] = True
    except Exception as error:
        result["error"] = str(error)
    finally:
        # Stop writers before freezing the cleanup inventory. This preparation
        # has its own absolute ceiling; it cannot renew the execution budget.
        cleanup_started = time.monotonic()
        inventory_budget = 10 + (10 if process and not assigned else 0) + 10
        operation_deadline.phase('runner', 'cleanup-inventory', inventory_budget)
        try:
            if result.get('termination_reason') == 'budget_exhausted':
                # Give the pytest timer its diagnostic grace before stopping
                # the job. This neither renews execution nor resumes a case.
                time.sleep(max(0, args.until + .25 - time.monotonic()))
            if process and not assigned:
                win32process.TerminateProcess(process, 1)
                if win32event.WaitForSingleObject(process, 10000) != win32con.WAIT_OBJECT_0:
                    raise RuntimeError("Unassigned test process did not terminate")
            win32job.TerminateJobObject(job, 1)
            deadline = time.monotonic() + 10
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
                # UIA key releases + cursor delivery, IME send/wait/overrun,
                # foreground restoration and ledger IO, using existing caps.
                ledger_budgets[path] = 5 * (len(state['held']) + 1) + (21 if state.get('ime') else 0) + 5 + 10
            except Exception as error:
                result['cleanup_errors'].append(str(error))
        handles = [handle for handle in (thread, process, job) if handle]
        # A watchdog may need its native recovery cap (10s) followed by task
        # cleanup (5s). A recovery marker alone is not a final success receipt.
        cleanup_budget = inventory_budget + 15 * len(receipts) + 10 * (len(profiles) + len(reports) + len(handles)
                               + bool(input_mutex) + bool(desktop) + 1) + sum(ledger_budgets.values())
        plan = {'attempt': str(output.relative_to(root)), 'deadline': cleanup_started + cleanup_budget,
                'budget': cleanup_budget, 'profiles': len(profiles), 'receipts': len(receipts),
                'ledgers': len(ledgers), 'reports': len(reports), 'handles': len(handles),
                'inventory_budget': inventory_budget}
        with (root / 'runner-cleanup.jsonl').open('a', encoding='utf-8') as stream:
            stream.write(json.dumps(plan) + '\n')
        operation_deadline.phase('runner', 'cleanup', max(.001, plan['deadline'] - time.monotonic()))
        for entry in profiles:
            try:
                with operation_deadline.operation('remove test profile', 10):
                    profile_root = entry.read_text(encoding="utf-8")
                    assert profile_root == "Software\\Winlex\\Tests\\" + entry.stem
                    try:
                        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, profile_root)
                    except FileNotFoundError:
                        pass
                    entry.unlink()
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        if input_owned and ledgers:
            try:
                from support.input_state import recover
                for path in ledgers:
                    try:
                        if path in ledger_budgets:
                            with operation_deadline.operation('recover owned input ledger', ledger_budgets[path]):
                                recover(path)
                    except Exception as error:
                        result["cleanup_errors"].append(str(error))
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        elif ledgers:
            result["cleanup_errors"].append("Input ledger exists without ownership of the active desktop")
        for report in reports:
            try:
                with operation_deadline.operation('read cleanup report', 10):
                    recorded = json.loads(report.read_text(encoding="utf-8"))
                    if not recorded["cleanup_complete"]:
                        result["cleanup_errors"].extend(recorded["errors"])
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        for receipt in receipts:
            try:
                with operation_deadline.operation('verify high-contrast recovery receipt', 15):
                    until = time.monotonic() + 15
                    while not receipt.exists() and time.monotonic() < until:
                        time.sleep(.05)
                    if not receipt.exists() or not json.loads(receipt.read_text(encoding='utf-8')).get('ok'):
                        result['cleanup_errors'].append('High-contrast recovery incomplete: ' + str(receipt))
            except Exception as error:
                result['cleanup_errors'].append(str(error))
        for handle in handles:
            if handle:
                try:
                    with operation_deadline.operation('close test process/job handle', 10):
                        win32api.CloseHandle(handle)
                except Exception as error:
                    result["cleanup_errors"].append(str(error))
        if input_mutex:
            try:
                with operation_deadline.operation('release input ownership', 10):
                    if input_owned:
                        win32event.ReleaseMutex(input_mutex)
                    win32api.CloseHandle(input_mutex)
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        if desktop:
            try:
                with operation_deadline.operation('close test desktop', 10):
                    desktop.CloseDesktop()
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        result["cleanup_complete"] = not result["cleanup_errors"]
        # The root run.json is the queue's whole-run completion signal. Keep
        # first-attempt evidence without publishing completion before resumes.
        report = output / ('attempt-result.json' if output == root else 'run.json')
        report.write_text(json.dumps(result, indent=2), encoding="utf-8")
    return result


if __name__ == "__main__":
    raise SystemExit(main())

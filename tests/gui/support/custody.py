"""Own one pytest process tree and its cleanup, never whole-run decisions.

CLI: --request PATH, a coordinator-written JSON object with output (attempt
directory), root (run directory), tests (pytest arguments), desktop, until
(absolute monotonic execution deadline), excluded (node IDs), and policy
(Policy.dump() string). The coordinator atomically publishes output/stop.json
with its stop evidence; custody does not interpret pytest deadlines or retry.

output/cleanup-plan.json is published once after writers stop. Its absolute
deadline bounds cleanup for the coordinator. output/custody-result.json is
evidence, not a final run result: exit_code is pytest's exit status (or null),
stop fields retain the coordinator's decision, and cleanup_complete/errors
describe custody's guarantee. Process exit, not this file, is the barrier.
Custody exits zero iff cleanup is complete, irrespective of pytest's outcome.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid
import winreg

# Direct script execution must initialize support before importing pywin32.
if __package__ in (None, ''):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from support import atomic, deadline, jsonl
from support.policy import Policy
import win32api
import win32con
import win32event
import win32job
import win32process
import win32service


def _cleanup(result, description, budget, action, *args):
    try:
        with deadline.operation(description, budget):
            return action(*args)
    except BaseException as error:
        result['cleanup_errors'].append(description + ': ' + str(error))


def _remove_profile(entry):
    profile = entry.read_text(encoding='utf-8')
    # A ledger may remove only its own test key, never an arbitrary registry path.
    if profile != 'Software\\Winlex\\Tests\\' + entry.stem:
        raise ValueError('Invalid test profile ledger: ' + str(entry))
    try:
        winreg.DeleteKey(winreg.HKEY_CURRENT_USER, profile)
    except FileNotFoundError:
        pass
    entry.unlink()


def _recover_ledger(path):
    from support.input_state import recover
    recover(path)


def _read_report(result, report):
    recorded = json.loads(report.read_text(encoding='utf-8'))
    if not recorded['cleanup_complete']:
        result['cleanup_errors'].extend(recorded['errors'] or ['Incomplete cleanup: ' + str(report)])


def _verify_receipt(receipt, budget):
    until = time.monotonic() + budget
    while not receipt.exists() and time.monotonic() < until:
        deadline.progress()
        time.sleep(.05)
    if not receipt.exists():
        raise RuntimeError('High-contrast recovery incomplete: ' + str(receipt))
    recorded = json.loads(receipt.read_text(encoding='utf-8'))
    if not recorded.get('ok') or not recorded.get('task_cleanup_complete'):
        raise RuntimeError('High-contrast recovery incomplete: ' + str(receipt))


def _release_input(mutex, owned):
    try:
        if owned:
            win32event.ReleaseMutex(mutex)
    finally:
        win32api.CloseHandle(mutex)


def _phase(result, name, budget):
    # A phase replaces, rather than temporarily nests within, the previous
    # deadline. Wrapping this in operation() would restore the expired phase.
    try:
        deadline.phase('custody', name, budget)
    except BaseException as error:
        result['cleanup_errors'].append('Publish ' + name + ': ' + str(error))


def _publish_plan(path, plan):
    if path.exists():
        raise RuntimeError('Cleanup plan already exists: ' + str(path))
    atomic.write_json(path, plan)


def _stop_tree(resources, policy):
    process, job = resources['process'], resources['job']
    # Assignment can fail after CreateProcess; that suspended process is not
    # protected by the job's kill-on-close guarantee yet.
    try:
        if process and not resources['assigned']:
            win32process.TerminateProcess(process, 1)
            if win32event.WaitForSingleObject(process, int(policy.cleanup_wait * 1000)) != win32con.WAIT_OBJECT_0:
                raise RuntimeError('Unassigned test process did not terminate')
    finally:
        if job:
            win32job.TerminateJobObject(job, 1)
            until = time.monotonic() + policy.cleanup_wait
            while win32job.QueryInformationJobObject(job, win32job.JobObjectBasicAccountingInformation)['ActiveProcesses']:
                if time.monotonic() > until:
                    raise RuntimeError('Job still has active processes')
                deadline.progress()
                time.sleep(.05)
    return True


def _inventory(output, policy, result, inventory):
    inventory['profiles'].extend((output / 'profiles').glob('*.profile'))
    inventory['reports'].extend(output.glob('*-cleanup.json'))
    for obligation in jsonl.records(output / 'recoveries.jsonl'):
        try:
            if obligation['kind'] != 'high_contrast':
                raise ValueError('Unknown recovery obligation')
            for key in ('snapshot', 'receipt'):
                if Path(obligation[key]).name != obligation[key]:
                    raise ValueError('Recovery obligation must name an owned artifact')
            snapshot = output / obligation['snapshot']
            # Registration precedes scheduling. Missing readiness is still an
            # obligation, and an older successful task cannot discharge it.
            inventory['receipts'].add(output / obligation['receipt'])
            if snapshot.exists():
                saved = json.loads(snapshot.read_text(encoding='utf-8'))
            else:
                # Cooperative recovery removes its snapshot only after the
                # owned final receipt retains the complete expected state.
                saved = json.loads((output / obligation['receipt']).read_text(
                    encoding='utf-8'))['expected']
            if not isinstance(saved, dict) or not {'flags', 'scheme', 'context'} <= saved.keys():
                raise RuntimeError('Invalid high-contrast snapshot: ' + str(snapshot))
        except Exception as error:
            result['cleanup_errors'].append(str(error))
    for name in ('input.json', 'run-input.json'):
        path = output / 'profiles' / name
        if path.exists():
            inventory['ledgers'].append(path)
            try:
                state = json.loads(path.read_text(encoding='utf-8'))
                inventory['ledger_budgets'][path] = policy.ledger_budget(len(state['held']), state.get('ime'))
            except Exception as error:
                result['cleanup_errors'].append(str(error))


def _dispose(request, policy, resources, result):
    output = Path(request['output'])
    started = time.monotonic()
    inventory_budget = policy.inventory * (3 if resources['process'] and not resources['assigned'] else 2)
    inventory = {'profiles': [], 'reports': [], 'receipts': set(),
                 'ledgers': [], 'ledger_budgets': {}}
    _phase(result, 'cleanup-inventory', inventory_budget)
    if result.get('termination_reason') == 'budget_exhausted':
        # Only honor the parent's decision, allowing pytest's existing timer
        # its diagnostic grace; never decide that execution has expired here.
        time.sleep(max(0, request['until'] + deadline.DIAGNOSTIC_GRACE - time.monotonic()))
    stopped = _cleanup(result, 'stop test process tree',
                       policy.inventory * (2 if resources['process'] and not resources['assigned'] else 1),
                       _stop_tree, resources, policy)
    if not stopped:
        # A still-running writer can mutate ledgers or inject input. Do not
        # inventory, restore shared state or publish a frozen cleanup plan.
        raise RuntimeError('Cannot recover resources before process-tree quiescence')
    _cleanup(result, 'inventory test cleanup', policy.inventory,
             _inventory, output, policy, result, inventory)
    profiles, reports = inventory['profiles'], inventory['reports']
    receipts, ledgers = inventory['receipts'], inventory['ledgers']
    ledger_budgets = inventory['ledger_budgets']
    handles = [resources[name] for name in ('thread', 'process', 'job', 'owner') if resources[name]]
    mutex, desktop = resources['input_mutex'], resources['desktop']
    budget = (inventory_budget + policy.cleanup_receipt * len(receipts)
              + policy.cleanup_step * (len(profiles) + len(reports) + len(handles)
                                       + bool(mutex) + bool(desktop) + 1)
              + sum(ledger_budgets.values()))
    plan = {'attempt': str(output.relative_to(Path(request['root']))),
            'deadline': started + budget, 'budget': budget,
            'profiles': len(profiles), 'receipts': len(receipts),
            'ledgers': len(ledgers), 'reports': len(reports),
            'handles': len(handles), 'inventory_budget': inventory_budget}
    _cleanup(result, 'publish cleanup plan', policy.cleanup_step,
             _publish_plan, output / 'cleanup-plan.json', plan)
    _phase(result, 'cleanup', max(.001, plan['deadline'] - time.monotonic()))
    for entry in profiles:
        _cleanup(result, 'remove test profile', policy.cleanup_step, _remove_profile, entry)
    if resources['input_owned']:
        for path in ledgers:
            if path in ledger_budgets:
                _cleanup(result, 'recover owned input ledger', ledger_budgets[path], _recover_ledger, path)
    elif ledgers:
        result['cleanup_errors'].append('Input ledger exists without ownership of the active desktop')
    for report in reports:
        _cleanup(result, 'read cleanup report', policy.cleanup_step, _read_report, result, report)
    for receipt in sorted(receipts):
        _cleanup(result, 'verify high-contrast recovery receipt', policy.cleanup_receipt,
                 _verify_receipt, receipt, policy.cleanup_receipt)


def _release_resources(policy, resources, result):
    # Always attempted, even when inventory or plan publication itself failed.
    for name in ('thread', 'process', 'job', 'owner'):
        if resources[name]:
            _cleanup(result, 'close test process/job handle', policy.cleanup_step,
                     win32api.CloseHandle, resources[name])
    if resources['input_mutex']:
        _cleanup(result, 'release input ownership', policy.cleanup_step,
                 _release_input, resources['input_mutex'], resources['input_owned'])
    if resources['desktop']:
        _cleanup(result, 'close test desktop', policy.cleanup_step, resources['desktop'].CloseDesktop)


def _spawn(request, policy, resources):
    output = Path(request['output'])
    tests = request['tests']
    interactive = '--real-input' in tests
    if interactive and request['desktop'] != 'current':
        raise ValueError('--real-input requires the current desktop')
    job = resources['job'] = win32job.CreateJobObject(None, '')
    win32api.SetHandleInformation(job, win32con.HANDLE_FLAG_INHERIT, 0)
    limits = win32job.QueryInformationJobObject(job, win32job.JobObjectExtendedLimitInformation)
    limits['BasicLimitInformation']['LimitFlags'] = win32job.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    win32job.SetInformationJobObject(job, win32job.JobObjectExtendedLimitInformation, limits)
    if interactive:
        mutex = resources['input_mutex'] = win32event.CreateMutex(None, False, 'Local\\Winlex.Tests.Input')
        if win32event.WaitForSingleObject(mutex, 0) not in (win32con.WAIT_OBJECT_0, win32con.WAIT_ABANDONED):
            raise RuntimeError('Another test owns input on this logon session')
        resources['input_owned'] = True
        from support.input_state import InputState
        InputState(output / 'profiles' / 'run-input.json').__enter__()
    startup = win32process.STARTUPINFO()
    if request['desktop'] == 'isolated':
        name = 'Winlex.Test.' + uuid.uuid4().hex
        resources['desktop'] = win32service.CreateDesktop(name, 0, 0x1ff, None)
        startup.lpDesktop = 'winsta0\\' + name
    elif request['desktop'] != 'current':
        raise ValueError('Invalid desktop: ' + str(request['desktop']))
    startup.dwFlags |= win32process.STARTF_USESTDHANDLES
    startup.hStdInput = win32api.GetStdHandle(win32api.STD_INPUT_HANDLE)
    startup.hStdOutput = win32api.GetStdHandle(win32api.STD_OUTPUT_HANDLE)
    startup.hStdError = win32api.GetStdHandle(win32api.STD_ERROR_HANDLE)
    gui = Path(__file__).resolve().parents[1]
    repository = gui.parents[1]
    command = subprocess.list2cmdline([
        sys.executable, '-X', 'utf8', '-m', 'pytest', '--rootdir', str(repository), str(gui),
        '--artifacts', str(output), '--junitxml', str(output / 'tests.xml'),
        '-o', 'cache_dir=' + str(output / 'cache'), *tests,
    ])
    exclusion_file = output / 'excluded.json'
    atomic.write_json(exclusion_file, sorted(request['excluded']))
    environment = {**os.environ, 'WINLEX_TEST_LEDGER': str(output / 'profiles'),
                   'WINLEX_TEST_POLICY': policy.dump(),
                   'WINLEX_TEST_DEADLINE': str(request['until']),
                   'WINLEX_TEST_EXCLUDE': str(exclusion_file)}
    environment.pop('WINLEX_TEST_REAL_INPUT', None)
    if interactive:
        environment['WINLEX_TEST_REAL_INPUT'] = '1'
    # XP-compatible CreateProcess, with only the proven standard-handle
    # inheritance setup. The job itself must never be inherited by descendants.
    handles = {int(value) for value in
               (startup.hStdInput, startup.hStdOutput, startup.hStdError) if value}
    saved = {handle: win32api.GetHandleInformation(handle) for handle in handles}
    try:
        for handle in handles:
            win32api.SetHandleInformation(handle, win32con.HANDLE_FLAG_INHERIT, win32con.HANDLE_FLAG_INHERIT)
        process, thread, _, _ = win32process.CreateProcess(
            None, command, None, None, True, win32process.CREATE_SUSPENDED,
            environment, str(repository), startup)
        resources.update(process=process, thread=thread)
    finally:
        for handle, flags in saved.items():
            win32api.SetHandleInformation(handle, win32con.HANDLE_FLAG_INHERIT, flags)
    win32job.AssignProcessToJobObject(job, resources['process'])
    resources['assigned'] = True
    win32process.ResumeThread(resources['thread'])


def run(request):
    output = Path(request['output']).resolve()
    request = dict(request, output=str(output), root=str(Path(request['root']).resolve()))
    output.mkdir(parents=True, exist_ok=True)
    os.environ['WINLEX_TEST_POLICY'] = request['policy']
    policy = Policy.load()
    resources = dict(job=None, process=None, thread=None, desktop=None, owner=None,
                     assigned=False, input_mutex=None, input_owned=False)
    result = {'exit_code': None, 'timed_out': False, 'cancelled': False,
              'cleanup_complete': False, 'cleanup_errors': []}
    fault = False
    try:
        deadline.configure(output, prefix='runner-')
        deadline.phase('custody', 'startup', policy.startup)
        identity = request['owner']
        owner = resources['owner'] = win32api.OpenProcess(
            win32con.SYNCHRONIZE | win32con.PROCESS_QUERY_INFORMATION, False, identity['pid'])
        if (win32process.GetProcessTimes(owner)['CreationTime'].timestamp() != identity['created']
                or win32event.WaitForSingleObject(owner, 0) != win32con.WAIT_TIMEOUT):
            raise RuntimeError('Coordinator exited before resource acquisition')
        _spawn(request, policy, resources)
        deadline.phase('custody', 'execution', max(.001, request['until'] - time.monotonic()))
        stop = output / 'stop.json'
        while True:
            deadline.progress()
            if win32event.WaitForSingleObject(owner, 0) == win32con.WAIT_OBJECT_0:
                # Owner loss is a recovery trigger, never authority to replay.
                result.update(cancelled=True, termination_reason='owner_lost')
                break
            if stop.exists():
                decision = json.loads(stop.read_text(encoding='utf-8'))
                if not isinstance(decision, dict):
                    raise ValueError('stop.json must be an object')
                result.update({key: value for key, value in decision.items()
                               if key not in ('cleanup_complete', 'cleanup_errors', 'exit_code')})
                break
            status = win32event.WaitForSingleObject(resources['process'], 50)
            if status == win32con.WAIT_OBJECT_0:
                result['exit_code'] = win32process.GetExitCodeProcess(resources['process'])
                break
            if status != win32con.WAIT_TIMEOUT:
                raise RuntimeError('Test process wait failed: ' + str(status))
    except BaseException as error:
        fault = True
        result['custody_error'] = type(error).__name__ + ': ' + str(error)
        result.setdefault('error', result['custody_error'])
    finally:
        try:
            _dispose(request, policy, resources, result)
        except BaseException as error:
            result['cleanup_errors'].append('Unexpected cleanup failure: ' + str(error))
        _release_resources(policy, resources, result)
        try:
            deadline.close()
        except BaseException as error:
            result['cleanup_errors'].append('Close custody deadline: ' + str(error))
        result['cleanup_complete'] = not fault and not result['cleanup_errors']
        # Closing the deadline precedes evidence; only actual process exit can
        # prove that there is no remaining cleanup or evidence writer.
        atomic.write_json(output / 'custody-result.json', result)
    return 0 if result['cleanup_complete'] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--request', required=True, type=Path)
    args = parser.parse_args()
    return run(json.loads(args.request.read_text(encoding='utf-8')))


if __name__ == '__main__':
    raise SystemExit(main())

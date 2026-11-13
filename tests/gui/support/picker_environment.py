"""Reject the observed Shell/WSL dependency before opening a native picker."""
import json
import os
from pathlib import Path, PureWindowsPath
import subprocess
import sys
import time


class PickerEnvironmentConflict(RuntimeError):
    pass


def inspect_shortcuts():
    # Read link metadata only: Resolve/GetAttributes/accessing a target can
    # start a distribution or block in the redirector we are trying to avoid.
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    import support  # XP pywin32 DLL initialization.
    import pythoncom
    from win32com.shell import shell, shellcon

    folder = Path(shell.SHGetFolderPath(0, shellcon.CSIDL_NETHOOD, 0, 0))
    if folder.drive.startswith('\\\\'):
        raise RuntimeError('Network Shortcuts folder itself is on a network path: ' + str(folder))
    rows = []
    if folder.exists():
        for path in sorted(folder.glob('*/target.lnk')):
            link = pythoncom.CoCreateInstance(shell.CLSID_ShellLink, None,
                                             pythoncom.CLSCTX_INPROC_SERVER, shell.IID_IShellLink)
            link.QueryInterface(pythoncom.IID_IPersistFile).Load(str(path))
            target = link.GetPath(4)[0]  # SLGP_RAWPATH; absent from some pywin32 releases.
            parts = PureWindowsPath(target).drive.lstrip('\\').split('\\')
            if len(parts) == 2 and parts[0].casefold() in ('wsl.localhost', 'wsl$'):
                rows.append({'shortcut': str(path), 'target': target, 'distribution': parts[1]})
    return {'folder': str(folder), 'wsl_shortcuts': rows}


def require_picker_environment(artifact):
    from . import deadline
    import pytest

    started = time.monotonic()
    evidence = {'status': 'checking', 'children': [], 'targets_accessed': False,
                'distribution_start_requested': False, 'shortcuts_modified': False}

    def run(command, operation):
        with deadline.operation(operation, 3):
            child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     creationflags=subprocess.CREATE_NO_WINDOW)
            receipt = {'pid': child.pid, 'operation': operation, 'cleanup_complete': False}
            evidence['children'].append(receipt)
            try:
                try:
                    stdout, stderr = child.communicate(timeout=1.5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    stdout, stderr = child.communicate(timeout=1)
                    raise RuntimeError(operation + ' exceeded 1.5 seconds; owned child terminated')
                if child.returncode:
                    raise RuntimeError('%s failed (%s): %s' % (
                        operation, child.returncode, stderr.decode('utf-8', errors='replace')))
                return stdout
            finally:
                receipt.update(exit_code=child.poll(), cleanup_complete=child.poll() is not None)
                if not receipt['cleanup_complete']:
                    error = 'Native picker preflight child exit unconfirmed: %s' % child.pid
                    report = Path(artifact).with_name(Path(artifact).stem + '-cleanup.json')
                    report.write_text(json.dumps({'cleanup_complete': False, 'errors': [error]}), encoding='utf-8')
                    pytest.exit(error, returncode=1)

    try:
        inspected = run([sys.executable, '-X', 'utf8', str(Path(__file__).resolve()), '--inspect'],
                        'read native picker WSL shortcut metadata')
        evidence.update(json.loads(inspected.decode('utf-8')))
        if evidence['wsl_shortcuts']:
            # Listing running distributions does not start one or open its UNC.
            windows = Path(os.environ['SystemRoot'])
            executable = windows / 'Sysnative' / 'wsl.exe'
            if not executable.is_file():
                executable = windows / 'System32' / 'wsl.exe'
            if not executable.is_file():
                raise PickerEnvironmentConflict('WSL shortcuts exist but wsl.exe is unavailable')
            data = run([str(executable), '--list', '--running', '--quiet'],
                       'list running WSL distributions for native picker')
            running = data.decode('utf-16le').lstrip('\ufeff').splitlines()
            evidence['running_distributions'] = [line.strip() for line in running if line.strip()]
            names = {name.casefold() for name in evidence['running_distributions']}
            blocked = [row for row in evidence['wsl_shortcuts']
                       if row['distribution'].casefold() not in names]
            if blocked:
                raise PickerEnvironmentConflict('WSL network shortcut target is not running: ' +
                                                ', '.join(row['target'] for row in blocked))
        evidence['status'] = 'passed'
    except pytest.exit.Exception as error:
        evidence.update(status='cleanup_unconfirmed', error=str(error))
        raise
    except Exception as error:
        conflict = isinstance(error, PickerEnvironmentConflict)
        evidence.update(status='environment_failed' if conflict else 'preflight_failed', error=str(error))
        if conflict:
            raise PickerEnvironmentConflict('Picker environment conflict before product launch: ' + str(error)) from error
        raise RuntimeError('Picker environment preflight failed before product launch: ' + str(error)) from error
    finally:
        evidence['elapsed_seconds'] = time.monotonic() - started
        Path(artifact).write_text(json.dumps(evidence, indent=2), encoding='utf-8')


if __name__ == '__main__':
    assert sys.argv[1:] == ['--inspect']
    print(json.dumps(inspect_shortcuts()))

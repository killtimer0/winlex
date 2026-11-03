"""Whole-run isolation, process-tree cleanup and a machine-readable final result."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid
import winreg
import importlib.metadata

import support
from packaging.requirements import Requirement
import win32api
import win32con
import win32event
import win32job
import win32process
import win32service


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--desktop", choices=("isolated", "current"), default="isolated")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--output")
    args, tests = parser.parse_known_args()
    output = Path(args.output or ("tmp/gui-tests/" + uuid.uuid4().hex)).resolve()
    if (output / "run.json").exists():
        parser.error("Output contains a completed run; select a fresh directory")
    output.mkdir(parents=True, exist_ok=True)
    job = win32job.CreateJobObject(None, "")
    limits = win32job.QueryInformationJobObject(job, win32job.JobObjectExtendedLimitInformation)
    limits["BasicLimitInformation"]["LimitFlags"] = win32job.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    win32job.SetInformationJobObject(job, win32job.JobObjectExtendedLimitInformation, limits)
    desktop = None
    process = thread = None
    assigned = False
    input_mutex = None
    result = {"exit_code": None, "timed_out": False, "cancelled": False, "cleanup_complete": False, "cleanup_errors": []}
    try:
        for line in Path(__file__).with_name("requirements.txt").read_text(encoding="utf-8").splitlines():
            if line.strip() and not line.startswith("#"):
                requirement = Requirement(line)
                if requirement.marker and not requirement.marker.evaluate():
                    continue
                actual = importlib.metadata.version(requirement.name)
                if actual not in requirement.specifier:
                    raise RuntimeError("Dependency mismatch: %s, found %s" % (requirement, actual))
        if "--real-input" in tests:
            if args.desktop != "current":
                raise RuntimeError("Actual input requires --desktop current")
            input_mutex = win32event.CreateMutex(None, False, "Local\\Winlex.Tests.Input")
            if win32event.WaitForSingleObject(input_mutex, 0) != win32con.WAIT_OBJECT_0:
                win32api.CloseHandle(input_mutex)
                input_mutex = None
                raise RuntimeError("Another test owns input on this logon session")
        startup = win32process.STARTUPINFO()
        if args.desktop == "isolated":
            name = "Winlex.Test." + uuid.uuid4().hex
            desktop = win32service.CreateDesktop(name, 0, win32con.GENERIC_ALL, None)
            startup.lpDesktop = name
        command = subprocess.list2cmdline([
            sys.executable, "-X", "utf8", "-m", "pytest", str(Path(__file__).parent),
            "--artifacts", str(output), "--junitxml", str(output / "tests.xml"), *tests,
        ])
        environment = {**os.environ, "WINLEX_TEST_LEDGER": str(output / "profiles")}
        # Keep redirected standard handles available to pytest.
        process, thread, _, _ = win32process.CreateProcess(None, command, None, None, True,
            win32process.CREATE_SUSPENDED, environment, str(Path(__file__).resolve().parents[2]), startup)
        win32job.AssignProcessToJobObject(job, process)
        assigned = True
        win32process.ResumeThread(thread)
        started = time.monotonic()
        while win32event.WaitForSingleObject(process, 100) == win32con.WAIT_TIMEOUT:
            if (output / "cancel").exists():
                result["cancelled"] = True
                break
            if time.monotonic() - started > args.timeout:
                result["timed_out"] = True
                break
        if not result["timed_out"] and not result["cancelled"]:
            result["exit_code"] = win32process.GetExitCodeProcess(process)
    except KeyboardInterrupt:
        result["cancelled"] = True
    except Exception as error:
        result["error"] = str(error)
    finally:
        try:
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
            for entry in (output / "profiles").glob("*.profile"):
                root = entry.read_text(encoding="utf-8")
                assert root == "Software\\Winlex\\Tests\\" + entry.stem
                try:
                    winreg.DeleteKey(winreg.HKEY_CURRENT_USER, root)
                except FileNotFoundError:
                    pass
                entry.unlink()
            from support.input_state import recover
            recover(output / "profiles" / "input.json")
            for report in output.glob("*-cleanup.json"):
                recorded = json.loads(report.read_text(encoding="utf-8"))
                if not recorded["cleanup_complete"]:
                    result["cleanup_errors"].extend(recorded["errors"])
            result["cleanup_complete"] = not result["cleanup_errors"]
        except Exception as error:
            result["cleanup_errors"].append(str(error))
        for handle in (thread, process, job):
            if handle:
                try:
                    win32api.CloseHandle(handle)
                except Exception as error:
                    result["cleanup_errors"].append(str(error))
        if input_mutex:
            try:
                win32event.ReleaseMutex(input_mutex)
                win32api.CloseHandle(input_mutex)
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        if desktop:
            try:
                desktop.CloseDesktop()
            except Exception as error:
                result["cleanup_errors"].append(str(error))
        result["cleanup_complete"] = result["cleanup_complete"] and not result["cleanup_errors"]
        (output / "run.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    return 0 if result["exit_code"] == 0 and result["cleanup_complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

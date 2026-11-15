"""One-shot target runner (Windows, Python 3.8+). No transport or retries."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import time


def require_interactive_desktop():
    """Read desktop state without switching desktops or changing input state."""
    if os.name != "nt":
        raise RuntimeError("GUI tests require Windows")
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.GetProcessWindowStation.restype = wintypes.HANDLE
    user32.GetThreadDesktop.argtypes = [wintypes.DWORD]
    user32.GetThreadDesktop.restype = wintypes.HANDLE
    user32.OpenInputDesktop.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    user32.OpenInputDesktop.restype = wintypes.HANDLE
    user32.CloseDesktop.argtypes = [wintypes.HANDLE]
    user32.GetUserObjectInformationW.argtypes = [
        wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD,
        ctypes.POINTER(wintypes.DWORD)]

    def name(handle):
        buffer = ctypes.create_unicode_buffer(256)
        needed = wintypes.DWORD()
        if not handle or not user32.GetUserObjectInformationW(
                handle, 2, buffer, ctypes.sizeof(buffer), ctypes.byref(needed)):
            raise ctypes.WinError(ctypes.get_last_error())
        return buffer.value.casefold()

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    station = name(user32.GetProcessWindowStation())
    desktop = name(user32.GetThreadDesktop(kernel32.GetCurrentThreadId()))
    input_desktop = user32.OpenInputDesktop(0, False, 1)
    try:
        if (station, desktop, name(input_desktop)) != ("winsta0", "default", "default"):
            raise RuntimeError("An unlocked WinSta0/Default interactive desktop is required")
    finally:
        if input_desktop:
            user32.CloseDesktop(input_desktop)


def process_identity(pid):
    """Creation time prevents cancellation from killing a reused PID."""
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel32.OpenProcess(0x0400, False, pid)
    if not handle:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        times = [wintypes.FILETIME() for _ in range(4)]
        if not kernel32.GetProcessTimes(handle, *[ctypes.byref(value) for value in times]):
            raise ctypes.WinError(ctypes.get_last_error())
        return (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
    finally:
        kernel32.CloseHandle(handle)


def terminate_tree(pid):
    completed = subprocess.run(
        ["taskkill.exe", "/PID", str(pid), "/T", "/F"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=15)
    return {"taskkill_exit_code": completed.returncode,
            "detail": completed.stdout.decode("mbcs", errors="replace")}


def file_sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_deployment(manifest):
    """Verify the developer snapshot, without starting pytest or a desktop process."""
    output = Path(manifest["output"])
    output.mkdir(parents=True, exist_ok=True)
    files = []
    for expected in manifest["files"]:
        actual = file_sha256(expected["path"])
        files.append(dict(expected, actual_sha256=actual, verified=actual == expected["sha256"]))
    (output / "input-hashes.json").write_text(
        json.dumps({"files": files}, ensure_ascii=False, indent=2), encoding="utf-8")
    if not files or not all(item["verified"] for item in files):
        raise RuntimeError("Deployment SHA256 mismatch; see input-hashes.json")


def require_pytest_dependencies():
    for module in ("pytest", "pytest_timeout"):
        if importlib.util.find_spec(module) is None:
            raise RuntimeError("Target Python is missing " + module)


def private_config_path(value):
    if not isinstance(value, str) or not re.fullmatch(
            r"Software\\Winlex\\E2E\\(?:[0-9a-f]{32}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})",
            value, re.IGNORECASE):
        raise ValueError("Refusing a config key outside Software\\Winlex\\E2E\\<UUID>")
    return value


def remove_private_config(value):
    import winreg
    root = private_config_path(value)

    def remove(path):
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path, 0,
                                winreg.KEY_READ | winreg.KEY_WRITE) as key:
                while True:
                    try:
                        child = winreg.EnumKey(key, 0)
                    except OSError as error:
                        if error.winerror == 259:  # ERROR_NO_MORE_ITEMS
                            break
                        raise
                    remove(path + "\\" + child)
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, path)
        except FileNotFoundError:
            pass

    remove(root)


def application_exited(pid, created):
    """Inspect only; never terminate an application from an untrusted/stale PID."""
    if type(pid) is not int or pid <= 0:
        raise ValueError("Application PID is missing or invalid")
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel32.OpenProcess(0x100400, False, pid)
    if not handle:
        error = ctypes.get_last_error()
        if error == 87:  # ERROR_INVALID_PARAMETER: PID no longer exists
            return True
        raise ctypes.WinError(error)
    try:
        state = kernel32.WaitForSingleObject(handle, 0)
        if state == 0:
            return True
        if state != 258:  # WAIT_TIMEOUT
            raise ctypes.WinError(ctypes.get_last_error())
        if type(created) is not int or created <= 0:
            raise ValueError("Cannot identify a live application without its creation time")
        if process_identity(pid) != created:
            return True
        # Job-handle closure can still be completing after pytest/taskkill exits.
        state = kernel32.WaitForSingleObject(handle, 5000)
        if state not in (0, 258):
            raise ctypes.WinError(ctypes.get_last_error())
        return state == 0
    finally:
        kernel32.CloseHandle(handle)


def cleanup_applications(output):
    """Only inspect records below this run; registry cleanup requires proven GUI exit."""
    result = {"applications_exited": True, "configs_removed": True, "applications_checked": 0,
              "errors": []}
    output = Path(output)
    try:
        records = sorted(output.rglob("process.json"))
    except Exception as error:
        result.update(applications_exited=False, configs_removed=False, errors=[str(error)])
        return result
    for path in records:
        result["applications_checked"] += 1
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
            if record.get("pid") is None and record.get("cleanup_complete") is True:
                exited = True
            else:
                exited = application_exited(record.get("pid"), record.get("created"))
            if not exited:
                raise RuntimeError("Recorded GUI process is still alive")
        except Exception as error:
            result["applications_exited"] = False
            result["configs_removed"] = False
            result["errors"].append("{}: {}".format(path.relative_to(output), error))
            continue
        try:
            remove_private_config(record.get("config_root"))
        except Exception as error:
            result["configs_removed"] = False
            result["errors"].append("{}: {}".format(path.relative_to(output), error))
    return result


def write_result(output, result):
    temporary = Path(output) / "result.json.tmp"
    temporary.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    temporary.replace(Path(output) / "result.json")


def probe_job_containment(output):
    """Exercise the fixture's Job policy with one hidden, non-GUI Python child."""
    report = {"breakaway": sys.getwindowsversion()[:2] < (6, 2), "assigned": False,
              "kill_on_close_confirmed": False}
    child = job = process = None
    try:
        import win32api
        import win32job
        job = win32job.CreateJobObject(None, 'Winlex.GuiProbe.' + str(os.getpid()))
        limits = win32job.QueryInformationJobObject(job, win32job.JobObjectExtendedLimitInformation)
        limits["BasicLimitInformation"]["LimitFlags"] = win32job.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        win32job.SetInformationJobObject(job, win32job.JobObjectExtendedLimitInformation, limits)
        flags = subprocess.CREATE_NO_WINDOW
        if report["breakaway"]:
            flags |= subprocess.CREATE_BREAKAWAY_FROM_JOB
        child = subprocess.Popen([str(Path(sys.executable).with_name("python.exe")), "-c",
                                  "import time;time.sleep(60)"], creationflags=flags,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL)
        process = win32api.OpenProcess(0x100501, False, child.pid)
        win32job.AssignProcessToJobObject(job, process)
        report["assigned"] = True
        if child.poll() is not None:
            raise RuntimeError("Probe child exited before Job closure")
        job.Close()
        job = None
        child.wait(timeout=5)
        report["kill_on_close_confirmed"] = True
    except Exception as error:
        report["error"] = "{}: {}".format(type(error).__name__, error)
        raise RuntimeError("Job containment unavailable; refusing GUI launch: " + report["error"])
    finally:
        try:
            if job is not None:
                job.Close()
            if child is not None and child.poll() is None:
                child.terminate()
                child.wait(timeout=10)
        finally:
            if process is not None:
                process.Close()
            report["probe_process_exited"] = child is None or child.poll() is not None
            (Path(output) / "job-capability.json").write_text(json.dumps(report, indent=2), encoding="utf-8")


def cancel_run(directory):
    """Called over SSH, never starts a GUI or replays a run."""
    directory = Path(directory)
    manifest = json.loads((directory / "manifest.json").read_text(encoding="utf-8"))
    output = Path(manifest["output"])
    result_path = output / "result.json"
    if result_path.is_file():
        return json.loads(result_path.read_text(encoding="utf-8"))
    (directory / "cancel.requested").touch()
    deadline = time.monotonic() + 5
    while True:
        if result_path.is_file():
            return json.loads(result_path.read_text(encoding="utf-8"))
        try:
            identity = json.loads((directory / "execute.pid").read_text(encoding="utf-8"))
            break
        except FileNotFoundError:
            if time.monotonic() >= deadline:
                raise RuntimeError("Neither execute PID nor final result exists; launch/cleanup is unknown; "
                                   "cancel.requested retained to block late execution")
            time.sleep(0.1)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = kernel32.OpenProcess(0x100400, False, identity["pid"])
    if not handle and ctypes.get_last_error() != 87:
        if result_path.is_file():
            return json.loads(result_path.read_text(encoding="utf-8"))
        raise ctypes.WinError(ctypes.get_last_error())
    termination = {}
    try:
        if (handle and kernel32.WaitForSingleObject(handle, 0) != 0 and
                process_identity(identity["pid"]) == identity["created"]):
            termination = terminate_tree(identity["pid"])
            if termination["taskkill_exit_code"] != 0 or kernel32.WaitForSingleObject(handle, 10000) != 0:
                raise RuntimeError("Execute tree termination unconfirmed: " + termination["detail"])
    finally:
        if handle:
            kernel32.CloseHandle(handle)
    # The execute process might have naturally published its result while we checked.
    if result_path.is_file():
        return json.loads(result_path.read_text(encoding="utf-8"))
    if not termination:
        raise RuntimeError("Execute ended without a final result; its remaining process tree is unknown")
    cleanup = cleanup_applications(output)
    cleanup.update(termination)
    result = {"exit_code": 130, "timed_out": False, "cancelled": True, "cleanup": cleanup}
    write_result(output, result)
    if not cleanup["applications_exited"] or not cleanup["configs_removed"]:
        raise RuntimeError("Cancellation cleanup unconfirmed; see " + str(result_path))
    return result


def pytest_command(manifest, directory):
    # execute itself is launched with pythonw; pytest needs real stdout/stderr.
    python = str(Path(sys.executable).with_name("python.exe"))
    arguments = manifest["pytest_args"]
    # An explicit path is needed for initial conftest option registration, but
    # must not broaden a caller's explicit node selection to the whole suite.
    return [python, "-m", "pytest"] + ([] if manifest.get('explicit_nodes') else [str(directory)]) + [
            "-c", str(Path(directory) / "pytest.ini"),
            "--strict-config",
            "--exe", manifest["exe"], "--winmd", manifest["winmd"],
            "--output", manifest["output"], "--desktop-authorized",
            "--junitxml", str(Path(manifest["output"]) / "junit.xml")] + arguments


def execute(manifest_path):
    started = time.monotonic()
    manifest_path = Path(manifest_path).resolve()
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    output = Path(manifest["output"])
    output.mkdir(parents=True, exist_ok=True)
    (output / "pytest.log").touch()
    result = {"exit_code": 1, "timed_out": False, "cleanup": {}}
    child = None
    try:
        if manifest.get("desktop_authorized") is not True:
            raise RuntimeError("Desktop execution was not explicitly authorized")
        if (not isinstance(manifest["timeout"], (int, float)) or
                not math.isfinite(manifest["timeout"]) or manifest["timeout"] <= 0):
            raise ValueError("timeout must be positive")
        pid_path = manifest_path.parent / "execute.pid"
        pid_temporary = pid_path.with_suffix(".pid.tmp")
        pid_temporary.write_text(json.dumps({
            "pid": os.getpid(), "created": process_identity(os.getpid())}), encoding="utf-8")
        pid_temporary.replace(pid_path)
        if (manifest_path.parent / "cancel.requested").is_file():
            result.update(exit_code=130, cancelled=True)
            return 130
        require_pytest_dependencies()
        verify_deployment(manifest)
        from importlib.metadata import version
        (output / "execution-environment.json").write_text(json.dumps({
            "python": sys.version, "executable": sys.executable, "platform": platform.platform(),
            "pytest": version("pytest"), "pytest_timeout": version("pytest-timeout")}, indent=2),
            encoding="utf-8")
        require_interactive_desktop()
        probe_job_containment(output)
        if manifest.get("probe_job"):
            result.update(exit_code=0, probe_only=True)
        else:
            with (output / "pytest.log").open("wb") as log:
                if (manifest_path.parent / "cancel.requested").is_file():
                    result.update(exit_code=130, cancelled=True)
                    return 130
                child = subprocess.Popen(pytest_command(manifest, manifest_path.parent),
                                         cwd=str(manifest_path.parent), stdout=log,
                                         stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                         env=dict(os.environ, PYTHONIOENCODING='utf-8'),
                                         creationflags=subprocess.CREATE_NO_WINDOW)
                try:
                    result["exit_code"] = child.wait(timeout=max(0.01, manifest["timeout"] -
                                                                 (time.monotonic() - started)))
                except subprocess.TimeoutExpired:
                    result["timed_out"] = True
                    result["exit_code"] = 124
                    result["cleanup"] = terminate_tree(child.pid)
                    child.wait(timeout=10)
                result["cleanup"]["pytest_exited"] = child.poll() is not None
    except BaseException as error:
        result["exit_code"] = max(2, result["exit_code"])
        result["error"] = "{}: {}".format(type(error).__name__, error)
        with (output / "pytest.log").open("a", encoding="utf-8") as log:
            log.write("\nRunner error: " + result["error"] + "\n")
        if child is not None and child.poll() is None:
            try:
                result["cleanup"] = terminate_tree(child.pid)
                child.wait(timeout=10)
            except Exception as cleanup_error:
                result["cleanup"]["error"] = str(cleanup_error)
    finally:
        result["cleanup"]["pytest_exited"] = child is None or child.poll() is not None
        if result["cleanup"]["pytest_exited"]:
            result["cleanup"].update(cleanup_applications(output))
        else:
            result["cleanup"].update(applications_exited=False, configs_removed=False,
                                      errors=["pytest is still running; application cleanup unconfirmed"])
        if not result["cleanup"]["applications_exited"] or not result["cleanup"]["configs_removed"]:
            result["exit_code"] = max(2, result["exit_code"])
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        write_result(output, result)
    return result["exit_code"]


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    args = parser.parse_args(argv)
    return execute(args.manifest)


if __name__ == "__main__":
    sys.exit(main())

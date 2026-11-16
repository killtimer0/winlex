"""Run GUI pytest once on an explicitly authorized local or SSH desktop."""
import argparse
import base64
import hashlib
import json
import math
import ntpath
import os
from pathlib import Path, PureWindowsPath
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
import zipfile


HERE = Path(__file__).resolve().parent
RESERVED = {"--exe", "--winmd", "--output", "--desktop-authorized", "--junitxml", "--junit-xml",
            "-c", "--config-file", "--rootdir", "--confcutdir", "--pyargs"}


def clean_text(value, label):
    if not isinstance(value, str) or not value or any(char in value for char in '\x00\r\n'):
        raise ValueError(label + " must be a nonempty single-line string")
    return value


def load_config(path):
    config = json.loads(Path(path).read_text(encoding="utf-8-sig"))
    if not isinstance(config, dict) or config.get("transport", "ssh") != "ssh":
        raise ValueError("--config requires an SSH target; local execution requires --local")
    host = clean_text(config.get("host"), "host")
    if host.startswith("-") or any(char.isspace() for char in host):
        raise ValueError("host must be a single SSH destination, not options")
    for field in ("python", "root"):
        value = clean_text(config.get(field), field)
        if not PureWindowsPath(value).is_absolute():
            raise ValueError(field + " must be an absolute Windows path")
    # These characters are expanded by cmd.exe even in quoted executable paths.
    if any(char in config["python"] for char in '%!"'):
        raise ValueError("python path contains unsupported command-shell expansion characters")
    if PureWindowsPath(config["python"]).name.casefold() != "python.exe":
        raise ValueError("python must name python.exe (with pythonw.exe alongside it)")
    port = config.get("port", 22)
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("port must be an integer from 1 to 65535")
    config["port"] = port
    config["key_file"] = str(Path(clean_text(config.get("key_file"), "key_file")).expanduser().resolve())
    if not Path(config["key_file"]).is_file():
        raise ValueError("key_file does not exist: " + config["key_file"])
    if "sftp_drive_root" in config:
        prefix = clean_text(config["sftp_drive_root"], "sftp_drive_root")
        if not prefix.startswith("/") or '"' in prefix:
            raise ValueError("sftp_drive_root must be an absolute SFTP prefix")
    return config


def sftp_path(path, config):
    value = ntpath.normpath(str(path))
    if value.startswith("\\\\?\\UNC\\"):
        value = "\\\\" + value[8:]
    elif value.startswith("\\\\?\\"):
        value = value[4:]
    if "sftp_drive_root" in config:
        drive, tail = ntpath.splitdrive(value)
        if len(drive) != 2 or drive[1] != ":":
            raise ValueError("sftp_drive_root mapping requires a drive-letter path, not UNC")
        return config["sftp_drive_root"].rstrip("/") + "/" + drive[0] + tail.replace("\\", "/")
    return value.replace("\\", "/")


def sftp_quote(path):
    text = clean_text(str(path), "SFTP path")
    return '"' + text.replace("\\", "/").replace('"', '\\"') + '"'


def python_command(python, source):
    encoded = base64.b64encode(source.encode("utf-8")).decode("ascii")
    # Bitvise/cmd requires an outer pair in addition to the executable's quotes.
    return ('""{}" -c "import sys,base64;sys.stdout.reconfigure(encoding=\'utf-8\');'
            'sys.stderr.reconfigure(encoding=\'utf-8\');exec(base64.b64decode(\'{}\'))""'
            .format(python, encoded))


class SSH:
    def __init__(self, config, deadline):
        self.config = config
        self.deadline = deadline

    def options(self):
        result = ["-oBatchMode=yes", "-oConnectTimeout=10"]
        if "key_file" in self.config:
            result += ["-i", self.config["key_file"]]
        return result

    def call(self, source, *, cleanup=False):
        timeout = 25 if cleanup else max(0.01, min(25, self.deadline - time.monotonic()))
        command = ["ssh", *self.options(), "-p", str(self.config["port"]), "--",
                   self.config["host"], python_command(self.config["python"], source)]
        completed = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   stdin=subprocess.DEVNULL, timeout=timeout)
        if completed.returncode:
            raise RuntimeError("SSH failed ({}): {}".format(
                completed.returncode, completed.stderr.decode("utf-8", errors="replace").strip()))
        return completed.stdout.decode("utf-8-sig").strip()

    def transfer(self, commands, *, cleanup=False):
        timeout = 40 if cleanup else max(0.01, self.deadline - time.monotonic())
        command = ["sftp", *self.options(), "-P", str(self.config["port"]), "-b", "-", "--",
                   self.config["host"]]
        completed = subprocess.run(command, input=("\n".join(commands) + "\n").encode("utf-8"),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
        if completed.returncode:
            raise RuntimeError("SFTP failed ({}): {}".format(
                completed.returncode, completed.stderr.decode("utf-8", errors="replace").strip()))


PREFLIGHT = r'''
import ctypes, getpass, importlib.util, json, os, shutil, sys
from ctypes import wintypes
from pathlib import Path
if os.name != 'nt' or sys.version_info < (3, 8):
    raise RuntimeError('Target requires Windows and Python 3.8+')
for module in ('pytest', 'pytest_timeout'):
    if importlib.util.find_spec(module) is None:
        raise RuntimeError('Target Python is missing ' + module)
pythonw = Path(sys.executable).with_name('pythonw.exe')
windows = list(sys.getwindowsversion()[:2])
if not pythonw.is_file() or not shutil.which('taskkill.exe'):
    raise RuntimeError('pythonw.exe and taskkill.exe are required')
if windows[0] < 6:
    from win32com.taskscheduler import taskscheduler
elif not shutil.which('schtasks.exe'):
    raise RuntimeError('schtasks.exe is required')
class Session(ctypes.Structure):
    _fields_ = [('id', wintypes.DWORD), ('station', wintypes.LPWSTR), ('state', ctypes.c_int)]
wts = ctypes.WinDLL('wtsapi32', use_last_error=True)
wts.WTSEnumerateSessionsW.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD,
                                    ctypes.POINTER(ctypes.POINTER(Session)), ctypes.POINTER(wintypes.DWORD)]
wts.WTSQuerySessionInformationW.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.c_int,
                                         ctypes.POINTER(wintypes.LPWSTR), ctypes.POINTER(wintypes.DWORD)]
wts.WTSFreeMemory.argtypes = [wintypes.LPVOID]
sessions = ctypes.POINTER(Session)()
count = wintypes.DWORD()
if not wts.WTSEnumerateSessionsW(None, 0, 1, ctypes.byref(sessions), ctypes.byref(count)):
    raise ctypes.WinError(ctypes.get_last_error())
active = []
try:
    for session in sessions[:count.value]:
        if session.state != 0:
            continue
        name, size = wintypes.LPWSTR(), wintypes.DWORD()
        if not wts.WTSQuerySessionInformationW(None, session.id, 5, ctypes.byref(name), ctypes.byref(size)):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            if name.value and name.value.casefold() == getpass.getuser().casefold():
                active.append(session.id)
        finally:
            wts.WTSFreeMemory(name)
finally:
    wts.WTSFreeMemory(sessions)
if len(active) != 1:
    raise RuntimeError('Exactly one logged-in active session for the SSH user is required')
print(json.dumps({'pythonw': str(pythonw), 'session': active[0],
                  'windows': windows, 'platform': sys.platform}))
'''


LEGACY_SCHEDULER = r'''
import pythoncom
from win32com.taskscheduler import taskscheduler
scheduler = pythoncom.CoCreateInstance(taskscheduler.CLSID_CTaskScheduler, None,
    pythoncom.CLSCTX_INPROC_SERVER, taskscheduler.IID_ITaskScheduler)
'''


def suite_files():
    files = sorted(HERE.glob("*.py")) + [HERE / "pytest.ini"]
    if not files[-1].is_file():
        raise ValueError("GUI suite is missing pytest.ini")
    files += sorted(path for path in (HERE / "licenses").rglob("*") if path.is_file())
    files += sorted((HERE / "fixtures").glob("*.winmd"))
    return files


def input_hashes(sources):
    result = []
    for source, destination in sources:
        with source.open("rb") as stream:
            digest = hashlib.sha256()
            for block in iter(lambda: stream.read(1024 * 1024), b''):
                digest.update(block)
        result.append({"path": str(destination), "sha256": digest.hexdigest()})
    return result


def manifest(exe, winmd, output, args, timeout, files, probe_job=False, explicit_nodes=False):
    return {"exe": str(exe), "winmd": str(winmd), "output": str(output),
            "desktop_authorized": True, "timeout": timeout, "pytest_args": args, "files": files,
            "probe_job": probe_job, "explicit_nodes": explicit_nodes}


def extract_artifacts(archive, output):
    with zipfile.ZipFile(archive) as package:
        for name in package.namelist():
            path = PureWindowsPath(name)
            if path.root or path.drive or ".." in path.parts:
                raise ValueError("Unsafe artifact archive member: " + name)
        package.extractall(output)


class TerminalProgress:
    def __init__(self):
        self.offset = 0
        self.open_line = False

    def append(self, text):
        if text:
            print(text, end='', flush=True)
            self.offset += len(text)
            self.open_line = True

    def read(self, output):
        try:
            with (output / 'progress.txt').open('rb') as stream:
                stream.seek(self.offset)
                self.append(stream.read().decode('ascii'))
        except FileNotFoundError:
            pass

    def finish(self):
        if self.open_line:
            print(flush=True)
            self.open_line = False


def remote_run(config, args, output, run_id):
    deadline = time.monotonic() + args.timeout
    ssh = SSH(config, deadline)
    # No deployment, task creation or GUI launch before this read-only probe.
    capability = json.loads(ssh.call(PREFLIGHT))
    legacy = capability["windows"][0] < 6
    remote = PureWindowsPath(config["root"]) / run_id
    task = "WinlexGui-" + run_id
    print("Target run: {}  task: {}".format(remote, task), flush=True)
    launched = False
    task_created = False
    result_received = False
    progress = TerminalProgress()
    try:
        ssh.call("from pathlib import Path\np=Path({!r})\np.mkdir(parents=True, exist_ok=False)\n"
                 "[(p/n).mkdir() for n in ('bin', 'input', 'output')]".format(str(remote)))
        with tempfile.TemporaryDirectory(prefix="winlex-gui-") as staging:
            staging = Path(staging)
            originals = [(path, path.relative_to(HERE).as_posix()) for path in suite_files()]
            originals += [(args.exe, "bin/" + args.exe.name), (args.winmd, "input/" + args.winmd.name)]
            sources = []
            for source, relative in originals:
                staged = staging / relative
                staged.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, staged)
                sources.append((staged, remote / relative))
            value = manifest(remote / "bin" / args.exe.name, remote / "input" / args.winmd.name,
                             remote / "output", args.pytest_args,
                             max(0.01, deadline - time.monotonic()), input_hashes(sources),
                             args.probe_job, args.explicit_nodes)
            staged_manifest = staging / "manifest.json"
            staged_manifest.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")
            sources.append((staged_manifest, remote / "manifest.json"))
            suite_dirs = sorted({str(destination.parent) for _, destination in sources})
            ssh.call("from pathlib import Path\n"
                     "[Path(p).mkdir(parents=True, exist_ok=True) for p in {!r}]".format(suite_dirs))
            ssh.transfer(["put {} {}".format(sftp_quote(source),
                                             sftp_quote(sftp_path(destination, config)))
                          for source, destination in sources])
        ssh.call("import sys,json\nfrom pathlib import Path\nsys.path.insert(0,{!r})\n"
                 "from execute import verify_deployment,require_pytest_dependencies\n"
                 "require_pytest_dependencies()\n"
                 "verify_deployment(json.loads(Path({!r}).read_text(encoding='utf-8')))".format(
                     str(remote), str(remote / "manifest.json")))
        # Update only the remaining budget after transfer, not a new timeout.
        ssh.call("import json\nfrom pathlib import Path\np=Path({!r})\n"
                 "v=json.loads(p.read_text(encoding='utf-8'))\nv['timeout']={!r}\n"
                 "p.write_text(json.dumps(v),encoding='utf-8')".format(
                     str(remote / "manifest.json"), max(0.01, deadline - time.monotonic())))
        command = subprocess.list2cmdline([capability["pythonw"], str(remote / "execute.py"),
                                          "--manifest", str(remote / "manifest.json")])
        task_created = True  # A lost SSH reply may still have created this UUID task.
        if legacy:
            ssh.call(LEGACY_SCHEDULER + "import win32api\n"
                     "task=scheduler.NewWorkItem({!r})\n"
                     "task.SetApplicationName({!r})\n"
                     "task.SetParameters({!r})\n"
                     "task.SetWorkingDirectory({!r})\n"
                     "task.SetFlags(taskscheduler.TASK_FLAG_RUN_ONLY_IF_LOGGED_ON)\n"
                     "task.SetAccountInformation(win32api.GetUserName(),None)\n"
                     "task.QueryInterface(pythoncom.IID_IPersistFile).Save(None,True)".format(
                         task, capability["pythonw"],
                         subprocess.list2cmdline([str(remote / "execute.py"), "--manifest",
                                                  str(remote / "manifest.json")]), str(remote)))
        else:
            # Today's midnight is in the past: only the explicit /Run starts this task.
            create = ["schtasks.exe", "/Create", "/SC", "ONCE", "/ST", "00:00", "/TN", task,
                      "/TR", command, "/IT"]
            ssh.call("import subprocess\nfrom datetime import datetime\nnow=datetime.now()\n"
                     "if now.hour == 0 and now.minute == 0:\n"
                     " raise RuntimeError('Refusing midnight task creation: /ST must be in the past')\n"
                     "subprocess.run({!r},check=True,stdin=subprocess.DEVNULL,"
                     "stdout=subprocess.PIPE,stderr=subprocess.STDOUT)".format(create))
        launched = True  # Treat an uncertain /Run response as potentially started.
        if legacy:
            ssh.call(LEGACY_SCHEDULER + "scheduler.Activate({!r}).Run()".format(task))
        else:
            ssh.call("import subprocess\nsubprocess.run({!r},check=True,stdin=subprocess.DEVNULL,"
                     "stdout=subprocess.PIPE,stderr=subprocess.STDOUT)".format(
                         ["schtasks.exe", "/Run", "/TN", task]))
        while time.monotonic() < deadline:
            # Read completion first: a published result guarantees the progress
            # writer has exited, so this same query also drains the final bytes.
            state = json.loads(ssh.call("import json\nfrom pathlib import Path\np=Path({!r})\n"
                                       "r=p/'result.json'\n"
                                       "result=json.loads(r.read_text(encoding='utf-8')) if r.is_file() else None\n"
                                       "try:\n"
                                       " with (p/'progress.txt').open('rb') as f:\n"
                                       "  f.seek({!r})\n  progress=f.read().decode('ascii')\n"
                                       "except FileNotFoundError:\n progress=''\n"
                                       "print(json.dumps(dict(result=result,progress=progress)))".format(
                                           str(remote / "output"), progress.offset)))
            progress.append(state['progress'])
            if state['result'] is not None:
                result = state['result']
                result_received = True
                break
            time.sleep(min(1, max(0, deadline - time.monotonic())))
        else:
            raise TimeoutError("Whole-run timeout expired")
        progress.finish()
        # Collection has a bounded grace period; it never restarts target execution.
        ssh.call("import zipfile\nfrom pathlib import Path\np=Path({!r})\n"
                 "with zipfile.ZipFile(str(p/'artifacts.zip'),'w',zipfile.ZIP_DEFLATED) as z:\n"
                 " for f in (p/'output').rglob('*'):\n"
                 "  if f.is_file(): z.write(str(f),str(f.relative_to(p/'output')))".format(str(remote)),
                 cleanup=True)
        archive = output / "artifacts.zip"
        ssh.transfer(["get {} {}".format(sftp_quote(sftp_path(remote / "artifacts.zip", config)),
                                        sftp_quote(archive))], cleanup=True)
        extract_artifacts(archive, output)
        archive.unlink()
        return int(result["exit_code"])
    except BaseException:
        progress.finish()
        if launched and not result_received:
            try:
                stop = (LEGACY_SCHEDULER + "scheduler.Activate({!r}).Terminate()".format(task)
                        if legacy else "import subprocess\nsubprocess.run({!r},check=True,"
                        "stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)"
                        .format(["schtasks.exe", "/End", "/TN", task]))
                detail = ssh.call("import sys,json\nfrom pathlib import Path\nsys.path.insert(0,{!r})\n"
                                  "from execute import cancel_run\ndirectory=Path({!r})\n"
                                  "if not (directory/'output'/'result.json').is_file():\n"
                                  " (directory/'cancel.requested').touch()\n"
                                  " if not (directory/'execute.pid').is_file():\n"
                                  "  try:\n   exec({!r})\n"
                                  "  except Exception as error:\n"
                                  "   print('Task stop unconfirmed: '+str(error),file=sys.stderr)\n"
                                  "print(json.dumps(cancel_run(directory)))"
                                  .format(str(remote), str(remote), stop), cleanup=True)
                print("Cancellation check: " + detail, file=sys.stderr)
                (output / 'result.json').write_text(detail, encoding='utf-8')
            except Exception as error:
                print("RESULT UNKNOWN; target cleanup unconfirmed: {}".format(error), file=sys.stderr)
        elif result_received:
            print("Target completed, but artifact retrieval failed; do not replay.", file=sys.stderr)
        print("Preserved target run: {}  task: {}".format(remote, task), file=sys.stderr)
        raise
    finally:
        progress.finish()
        if task_created:
            try:
                if legacy:
                    ssh.call(LEGACY_SCHEDULER + "scheduler.Delete({!r})".format(task), cleanup=True)
                else:
                    ssh.call("import subprocess\nsubprocess.run({!r},check=True,stdin=subprocess.DEVNULL,"
                             "stdout=subprocess.PIPE,stderr=subprocess.STDOUT)".format(
                                 ["schtasks.exe", "/Delete", "/TN", task, "/F"]), cleanup=True)
            except Exception as error:
                print("Could not delete this run's task {}: {}".format(task, error), file=sys.stderr)


def local_run(args, output):
    # This branch is unreachable without both explicit local authorization flags.
    import execute
    execute.require_pytest_dependencies()
    execute.require_interactive_desktop()
    # Keep this run's source and manifest available even after uncertain cancellation.
    folder = output / "suite"
    folder.mkdir()
    sources = []
    for source in suite_files():
        target = folder / source.relative_to(HERE)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        sources.append((target, target))
    sources += [(args.exe, args.exe), (args.winmd, args.winmd)]
    path = folder / "manifest.json"
    path.write_text(json.dumps(manifest(args.exe, args.winmd, output,
                                        args.pytest_args, args.timeout, input_hashes(sources),
                                        args.probe_job, args.explicit_nodes)),
                    encoding="utf-8")
    process = subprocess.Popen([sys.executable, str(folder / "execute.py"), "--manifest", str(path)],
                               creationflags=subprocess.CREATE_NO_WINDOW)
    progress = TerminalProgress()
    try:
        deadline = time.monotonic() + args.timeout + 20
        while process.poll() is None:
            progress.read(output)
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired(process.args, args.timeout + 20)
            try:
                process.wait(timeout=min(0.1, remaining))
            except subprocess.TimeoutExpired:
                pass
        progress.read(output)
        return process.returncode
    except BaseException:
        progress.finish()
        try:
            result_path = output / "result.json"
            if process.poll() is None:
                termination = execute.terminate_tree(process.pid)
                process.wait(timeout=10)
                if termination["taskkill_exit_code"] != 0:
                    raise RuntimeError("Local execute tree termination unconfirmed")
                if not result_path.is_file():
                    cleanup = execute.cleanup_applications(output)
                    cleanup.update(termination)
                    execute.write_result(output, {"exit_code": 130, "timed_out": False,
                                                   "cancelled": True, "cleanup": cleanup})
            if not result_path.is_file():
                raise RuntimeError("Execute ended without a final result; remaining tree unknown")
            print("Cancellation check: " + result_path.read_text(encoding="utf-8"), file=sys.stderr)
        except Exception as error:
            print("RESULT UNKNOWN; cleanup unconfirmed: {}; run: {}".format(error, folder), file=sys.stderr)
        raise
    finally:
        progress.finish()


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, epilog="Pass pytest options after --.")
    transport = parser.add_mutually_exclusive_group(required=True)
    transport.add_argument("--local", action="store_true", help="use this computer's desktop")
    transport.add_argument("--config", type=Path, help="SSH target JSON (never falls back locally)")
    parser.add_argument("--allow-local-desktop", action="store_true", help="authorize local GUI activity")
    parser.add_argument("--exe", required=True, type=Path, help="existing target-architecture Debug GUI EXE")
    parser.add_argument("--winmd", required=True, type=Path, help="WinMD input to deploy")
    parser.add_argument("--output", type=Path, help="empty local result directory")
    parser.add_argument("--timeout", type=float, default=120, help="whole-run budget in seconds (default: 120)")
    parser.add_argument("--probe-job", action="store_true", help="check Job containment only; never launch GUI/pytest")
    parser.add_argument("pytest_args", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.local and not args.allow_local_desktop:
        parser.error("--local also requires --allow-local-desktop")
    if args.config and args.allow_local_desktop:
        parser.error("--allow-local-desktop is only valid with --local")
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("--timeout must be positive and finite")
    if args.pytest_args[:1] == ["--"]:
        args.pytest_args = args.pytest_args[1:]
    if any(option.split("=", 1)[0] in RESERVED for option in args.pytest_args):
        parser.error("pytest arguments must not override runner paths or desktop authorization")
    # Catch accidental external test nodes, not every possible plugin argument grammar.
    value_options = {"-k", "-m", "-o", "--override-ini", "--fault-mode", "--tb", "--capture",
                     "--color", "--durations", "--maxfail", "--timeout", "--timeout-method"}
    skip = False
    suite_names = {path.name for path in HERE.glob("test_*.py")} | {'fault_probe.py'}
    args.explicit_nodes = False
    for option in args.pytest_args:
        if skip:
            skip = False
            continue
        if option in value_options:
            skip = True
            continue
        if option.startswith("-"):
            continue
        node = option.split("::", 1)[0]
        path = PureWindowsPath(node)
        if path.root or path.drive or ".." in path.parts:
            parser.error("explicit pytest nodes must stay inside this GUI suite")
        if node.endswith(".py") or "::" in option:
            if len(path.parts) != 1 or path.name not in suite_names:
                parser.error("explicit pytest nodes must be files in this GUI suite")
            args.explicit_nodes = True
    return args


def main(argv=None):
    if hasattr(signal, 'SIGBREAK'):
        signal.signal(signal.SIGBREAK, signal.default_int_handler)
    args = parse_args(argv)
    try:
        config = load_config(args.config) if args.config else None
        for field in ("exe", "winmd"):
            path = getattr(args, field).expanduser().resolve()
            if not path.is_file():
                raise ValueError("{} does not exist: {}".format(field, path))
            setattr(args, field, path)
        run_id = uuid.uuid4().hex
        output = (args.output or HERE.parents[1] / "tmp" / "gui-tests" / run_id).expanduser().resolve()
        if output.exists() and (not output.is_dir() or any(output.iterdir())):
            raise ValueError("--output must be absent or empty")
        output.mkdir(parents=True, exist_ok=True)
        print("Local artifacts: " + str(output), flush=True)
        return remote_run(config, args, output, run_id) if config else local_run(args, output)
    except KeyboardInterrupt:
        print("Cancelled; no run was replayed.", file=sys.stderr)
        return 130
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError, TimeoutError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())

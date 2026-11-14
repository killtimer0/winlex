"""File-queue transport. Run `worker` from the target interactive logon, never SSH.

The SSH endpoint only performs queue/file operations. Each run gets its own
directory, log, cancellation marker and result. No adjacent checkout dependency.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import uuid
import zipfile

from support import atomic, engine, progress, supervisor
from support.policy import Policy


def run_path(root, run_id):
    if len(run_id) != 32 or any(c not in "0123456789abcdef" for c in run_id):
        raise ValueError("Invalid run id")
    return root / run_id


def connection(config, program):
    command = [program, "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]
    if config.get("key_file"):
        command += ["-i", str(Path(config["key_file"]).expanduser())]
    if config.get("port"):
        command += ["-P" if program == "scp" else "-p", str(config["port"])]
    return command


def transfer_path(config, path):
    path = path.replace("\\", "/")
    prefix = config.get("sftp_drive_root")
    if prefix is not None and len(path) > 2 and path[1:3] == ":/":
        return prefix.rstrip("/") + "/" + path[0].upper() + path[2:]
    return path


def rpc(root, request):
    root.mkdir(parents=True, exist_ok=True)
    action = request["action"]
    if action == "capabilities":
        path = root / "capabilities.json"
        if not path.exists():
            return {"worker_available": False}
        result = json.loads(path.read_text(encoding="utf-8"))
        result["heartbeat_age"] = time.time() - result["heartbeat"]
        result["worker_available"] = result.get('worker_available', True) and result["heartbeat_age"] < 10
        if result['worker_available']:
            import support
            import win32api
            import win32con
            import win32event
            try:
                process = win32api.OpenProcess(win32con.SYNCHRONIZE, False, result['pid'])
            except win32api.error:
                result['worker_available'] = False
            else:
                try:
                    result['worker_available'] = win32event.WaitForSingleObject(process, 0) == win32con.WAIT_TIMEOUT
                finally:
                    win32api.CloseHandle(process)
        return result
    if action == "start-worker":
        current = rpc(root, {"action": "capabilities"})
        if current["worker_available"]:
            return current
        import support
        import pythoncom
        import win32api
        from win32com.taskscheduler import taskscheduler as t
        scheduler = pythoncom.CoCreateInstance(t.CLSID_CTaskScheduler, None,
            pythoncom.CLSCTX_INPROC_SERVER, t.IID_ITaskScheduler)
        name = "Winlex.Tests." + hashlib.sha256(str(root).casefold().encode()).hexdigest()[:12]
        names = {entry.casefold() for entry in scheduler.Enum()}
        task = scheduler.Activate(name) if (name + ".job").casefold() in names else scheduler.NewWorkItem(name)
        interpreter = Path(sys.executable).with_name('pythonw.exe')
        if not interpreter.is_file():
            raise FileNotFoundError('Scheduled worker requires windowless Python: ' + str(interpreter))
        task.SetApplicationName(str(interpreter))
        task.SetParameters(subprocess.list2cmdline([str(Path(__file__).resolve()), "worker", "--root", str(root)]))
        task.SetWorkingDirectory(str(Path(__file__).resolve().parent))
        task.SetFlags(t.TASK_FLAG_RUN_ONLY_IF_LOGGED_ON)
        task.SetAccountInformation(win32api.GetUserName(), None)
        task.SetMaxRunTime(-1)
        task.QueryInterface(pythoncom.IID_IPersistFile).Save(None, True)
        task.Run()
        return {"state": "worker_start_requested", "task": name}
    if action == "watch":
        deadline = time.monotonic() + request["timeout"]
        while True:
            result = rpc(root, {**request, "action": "status"})
            if result.get("state") not in ("running", "queued"):
                return {**result, "watch_event": "finished"}
            state = result.get("progress")
            if state and state["active"] and state["elapsed"] >= request["slow_after"]:
                return {**result, "watch_event": "slow_phase"}
            if time.monotonic() >= deadline:
                return {**result, "watch_event": "watch_timeout"}
            time.sleep(.25)
    path = run_path(root, request["run_id"])
    if action == "submit":
        path.mkdir()
        atomic.write_json(path / "request.json", request)
        return {"run_id": request["run_id"], "state": "queued"}
    if action == "cancel":
        (path / "cancel").touch()
        return {"cancel_requested": True}
    if action == "status":
        failure = path / 'supervisor.json'
        if failure.exists():
            return json.loads(failure.read_text(encoding='utf-8'))
        result = path / "run.json"
        if result.exists():
            return json.loads(result.read_text(encoding="utf-8"))
        result = {"state": "running" if (path / "started.json").exists() else "queued"}
        active = progress.active(path)
        if active is not None:
            result["progress"] = active
        return result
    if action == "collect":
        from support.paths import extended
        root = extended(path)
        archive = path.with_suffix(".zip")
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
            for file in root.rglob("*"):
                if file.is_file():
                    output.write(file, str(file.relative_to(root)))
        return {"archive": str(archive)}
    raise ValueError(action)


def worker(root, once=False):
    from support import native
    import platform
    import win32api
    import win32con
    import win32service
    import win32event
    desktop = win32service.GetThreadDesktop(win32api.GetCurrentThreadId())
    station = win32service.GetProcessWindowStation()
    station_name = win32service.GetUserObjectInformation(int(station), win32con.UOI_NAME)
    if station_name.lower() != "winsta0":
        raise RuntimeError("Worker must run from an interactive logon")
    desktop_name = win32service.GetUserObjectInformation(int(desktop), win32con.UOI_NAME)
    mutex = win32event.CreateMutex(None, False, "Local\\Winlex.Tests.Worker." + desktop_name)
    if win32api.GetLastError() == 183:
        win32api.CloseHandle(mutex)
        raise RuntimeError("Another Winlex worker owns this desktop")
    root.mkdir(parents=True, exist_ok=True)
    capabilities = {"worker_available": True, "pid": os.getpid(), "os": platform.platform(),
                    "python": sys.version, "executable": sys.executable, "desktop": desktop_name,
                    **native.capabilities()}
    while True:
        atomic.write_json(root / "capabilities.json", {**capabilities, "heartbeat": time.time()})
        for request in sorted(root.glob("*/request.json")):
            path = request.parent
            if (path / "run.json").exists() or (path / "started.json").exists():
                continue
            # Atomic claim also prevents two workers executing the same request.
            try:
                with (path / "started.json").open("x", encoding="utf-8") as stream:
                    json.dump({"worker": os.getpid(), "time": time.time()}, stream)
            except FileExistsError:
                continue
            data = json.loads(request.read_text(encoding="utf-8"))
            if (path / "cancel").exists():
                atomic.write_json(path / "run.json", {"cancelled": True, "cleanup_complete": True, "state": "cancelled_before_start"})
                continue
            policy = Policy.load().overrides(
                run=data["timeout"], call=data.get("case_timeout"),
                stall=data.get("stall_after"),
                consecutive_stalls=data.get("consecutive_stalls"))
            command = engine.command(path, data["tests"], policy, data["desktop"])
            # Read-write so the engine and pytest can query the redirected
            # standard handle; a write-only handle is rejected on XP.
            log = os.fdopen(os.open(path / "output.log",
                                    os.O_RDWR | os.O_CREAT | os.O_TRUNC),
                            "w", encoding="utf-8")
            with log:
                started = time.monotonic()
                process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                           env={**os.environ, "PYTHONUTF8": "1"})
                try:
                    safe = supervisor.supervise_runner(process, path, policy.run, started,
                        lambda: atomic.write_json(root / 'capabilities.json', {**capabilities, 'heartbeat': time.time()}),
                        policy=policy)
                except KeyboardInterrupt:
                    (path / "cancel").touch()
                    safe = supervisor.supervise_runner(process, path, policy.run, started,
                        lambda: atomic.write_json(root / 'capabilities.json', {**capabilities, 'heartbeat': time.time()}),
                        policy=policy)
                    if safe:
                        raise
            if not safe:
                atomic.write_json(root / 'capabilities.json', {**capabilities, 'heartbeat': time.time(),
                    'worker_available': False, 'error': 'Runner cleanup unverified; queue stopped'})
                return
        if once:
            return
        time.sleep(.25)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", type=Path)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--request")
    parser.add_argument("--once", action="store_true")
    parser.add_argument("action", choices=(
        "worker", "rpc", "start-worker", "capabilities", "submit", "status", "watch", "cancel", "collect", "deploy",
    ))
    parser.add_argument("--source", type=Path)
    parser.add_argument("--destination")
    parser.add_argument("--run-id")
    parser.add_argument("--timeout", type=float, default=Policy.load().run)
    parser.add_argument("--case-timeout", type=int)
    parser.add_argument("--stall-after", type=float)
    parser.add_argument("--consecutive-stalls", type=int)
    parser.add_argument("--slow-after", type=float, default=15)
    parser.add_argument("--desktop", choices=("isolated", "current"))
    parser.add_argument("--output", type=Path, default=Path("tmp/remote-tests"))
    args, tests = parser.parse_known_args()
    if args.slow_after <= 0:
        parser.error("--slow-after must be positive")
    if args.timeout <= 0 or any(value is not None and value <= 0
                                for value in (args.case_timeout, args.stall_after,
                                              args.consecutive_stalls)):
        parser.error("timeout budgets must be positive")
    if args.action == "worker":
        worker(args.root.resolve(), args.once)
        return
    if args.action == "rpc":
        print(json.dumps(rpc(args.root.resolve(), json.loads(base64.b64decode(args.request)))))
        return
    config = json.loads(args.config.read_text(encoding="utf-8"))
    # A target may pin its desktop: on XP the automation provider times out for
    # about 2s per method call whenever the window is not on the input desktop.
    desktop = args.desktop or config.get("desktop", "isolated")
    if desktop not in ("isolated", "current"):
        parser.error("desktop must be isolated or current")
    if args.action == "submit" and "--real-input" in tests and desktop != "current":
        parser.error("--real-input requires --desktop current")
    if args.action == "deploy":
        if not args.source or not args.destination:
            parser.error("deploy requires --source and --destination")
        if config["transport"] == "local":
            if args.source.is_dir():
                shutil.copytree(args.source, args.destination, dirs_exist_ok=True)
            else:
                Path(args.destination).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(args.source, args.destination)
        else:
            subprocess.run(connection(config, "scp") + [
                "-r", str(args.source), config["host"] + ":" + transfer_path(config, args.destination),
            ], check=True, timeout=max(180, args.timeout) + 30)
        print(json.dumps({"deployed": args.destination}))
        return
    request = {"action": args.action, "run_id": args.run_id or uuid.uuid4().hex,
               "timeout": args.timeout, "case_timeout": args.case_timeout,
               "stall_after": args.stall_after,
               "consecutive_stalls": args.consecutive_stalls,
               "slow_after": args.slow_after, "desktop": desktop, "tests": tests}
    if config["transport"] == "local":
        result = rpc(Path(config["root"]).resolve(), request)
    else:
        payload = base64.b64encode(json.dumps(request).encode()).decode()
        # Windows OpenSSH target command line; GUI starts only in the worker.
        command = subprocess.list2cmdline([config["python"], config["script"], "rpc", "--root", config["root"], "--request", payload])
        # Submission only queues work; watch must allow its full requested wait.
        # Archive creation and transfers keep the default run budget for large files.
        timeout = 30
        if args.action == "watch":
            timeout += max(0, args.timeout)
        elif args.action == "collect":
            timeout += max(180, args.timeout)
        result = json.loads(subprocess.check_output(connection(config, "ssh") + [config["host"], command],
                                                   text=True, encoding="utf-8", timeout=timeout))
    if args.action == "collect":
        args.output.mkdir(parents=True, exist_ok=True)
        destination = args.output / (request["run_id"] + ".zip")
        if config["transport"] == "local":
            shutil.copyfile(result["archive"], destination)
        else:
            subprocess.run(connection(config, "scp") + [
                config["host"] + ":" + transfer_path(config, result["archive"]), str(destination),
            ], check=True, timeout=max(180, args.timeout) + 30)
        result["local_archive"] = str(destination)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()

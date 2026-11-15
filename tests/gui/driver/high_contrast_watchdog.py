"""Standalone scheduler supervisor; importable without pytest or UI Automation."""
import sys
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ctypes as c
from ctypes import wintypes as w
import json
import os
import subprocess
import time
import traceback
import uuid
from support import bounded


SPI_GETHIGHCONTRAST = 0x0042
SPI_SETHIGHCONTRAST = 0x0043
HCF_HIGHCONTRASTON = 0x0001
HCF_AVAILABLE = 0x0002


class ObscuredSample(AssertionError):
    pass


class HighContrast(c.Structure):
    _fields_ = [('size', w.UINT), ('flags', w.DWORD), ('scheme', c.c_void_p)]


def spi_high_contrast(action, state, flags=0):
    spi = c.WinDLL('user32', use_last_error=True).SystemParametersInfoW
    spi.restype, spi.argtypes = w.BOOL, (w.UINT, w.UINT, c.c_void_p, w.UINT)
    if not spi(action, state.size, c.byref(state), flags):
        raise c.WinError(c.get_last_error())


def high_contrast():
    state = HighContrast()
    state.size = c.sizeof(state)
    spi_high_contrast(SPI_GETHIGHCONTRAST, state)
    # Borrowed pointer: the 19045 native probe returned the same address for
    # four GETs, ignoring caller buffers and leaving their sentinels untouched.
    # Copy immediately; LocalFree of the cached address caused 0xc0000374.
    return state.flags, c.wstring_at(state.scheme) if state.scheme else None


def restored_high_contrast(state, snapshot):
    # Windows materializes an empty scheme name on the first transition and
    # cannot represent that initialized state as an empty string again.
    return state[0] == snapshot[0] and (
        state[1] == snapshot[1] or snapshot[1] == '' and state[1] is not None)


def set_high_contrast(snapshot):
    flags, scheme = snapshot
    buffer = c.create_unicode_buffer(scheme) if scheme is not None else None
    state = HighContrast(c.sizeof(HighContrast), flags,
                         c.addressof(buffer) if buffer is not None else None)
    # Broadcast the real setting change, but never SPIF_UPDATEINIFILE. Do not
    # use HCF_OPTION_NOTHEMECHANGE: Windows forbids it for an ON/OFF transition.
    spi_high_contrast(SPI_SETHIGHCONTRAST, state, 0x0002)


def execution_context():
    """Identify the desktop whose native settings this process can observe."""
    kernel = c.WinDLL('kernel32', use_last_error=True)
    user = c.WinDLL('user32', use_last_error=True)
    kernel.ProcessIdToSessionId.restype = w.BOOL
    kernel.ProcessIdToSessionId.argtypes = (w.DWORD, c.POINTER(w.DWORD))
    kernel.GetCurrentThreadId.restype, kernel.GetCurrentThreadId.argtypes = w.DWORD, ()
    user.GetProcessWindowStation.restype, user.GetProcessWindowStation.argtypes = w.HANDLE, ()
    user.GetThreadDesktop.restype, user.GetThreadDesktop.argtypes = w.HANDLE, (w.DWORD,)
    user.GetUserObjectInformationW.restype = w.BOOL
    user.GetUserObjectInformationW.argtypes = (
        w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD))
    session = w.DWORD()
    if not kernel.ProcessIdToSessionId(os.getpid(), c.byref(session)):
        raise c.WinError(c.get_last_error())

    def name(handle):
        if not handle:
            raise c.WinError(c.get_last_error())
        needed = w.DWORD()
        if not user.GetUserObjectInformationW(handle, 2, None, 0, c.byref(needed)):
            if c.get_last_error() != 122:  # ERROR_INSUFFICIENT_BUFFER
                raise c.WinError(c.get_last_error())
        if not needed.value:
            raise RuntimeError('Native desktop context name has no buffer size')
        buffer = c.create_unicode_buffer((needed.value + c.sizeof(c.c_wchar) - 1) //
                                         c.sizeof(c.c_wchar))
        if not user.GetUserObjectInformationW(handle, 2, buffer, c.sizeof(buffer),
                                             c.byref(needed)):
            raise c.WinError(c.get_last_error())
        return buffer.value

    return {'session_id': session.value,
            'window_station': name(user.GetProcessWindowStation()),
            'desktop': name(user.GetThreadDesktop(kernel.GetCurrentThreadId()))}


def probe_high_contrast():
    """Read-only native probe; never free an unproven-ownership pointer."""
    states, buffers, rows = [], [], []
    for index in range(4):
        buffer = c.create_unicode_buffer('caller-buffer-sentinel', 32768)
        buffers.append(buffer)
        state = HighContrast(c.sizeof(HighContrast), 0,
                             c.addressof(buffer) if index % 2 else None)
        supplied = state.scheme
        spi_high_contrast(SPI_GETHIGHCONTRAST, state)
        states.append(state)
        rows.append({'supplied': supplied, 'returned': state.scheme,
                     'caller_buffer': buffer.value, 'flags': state.flags,
                     'scheme': c.wstring_at(state.scheme) if state.scheme else None})
    print(json.dumps({'windows': list(sys.getwindowsversion()[:3]), 'reads': rows}, indent=2))


def task_scheduler():
    # The bounded import above initializes support's XP/pywin32 DLL loading.
    import pythoncom
    from win32com.taskscheduler import taskscheduler as t
    return pythoncom.CoCreateInstance(t.CLSID_CTaskScheduler, None,
                                     pythoncom.CLSCTX_INPROC_SERVER, t.IID_ITaskScheduler)


def delete_watchdog_task(name):
    prefix = 'Winlex.Tests.HighContrast.'
    if not name.startswith(prefix) or uuid.UUID(hex=name[len(prefix):]).hex != name[len(prefix):]:
        raise ValueError('Invalid owned watchdog task name')
    def remove():
        initialized = False
        scheduler = None
        try:
            import pythoncom
            pythoncom.CoInitialize()
            initialized = True
            scheduler = task_scheduler()
            if (name + '.job').casefold() in {entry.casefold() for entry in scheduler.Enum()}:
                from support.task_scheduler import delete_work_item
                delete_work_item(name)
        finally:
            if initialized:
                scheduler = None
                pythoncom.CoUninitialize()

    bounded.call(remove, 10, 'High-contrast watchdog task cleanup')


def watchdog_path(artifact, task_name, suffix):
    return artifact.with_suffix('.' + task_name.rsplit('.', 1)[1] + suffix)


def watchdog_record(path, text):
    temporary = path.with_suffix(path.suffix + '.new')
    temporary.write_text(text, encoding='utf-8')
    temporary.replace(path)


def restoration_observation():
    """Native-only observation for a new process, independent of UIA/pytest."""
    user = c.WinDLL('user32', use_last_error=True)
    user.GetSysColor.argtypes, user.GetSysColor.restype = (c.c_int,), w.DWORD
    palette = []
    for index in (5, 15, 13):  # COLOR_WINDOW, COLOR_BTNFACE, COLOR_HIGHLIGHT
        color = user.GetSysColor(index)
        palette.append([(color >> 16) & 255, (color >> 8) & 255, color & 255])
    covers = []
    callback_type = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    user.IsWindowVisible.argtypes, user.IsWindowVisible.restype = (w.HWND,), w.BOOL
    user.GetClassNameW.argtypes = (w.HWND, w.LPWSTR, c.c_int)
    user.EnumWindows.argtypes, user.EnumWindows.restype = (callback_type, w.LPARAM), w.BOOL

    def visit(hwnd, _):
        name = c.create_unicode_buffer(256)
        if user.IsWindowVisible(hwnd) and user.GetClassNameW(hwnd, name, len(name)):
            if name.value == 'CoverWindowClass':
                covers.append(hwnd)
        return True

    if not user.EnumWindows(callback_type(visit), 0):
        raise c.WinError(c.get_last_error())
    return {'state': high_contrast(), 'palette': palette, 'covers': covers,
            'context': execution_context()}


def verify_restoration(artifact, restore=False):
    """Fresh-process evidence, only after a verified application boundary."""
    saved = json.loads(artifact.read_text(encoding='utf-8'))
    if execution_context() != saved['context']:
        raise RuntimeError('Native verification context does not match caller')
    if restore:
        set_high_contrast((saved['flags'], saved['scheme']))
    actual = restoration_observation()
    if actual['context'] != saved['context']:
        raise RuntimeError('Native verification context does not match caller')
    print(json.dumps(actual), flush=True)


def fresh_restoration(artifact, until, restore=False):
    saved = json.loads(artifact.read_text(encoding='utf-8'))
    actual = None
    while time.monotonic() < until:
        action = '--restore-native' if restore else '--verify-restoration'
        result = subprocess.run([sys.executable, str(Path(__file__).resolve()),
                                 action, str(artifact)],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=max(.001, until - time.monotonic()))
        if result.returncode:
            raise RuntimeError(result.stderr.decode('utf-8', errors='replace'))
        actual = json.loads(result.stdout)
        was_setter = restore
        restore = False
        if was_setter:
            # SET can update only the caller's cached view before Windows
            # finishes the transition. Never let the setter certify itself.
            continue
        if (restored_high_contrast(actual['state'], (saved['flags'], saved['scheme']))
                and actual['palette'] == saved['original_palette'] and not actual['covers']):
            return actual
        time.sleep(min(.05, max(0, until - time.monotonic())))
    raise RuntimeError('Fresh native restoration unverified: ' + json.dumps(actual))


def watchdog(parent_pid, artifact, task_name, receipt):
    """Scheduler-launched supervisor, independent of the pytest process/job."""
    kernel = c.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.restype, kernel.OpenProcess.argtypes = w.HANDLE, (w.DWORD, w.BOOL, w.DWORD)
    kernel.WaitForSingleObject.restype, kernel.WaitForSingleObject.argtypes = w.DWORD, (w.HANDLE, w.DWORD)
    kernel.CloseHandle.restype, kernel.CloseHandle.argtypes = w.BOOL, (w.HANDLE,)
    handle = None
    ready = watchdog_path(artifact, task_name, '.ready')
    try:
        handle = kernel.OpenProcess(0x00100000, False, parent_pid)
        if not handle:
            raise c.WinError(c.get_last_error())
        saved = json.loads(artifact.read_text(encoding='utf-8'))
        snapshot = saved['flags'], saved['scheme']
        receipt['expected'] = saved
        receipt['context'] = execution_context()
        if receipt['context'] != saved['context']:
            raise RuntimeError('Watchdog execution context does not match caller')
        watchdog_record(ready, json.dumps({'pid': os.getpid(),
                                         'context': receipt['context']}))
        completed = watchdog_path(artifact, task_name, '.restored')
        deadline = time.monotonic() + 120
        while True:
            result = kernel.WaitForSingleObject(handle, 100)
            emergency = result == 0 or time.monotonic() >= deadline
            if emergency or completed.exists():
                # The application acknowledgement is committed after the
                # independent snapshot was first published, before enable.
                saved = json.loads(artifact.read_text(encoding='utf-8'))
                receipt['expected'] = saved
                actual = high_contrast()
                receipt['actual'] = {'flags': actual[0], 'scheme': actual[1]}
                receipt['emergency'] = emergency
                receipt['recovery_needed'] = not restored_high_contrast(actual, snapshot)
                if emergency or receipt['recovery_needed']:
                    # Keep native recovery bounded in this standalone supervisor.
                    recovery = {}
                    recovery_until = time.monotonic() + 10

                    def recover():
                        recovery['recovery_context'] = execution_context()
                        if recovery['recovery_context'] != saved['context']:
                            raise RuntimeError('Native recovery context does not match caller')
                        recovery['native_verification'] = fresh_restoration(
                            artifact, recovery_until, restore=True)
                        restored = recovery['native_verification']['state']
                        recovery['restored'] = {'flags': restored[0], 'scheme': restored[1]}
                        if not restored_high_contrast(restored, snapshot):
                            raise RuntimeError('Watchdog could not restore entire HIGHCONTRAST snapshot')
                        return recovery

                    task = bounded.start(recover)
                    try:
                        task.wait(max(.001, recovery_until - time.monotonic()), 'Watchdog native recovery')
                    except BaseException:
                        receipt.update(recovery)
                        raise
                    receipt.update(recovery)
                else:
                    receipt['restored'] = receipt['actual']
                    receipt['native_verification'] = fresh_restoration(artifact, time.monotonic() + 10)
                if not saved.get('application_verified'):
                    raise RuntimeError('HC application was interrupted; restoration settlement is unverified')
                if not emergency and receipt['recovery_needed']:
                    raise RuntimeError('Parent restoration did not stick; watchdog recovery was needed')
                return
            if result != 0x102:
                raise RuntimeError('Watchdog parent wait failed: %#x' % result)
    finally:
        if handle:
            kernel.CloseHandle(handle)
        if ready.exists():
            ready.unlink()


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] in ('--verify-restoration', '--restore-native'):
        verify_restoration(Path(sys.argv[2]), restore=sys.argv[1] == '--restore-native')
    elif sys.argv[1:] == ['--probe-high-contrast']:
        probe_high_contrast()
    elif len(sys.argv) == 5 and sys.argv[1] == '--watch-high-contrast':
        if os.environ.get('PYTHONMALLOC') != 'malloc':
            # pywin32 312's Task Scheduler wrappers free Python memory while
            # the GIL is released. The scheduler does not inherit the test
            # runner's environment, so start the watchdog with the same safe
            # allocator before it uses those bindings for task cleanup.
            raise SystemExit(subprocess.call(
                [sys.executable, *sys.argv],
                env={**os.environ, 'PYTHONMALLOC': 'malloc'}))
        artifact, task_name = Path(sys.argv[3]), sys.argv[4]
        result = watchdog_path(artifact, task_name, '.result.json')
        receipt = {'ok': False}
        try:
            watchdog(int(sys.argv[2]), artifact, task_name, receipt)
        except BaseException:
            receipt['error'] = traceback.format_exc()
        else:
            receipt['ok'] = True
        # Native restoration evidence must survive a blocked scheduler call.
        watchdog_record(watchdog_path(artifact, task_name, '.recovery.json'), json.dumps(receipt))
        try:
            delete_watchdog_task(task_name)
        except BaseException:
            receipt['ok'] = False
            receipt['task_cleanup_complete'] = False
            receipt['task_cleanup_error'] = traceback.format_exc()
        else:
            receipt['task_cleanup_complete'] = True
        watchdog_record(result, json.dumps(receipt))
        raise SystemExit(0 if receipt['ok'] else 1)
    else:
        raise SystemExit('Use --probe-high-contrast or --watch-high-contrast PID SNAPSHOT TASK')
    raise SystemExit(0)

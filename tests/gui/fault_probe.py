"""Fault injection cases owned by verify_faults.py; excluded from behavior collection."""
import json
import os
import time

import pytest
from driver import until
import win32con
import win32gui


def test_execution_fault_contract(app, request):
    mode = request.config.getoption('--fault-mode')
    if mode is None:
        pytest.skip('requires --fault-mode')

    def injected():
        (app.output / 'injected-fault.json').write_text(json.dumps({'mode': mode}), encoding='utf-8')

    if mode == 'assertion':
        injected()
        assert False, 'deliberate infrastructure assertion'
    if mode == 'driver-crash':
        injected()
        os._exit(37)
    if mode == 'driver-hang':
        injected()
        while True:
            time.sleep(60)
    if mode == 'application-crash':
        import win32api
        import win32process
        process = win32api.OpenProcess(win32con.PROCESS_TERMINATE, False, app.pid)
        try:
            win32process.TerminateProcess(process, 37)
        finally:
            process.Close()
        until('forced application exit', lambda: not app.is_running())
        injected()
        assert app.is_running(), 'Application unexpectedly exited'
    if mode == 'application-hang':
        import ctypes
        from ctypes import wintypes
        import win32process
        thread_id, _ = win32process.GetWindowThreadProcessId(app.frame)
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenThread.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        thread = kernel.OpenThread(2, False, thread_id)  # THREAD_SUSPEND_RESUME
        assert thread, ctypes.WinError(ctypes.get_last_error())
        try:
            win32process.SuspendThread(thread)
        finally:
            kernel.CloseHandle(thread)
        injected()
        # A real blocked SendMessage, not just a sleeping controller. The
        # pytest-timeout thread must abort and Job closure must remove the app.
        win32gui.SendMessage(app.frame, win32con.WM_NULL, 0, 0)

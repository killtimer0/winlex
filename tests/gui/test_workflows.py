"""Public behavior smoke coverage; no transient loading-window assertions."""
import os
import json
import shutil
import time

import pytest
from driver import until
import win32con
import win32gui


def test_search_read_find_and_exit(app):
    editor = app.open_symbol('CreateFileW')
    text = editor.text_block()
    assert 'CreateFileW' in text and 'HANDLE' in text
    start, end = app.find('CreateFileW')
    # Native selection offsets are UTF-16 units, not Python code point offsets.
    selected = text.replace('\r\n', '\r').encode('utf-16-le')[start * 2:end * 2].decode('utf-16-le')
    assert selected == 'CreateFileW'
    app.close()


def test_history_returns_to_previous_declaration(app):
    app.open_symbol('CreateFileW')
    app.open_symbol('CloseHandle')
    app.command(1055)
    until('history back', lambda: 'CreateFileW' in app.editor().text_block())
    app.command(1056)
    until('history forward', lambda: 'CloseHandle' in app.editor().text_block())


def test_file_dialog_cancel_preserves_search(app):
    app.cancel_file_dialog()
    assert 'CloseHandle' in app.open_symbol('CloseHandle').text_block()


def test_file_dialog_opens_metadata(app, metadata):
    # A different source path must actually be committed; an ignored dialog
    # cannot pass by reusing the catalog loaded through the command line.
    app.command(1021)
    before = app.source_count()
    added = app.output / 'additional 输入.winmd'
    shutil.copyfile(metadata, added)
    app.open_file(added)
    until('additional source committed', lambda: app.status().startswith('Sources:') and app.source_count() == before + 1, 30)
    assert 'CreateFileW' in app.open_symbol('CreateFileW').text_block()


@pytest.mark.real_input
def test_keyboard_search_and_open(app):
    app.keyboard_search('CreateFileW')


def test_execution_fault_contract(app, request):
    mode = request.config.getoption('--fault-mode')
    if mode is None:
        pytest.skip('only selected for runner fault validation')
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

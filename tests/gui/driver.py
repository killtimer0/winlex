"""Winlex public actions using the official AutoItX control API and pywin32."""
import ctypes as c
from ctypes import wintypes as w
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import time

import polling2
import win32api
import win32con
import win32event
import win32gui
import win32process


def until(description, condition, timeout=20):
    try:
        return polling2.poll(condition, step=.05, timeout=timeout,
                             ignore_exceptions=(AssertionError, StopIteration, win32gui.error))
    except polling2.TimeoutException as error:
        raise TimeoutError(description) from error


# PyAutoIt's wheel carries official AutoItX 3.3.12.0 (including XP RTM support).
# Bind the small documented API subset directly: its Python wrapper strips
# whitespace and has pointer-sized return-value defects. Never patch the library.
package = Path(next(iter(importlib.util.find_spec('autoit').submodule_search_locations)))
bits = struct.calcsize('P') * 8
dll = package / 'lib' / ('AutoItX3_x64.dll' if bits == 64 else 'AutoItX3.dll')
expected = {32: '7c860b9ae4549ca2dd81c607bdb641ab2e38895aa5431a549df6c5da75b3dd89',
            64: '1e4c494b81714633a7c067ff2820d1303cd83d9fd66886dab8bbe369e6514830'}
if hashlib.sha256(dll.read_bytes()).hexdigest() != expected[bits]:
    raise RuntimeError('Unexpected AutoItX binary; reinstall the pinned PyAutoIt wheel')
auto = c.WinDLL(str(dll))
for name, result, arguments in [
    ('Init', None, []),
    ('error', c.c_int, []),
    ('ControlGetTextByHandle', None, [w.HWND, w.HWND, w.LPWSTR, c.c_int]),
    ('ControlSetTextByHandle', c.c_int, [w.HWND, w.HWND, w.LPCWSTR]),
    ('ControlListViewByHandle', None, [w.HWND, w.HWND, w.LPCWSTR, w.LPCWSTR, w.LPCWSTR, w.LPWSTR, c.c_int]),
    ('WinActivateByHandle', c.c_int, [w.HWND]),
    ('Send', None, [w.LPCWSTR, c.c_int]),
]:
    function = getattr(auto, 'AU3_' + name)
    function.argtypes, function.restype = arguments, result
auto.AU3_Init()


class Declaration:
    def __init__(self, app, hwnd):
        self.app, self.handle = app, hwnd

    def text_block(self):
        return self.app.text(self.handle)

    def activate(self):
        document = win32gui.GetParent(self.handle)
        while win32gui.GetClassName(win32gui.GetParent(document)) != 'MDIClient':
            document = win32gui.GetParent(document)
            assert document, 'Declaration has no MDI document'
        self.app.record('activate document')
        win32gui.SendMessage(win32gui.GetParent(document), win32con.WM_MDIACTIVATE, document, 0)
        until('document activated', lambda: self.app.document() == document)

    def selection_indices(self):
        # EM_GETSEL is below WM_USER and Windows marshals its two DWORD outputs.
        # Unlike EM_EXGETSEL, it is safe with controller-local pointers.
        start, end = w.DWORD(), w.DWORD()
        send = c.windll.user32.SendMessageW
        send.argtypes = [w.HWND, w.UINT, c.c_size_t, c.c_ssize_t]
        send.restype = c.c_ssize_t
        send(self.handle, win32con.EM_GETSEL, c.addressof(start), c.addressof(end))
        return start.value, end.value


class Winlex:
    def __init__(self, pid, output):
        self.pid, self.output, self.actions = pid, Path(output), []
        self.frame = until('Winlex frame', lambda: next(iter(self.windows('Winlex.Frame')), None), 15)
        (self.output / 'driver.json').write_text(json.dumps(dict(
            backend='AutoItX', version='3.3.12.0', bits=bits, sha256=expected[bits])), encoding='utf-8')

    def windows(self, cls=None):
        found = []
        def visit(hwnd, unused):
            if (win32process.GetWindowThreadProcessId(hwnd)[1] == self.pid
                    and (cls is None or win32gui.GetClassName(hwnd) == cls)):
                found.append(hwnd)
            return True
        win32gui.EnumWindows(visit, None)
        return found

    def record(self, action):
        self.actions.append((time.monotonic(), action))

    def child(self, identifier, parent=None):
        found = []
        def visit(hwnd, unused):
            if win32gui.GetDlgCtrlID(hwnd) == identifier:
                found.append(hwnd)
            return True
        win32gui.EnumChildWindows(parent or self.frame, visit, None)
        return next(iter(found))

    def text(self, hwnd):
        length = win32gui.SendMessage(hwnd, win32con.WM_GETTEXTLENGTH, 0, 0)
        # WM_GETTEXT can expand RichEdit's CR to CRLF; do not truncate or rstrip.
        buffer = c.create_unicode_buffer(max(4, 2 * length + 2))
        auto.AU3_ControlGetTextByHandle(self.frame, hwnd, buffer, len(buffer))
        assert not auto.AU3_error(), 'AutoItX text read failed'
        assert len(buffer.value.encode('utf-16-le')) // 2 < len(buffer) - 1, 'Text read filled its buffer'
        return buffer.value

    def set_text(self, hwnd, text):
        assert auto.AU3_ControlSetTextByHandle(self.frame, hwnd, text), 'AutoItX edit failed'

    def list_command(self, hwnd, command, extra=''):
        result = c.create_unicode_buffer(4096)
        auto.AU3_ControlListViewByHandle(self.frame, hwnd, command, extra, '', result, len(result))
        assert not auto.AU3_error(), 'AutoItX ListView ' + command
        return result.value

    def ready(self):
        until('loaded source available', lambda: self.source_count() > 0, 30)

    def command(self, identifier):
        self.record('command {}'.format(identifier))
        win32gui.PostMessage(self.frame, win32con.WM_COMMAND, identifier, 0)

    def search(self, name):
        self.record('search ' + name)
        win32gui.SendMessage(self.frame, win32con.WM_COMMAND, 1024, 0)
        self.set_text(self.child(1102), name)
        return self.wait_result(name)

    def wait_result(self, name):
        results = self.child(1103)
        def matching():
            for index in range(int(self.list_command(results, 'GetItemCount'))):
                full_name = self.list_command(results, 'GetText', str(index)).split('\r\n', 1)[0]
                if full_name.rsplit('.', 1)[-1] == name:
                    return results, index
            return None
        return until('search result ' + name, matching)

    def open_symbol(self, name):
        results, index = self.search(name)
        self.list_command(results, 'SelectClear')
        self.list_command(results, 'Select', str(index))
        self.command(1050)
        def declaration():
            editor = self.editor()
            assert name in editor.text_block(), 'Waiting for ' + name
            return editor
        return until('declaration ' + name, declaration)

    def document(self):
        found = []
        win32gui.EnumChildWindows(self.frame, lambda h, _: found.append(h)
                                 if win32gui.GetClassName(h) == 'MDIClient' else None, None)
        active = win32gui.SendMessage(next(iter(found)), win32con.WM_MDIGETACTIVE, 0, 0)
        assert active, 'No active document yet'
        return active

    def editor(self):
        return Declaration(self, self.child(1203, self.document()))

    def find(self, text):
        self.command(1060)
        edit = until('find edit', lambda: self.child(1220, self.document()))
        self.set_text(edit, text)
        editor = self.editor()
        until('find selection', lambda: editor.selection_indices()[1] > editor.selection_indices()[0])
        return editor.selection_indices()

    def file_dialog(self):
        self.command(1020)
        return until('file dialog', lambda: next((h for h in self.windows('#32770')
                                                 if win32gui.IsWindowVisible(h)), None), 15)

    def cancel_file_dialog(self):
        dialog = self.file_dialog()
        win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDCANCEL, 0)
        until('file dialog closed', lambda: not win32gui.IsWindow(dialog), 10)

    def open_file(self, path):
        dialog = self.file_dialog()
        combo = self.child(1148, dialog)
        edits = []
        win32gui.EnumChildWindows(combo, lambda h, _: edits.append(h)
                                 if win32gui.GetClassName(h) == 'Edit' else None, None)
        self.set_text(next(iter(edits)), str(path))
        win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDOK, 0)
        until('file dialog closed', lambda: not win32gui.IsWindow(dialog), 30)

    def source_count(self):
        return int(self.list_command(self.child(1112), 'GetItemCount'))

    def source_index(self, path):
        sources = self.child(1112)
        def matching():
            for index in range(int(self.list_command(sources, 'GetItemCount'))):
                text = self.list_command(sources, 'GetText', str(index))
                if Path(text.split('\r\n', 1)[0]) == path:
                    return index + 1
            return None
        return until('source ' + str(path), matching) - 1

    def source_text(self, path):
        return self.list_command(self.child(1112), 'GetText', str(self.source_index(path)))

    def reload_source(self, path):
        self.command(1021)
        sources = self.child(1112)
        index = self.source_index(path)
        self.list_command(sources, 'SelectClear')
        self.list_command(sources, 'Select', str(index))
        until('reload action available', lambda: win32gui.IsWindowEnabled(self.child(1044)))
        self.command(1044)

    def keyboard_search(self, name):
        class GUIThreadInfo(c.Structure):
            _fields_ = [('cbSize', w.DWORD), ('flags', w.DWORD),
                        ('hwndActive', w.HWND), ('hwndFocus', w.HWND),
                        ('hwndCapture', w.HWND), ('hwndMenuOwner', w.HWND),
                        ('hwndMoveSize', w.HWND), ('hwndCaret', w.HWND), ('rcCaret', w.RECT)]
        get_info = c.windll.user32.GetGUIThreadInfo
        get_info.argtypes, get_info.restype = [w.DWORD, c.POINTER(GUIThreadInfo)], w.BOOL
        thread = win32process.GetWindowThreadProcessId(self.frame)[0]
        def focused(hwnd):
            info = GUIThreadInfo(cbSize=c.sizeof(GUIThreadInfo))
            assert get_info(thread, c.byref(info)), 'GetGUIThreadInfo failed'
            return win32gui.GetForegroundWindow() == self.frame and info.hwndFocus == hwnd

        self.record('keyboard search ' + name)
        assert auto.AU3_WinActivateByHandle(self.frame)
        until('application foreground', lambda: win32gui.GetForegroundWindow() == self.frame)
        search = self.child(1102)
        auto.AU3_Send('^l', 0)
        until('search keyboard focus', lambda: focused(search), 5)
        auto.AU3_Send(name, 1)
        # Enter commits an active IME composition without changing input settings;
        # with no composition it invokes the public search action instead.
        auto.AU3_Send('{ENTER}', 0)
        until('typed search text', lambda: self.text(search) == name)
        results, index = self.wait_result(name)
        def tab_to_results():
            if focused(results):
                return True
            auto.AU3_Send('{TAB}', 0)
            return False
        until('results reachable by keyboard', tab_to_results, 5)
        auto.AU3_Send('{HOME}', 0)
        if index:
            auto.AU3_Send('{DOWN ' + str(index) + '}', 0)
        auto.AU3_Send('{ENTER}', 0)
        until('keyboard-opened declaration', lambda: name in self.editor().text_block())

    def is_running(self):
        try:
            handle = win32api.OpenProcess(win32con.SYNCHRONIZE, False, self.pid)
        except win32api.error as error:
            if error.winerror == 87:
                return False
            raise
        try:
            return win32event.WaitForSingleObject(handle, 0) == win32con.WAIT_TIMEOUT
        finally:
            handle.Close()

    def close(self):
        self.record('close')
        win32gui.PostMessage(self.frame, win32con.WM_CLOSE, 0, 0)
        until('process exit', lambda: not self.is_running(), 10)

    def evidence(self, destination):
        destination = Path(destination)
        destination.mkdir(parents=True, exist_ok=True)
        (destination / 'actions.txt').write_text(
            '\n'.join('{:.6f} {}'.format(*action) for action in self.actions), encoding='utf-8')
        try:
            rows = []
            for window in self.windows():
                children = [window]
                win32gui.EnumChildWindows(window, lambda h, _: children.append(h), None)
                rows.extend(dict(hwnd=h, cls=win32gui.GetClassName(h), id=win32gui.GetDlgCtrlID(h),
                                 text=win32gui.GetWindowText(h), rect=win32gui.GetWindowRect(h)) for h in children)
            (destination / 'windows.json').write_text(json.dumps(rows, ensure_ascii=False), encoding='utf-8')
        except Exception as error:
            (destination / 'windows-error.txt').write_text(repr(error), encoding='utf-8')
        try:
            from PIL import ImageGrab
            ImageGrab.grab(bbox=win32gui.GetWindowRect(self.frame)).save(str(destination / 'screen.png'))
        except Exception as error:
            (destination / 'screen-error.txt').write_text(repr(error), encoding='utf-8')

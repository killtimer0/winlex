"""Pointer-sized Win32 bindings with bounded cross-process messaging."""
import ctypes as c
from ctypes import wintypes as w
import time
from contextlib import contextmanager
import win32con

# Scan code 1, repeat count 1, previous state down, transition bit set.
KEY_UP_LPARAM = 0xC0000001

user = c.WinDLL("user32", use_last_error=True)
kernel = c.WinDLL("kernel32", use_last_error=True)


def api(dll, name, result, *args):
    function = getattr(dll, name)
    function.restype, function.argtypes = result, args
    return function


callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
enum_windows = api(user, "EnumWindows", w.BOOL, callback, w.LPARAM)
enum_children = api(user, "EnumChildWindows", w.BOOL, w.HWND, callback, w.LPARAM)
get_pid = api(user, "GetWindowThreadProcessId", w.DWORD, w.HWND, c.POINTER(w.DWORD))
get_class = api(user, "GetClassNameW", c.c_int, w.HWND, w.LPWSTR, c.c_int)
get_rect = api(user, "GetWindowRect", w.BOOL, w.HWND, c.POINTER(w.RECT))
get_client = api(user, "GetClientRect", w.BOOL, w.HWND, c.POINTER(w.RECT))
get_item = api(user, "GetDlgItem", w.HWND, w.HWND, c.c_int)
get_id = api(user, "GetDlgCtrlID", c.c_int, w.HWND)
is_window = api(user, "IsWindow", w.BOOL, w.HWND)
is_visible = api(user, "IsWindowVisible", w.BOOL, w.HWND)
is_zoomed = api(user, "IsZoomed", w.BOOL, w.HWND)
post = api(user, "PostMessageW", w.BOOL, w.HWND, w.UINT, c.c_size_t, c.c_ssize_t)
send_timeout = api(
    user, "SendMessageTimeoutW", c.c_ssize_t,
    w.HWND, w.UINT, c.c_size_t, c.c_ssize_t, w.UINT, w.UINT, c.POINTER(c.c_size_t),
)
set_pos = api(user, "SetWindowPos", w.BOOL, w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT)
close_handle = api(kernel, "CloseHandle", w.BOOL, c.c_void_p)
open_process = api(kernel, "OpenProcess", c.c_void_p, w.DWORD, w.BOOL, w.DWORD)
alloc = api(kernel, "VirtualAllocEx", c.c_void_p, c.c_void_p, c.c_void_p, c.c_size_t, w.DWORD, w.DWORD)
free = api(kernel, "VirtualFreeEx", w.BOOL, c.c_void_p, c.c_void_p, c.c_size_t, w.DWORD)
write_memory = api(kernel, "WriteProcessMemory", w.BOOL, c.c_void_p, c.c_void_p, c.c_void_p, c.c_size_t, c.c_void_p)
read_memory = api(kernel, "ReadProcessMemory", w.BOOL, c.c_void_p, c.c_void_p, c.c_void_p, c.c_size_t, c.c_void_p)


def send(hwnd, message, wp=0, lp=0):
    result = c.c_size_t()
    assert send_timeout(hwnd, message, wp, lp, 3, 3000, c.byref(result)), "Message timeout: %#x" % message
    return c.c_ssize_t(result.value).value


def window_text(hwnd):
    buffer = c.create_unicode_buffer(send(hwnd, win32con.WM_GETTEXTLENGTH) + 1)
    send(hwnd, win32con.WM_GETTEXT, len(buffer), c.addressof(buffer))
    return buffer.value


def set_text(hwnd, value):
    buffer = c.create_unicode_buffer(value)
    send(hwnd, win32con.WM_SETTEXT, 0, c.addressof(buffer))


def key_message(hwnd, code):
    assert post(hwnd, win32con.WM_KEYDOWN, code, 1)
    assert post(hwnd, win32con.WM_KEYUP, code, KEY_UP_LPARAM) or not is_window(hwnd)


def windows(parent=None):
    result = []
    fn = callback(lambda hwnd, _: result.append(hwnd) or True)
    if parent:
        enum_children(parent, fn, 0)
    else:
        enum_windows(fn, 0)
    return result


def class_name(hwnd):
    buffer = c.create_unicode_buffer(256)
    get_class(hwnd, buffer, len(buffer))
    return buffer.value


def rectangle(hwnd, client=False):
    rect = w.RECT()
    assert (get_client if client else get_rect)(hwnd, c.byref(rect))
    return rect.left, rect.top, rect.right, rect.bottom


@contextmanager
def window_dpi_context(hwnd):
    """Use the target's coordinate space for control geometry and mouse messages."""
    previous = None
    if hasattr(user, "GetWindowDpiAwarenessContext") and hasattr(user, "SetThreadDpiAwarenessContext"):
        get_context = api(user, "GetWindowDpiAwarenessContext", c.c_void_p, w.HWND)
        set_context = api(user, "SetThreadDpiAwarenessContext", c.c_void_p, c.c_void_p)
        previous = set_context(get_context(hwnd))
        assert previous, "Cannot match target DPI context"
    try:
        yield
    finally:
        if previous:
            assert set_context(previous), "Cannot restore test DPI context"


def wait(probe, description, timeout=15):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        value = probe()
        if value:
            return value
        time.sleep(0.025)
    raise AssertionError("Timeout: " + description)


def monitors():
    values = []
    kind = c.WINFUNCTYPE(w.BOOL, c.c_void_p, c.c_void_p, c.POINTER(w.RECT), w.LPARAM)
    def visit(_monitor, _dc, rect, _data):
        r = rect.contents
        values.append((r.left, r.top, r.right, r.bottom))
        return True
    api(user, "EnumDisplayMonitors", w.BOOL, c.c_void_p, c.c_void_p, kind, w.LPARAM)(None, None, kind(visit), 0)
    return values


def capabilities():
    return {"get_dpi_for_window": hasattr(user, "GetDpiForWindow"),
            "thread_dpi_context": hasattr(user, "SetThreadDpiAwarenessContext"),
            "monitors": monitors()}

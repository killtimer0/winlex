"""UIA control queries and native measurements for Windows-specific assertions."""
import ctypes as c
from ctypes import wintypes as w
import time
from contextlib import contextmanager
import uiautomation as auto
import pythoncom
import win32com.client

user = c.WinDLL("user32", use_last_error=True)
kernel = c.WinDLL("kernel32", use_last_error=True)


def api(dll, name, result, *args):
    function = getattr(dll, name)
    function.restype, function.argtypes = result, args
    return function


get_pid = api(user, "GetWindowThreadProcessId", w.DWORD, w.HWND, c.POINTER(w.DWORD))
read_process_memory = api(kernel, "ReadProcessMemory", w.BOOL, w.HANDLE,
                          c.c_void_p, c.c_void_p, c.c_size_t, c.POINTER(c.c_size_t))
write_process_memory = api(kernel, "WriteProcessMemory", w.BOOL, w.HANDLE,
                           c.c_void_p, c.c_void_p, c.c_size_t, c.POINTER(c.c_size_t))
get_class = api(user, "GetClassNameW", c.c_int, w.HWND, w.LPWSTR, c.c_int)
get_rect = api(user, "GetWindowRect", w.BOOL, w.HWND, c.POINTER(w.RECT))
get_client = api(user, "GetClientRect", w.BOOL, w.HWND, c.POINTER(w.RECT))
get_id = api(user, "GetDlgCtrlID", c.c_int, w.HWND)
is_window = api(user, "IsWindow", w.BOOL, w.HWND)
is_visible = api(user, "IsWindowVisible", w.BOOL, w.HWND)
is_zoomed = api(user, "IsZoomed", w.BOOL, w.HWND)
send_timeout = api(
    user, "SendMessageTimeoutW", c.c_ssize_t,
    w.HWND, w.UINT, c.c_size_t, c.c_ssize_t, w.UINT, w.UINT, c.POINTER(c.c_size_t),
)


def send(hwnd, message, wp=0, lp=0):
    result = c.c_size_t()
    assert send_timeout(hwnd, message, wp, lp, 3, 3000, c.byref(result)), "Message timeout: %#x" % message
    return c.c_ssize_t(result.value).value


def window_text(hwnd):
    control = auto.ControlFromHandle(hwnd)
    value = control.GetPattern(auto.PatternId.ValuePattern)
    if value is not None:
        return value.Value
    text = control.GetPattern(auto.PatternId.TextPattern)
    return text.DocumentRange.GetText(-1) if text is not None else control.Name


def rich_edit_document(hwnd):
    """Native TOM object for font, glyph and full-width selection measurements."""
    token = send(hwnd, 0x003D, 0, -16)  # WM_GETOBJECT, OBJID_NATIVEOM
    return win32com.client.Dispatch(pythoncom.ObjectFromLresult(token, pythoncom.IID_IDispatch, 0))


def rich_edit_text(hwnd):
    """Complete text for UTF-16 and full-render measurements, excluding TOM's final paragraph mark."""
    document = rich_edit_document(hwnd)
    return document.Range(0, document.Range(0, 0).StoryLength - 1).Text


def get_item(hwnd, identifier):
    return next((control.NativeWindowHandle for control, _ in auto.WalkControl(auto.ControlFromHandle(hwnd))
                 if control.AutomationId == str(identifier) and control.NativeWindowHandle), None)


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
    """Use the target's coordinate space for native geometry measurements."""
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

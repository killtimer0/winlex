"""UIA control queries and native measurements for Windows-specific assertions."""
import ctypes as c
from ctypes import wintypes as w
import time
from contextlib import contextmanager
import uiautomation as auto
import pythoncom
import win32com.client
import win32gui
from . import deadline

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


@deadline.operation('dispatch queued native theme messages')
def dispatch_pending_messages():
    """Refresh caller-thread state without synthesizing theme notifications."""
    until = time.monotonic() + 5
    while True:
        assert time.monotonic() < until, 'Controller message queue did not drain before theme read'
        available, message = win32gui.PeekMessage(None, 0, 0, 1)  # PM_REMOVE
        if not available:
            return
        if message[1] == 0x12:  # WM_QUIT
            win32gui.PostQuitMessage(message[2])
            raise AssertionError('Controller quit before native theme read')
        win32gui.TranslateMessage(message)
        win32gui.DispatchMessage(message)


@deadline.operation('read control text via UIA')
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


@deadline.operation('read complete Rich Edit text via TOM')
def rich_edit_text(hwnd):
    """Complete text for UTF-16 and full-render measurements, excluding TOM's final paragraph mark."""
    document = rich_edit_document(hwnd)
    return document.Range(0, document.Range(0, 0).StoryLength - 1).Text


def get_item(hwnd, identifier):
    direct = api(user, 'GetDlgItem', w.HWND, w.HWND, c.c_int)(hwnd, identifier)
    if direct:
        return direct
    found = []
    win32gui.EnumChildWindows(hwnd, lambda child, _: found.append(child)
                             if get_id(child) == identifier else None, None)
    return next(iter(found), None)


def top_level_windows():
    found = []
    try:
        win32gui.EnumWindows(lambda hwnd, _: found.append(hwnd) or True, None)
    except win32gui.error as error:
        # Windows 10 19045 returns FALSE with last-error zero on an empty
        # isolated desktop; pywin32 turns that empty result into error(0).
        if error.winerror or found:
            raise
    return found


def find_window(pid, window_class, owner=None):
    found = []

    def visit(hwnd, _):
        process = w.DWORD()
        get_pid(hwnd, c.byref(process))
        if (process.value == pid and class_name(hwnd) == window_class and is_visible(hwnd)
                and (owner is None or win32gui.GetWindow(hwnd, 4) == owner)):
            found.append(hwnd)

    for hwnd in top_level_windows():
        visit(hwnd, None)
    assert len(found) <= 1, ('ambiguous native window', pid, window_class, found)
    return next(iter(found), None)


def class_name(hwnd):
    buffer = c.create_unicode_buffer(256)
    get_class(hwnd, buffer, len(buffer))
    return buffer.value


def rectangle(hwnd, client=False):
    rect = w.RECT()
    assert (get_client if client else get_rect)(hwnd, c.byref(rect))
    return rect.left, rect.top, rect.right, rect.bottom


def titlebar_rectangle(hwnd):
    """Get the non-client caption without walking virtual list items via UIA."""
    class TitleBarInfo(c.Structure):
        _fields_ = [('size', w.DWORD), ('rect', w.RECT), ('states', w.DWORD * 6)]

    info = TitleBarInfo()
    info.size = c.sizeof(info)
    get_info = api(user, 'GetTitleBarInfo', w.BOOL, w.HWND, c.POINTER(TitleBarInfo))
    assert get_info(hwnd, c.byref(info)), 'Cannot get title bar bounds'
    rect = info.rect
    return rect.left, rect.top, rect.right, rect.bottom


def physical_screen_point(hwnd, point):
    """Convert a point measured in the target's DPI space for real input."""
    if not hasattr(user, "LogicalToPhysicalPointForPerMonitorDPI"):
        return point
    value = w.POINT(*point)
    convert = api(user, "LogicalToPhysicalPointForPerMonitorDPI", w.BOOL,
                  w.HWND, c.POINTER(w.POINT))
    assert convert(hwnd, c.byref(value)), "Cannot convert target point to physical pixels"
    return value.x, value.y


@contextmanager
def physical_dpi_context():
    """Use physical screen pixels for capture and input on modern Windows."""
    previous = None
    if hasattr(user, "SetThreadDpiAwarenessContext"):
        set_context = api(user, "SetThreadDpiAwarenessContext", c.c_void_p, c.c_void_p)
        previous = set_context(-4)  # DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
        assert previous, "Cannot enter physical screen coordinate space"
    try:
        yield
    finally:
        if previous:
            assert set_context(previous), "Cannot restore test DPI context"


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
    with deadline.operation(description):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            deadline.progress()
            value = probe()
            if time.monotonic() >= until:
                raise AssertionError('Timeout: ' + description)
            if value:
                return value
            time.sleep(min(0.025, max(0, until - time.monotonic())))
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

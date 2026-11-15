"""Context-menu gestures, native tracking, dismissal and clipboard results."""
import time
import win32clipboard as clipboard
import win32con
import win32gui
import uiautomation as auto
from support import messages
from support import native as n
from support.session import ThreadInfo


def popup(app, editor, location=-1, keyboard=False, mouse=False, protocol_mouse=False):
    if mouse:
        auto.RightClick(n.c.c_short(location & 0xFFFF).value,
                        n.c.c_short((location >> 16) & 0xFFFF).value, waitTime=0)
    elif protocol_mouse:
        point = n.w.POINT(n.c.c_short(location & 0xFFFF).value,
                          n.c.c_short((location >> 16) & 0xFFFF).value)
        assert n.api(n.user, "ScreenToClient", n.w.BOOL, n.w.HWND,
                     n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
        local = (point.x & 0xffff) | ((point.y & 0xffff) << 16)
        win32gui.PostMessage(editor, win32con.WM_RBUTTONDOWN, win32con.MK_RBUTTON, local)
        win32gui.PostMessage(editor, win32con.WM_RBUTTONUP, 0, local)
    else:
        app.focus(editor)
        win32gui.PostMessage(editor, win32con.WM_CONTEXTMENU, editor, -1 if keyboard else location)

    return find_popup(app)


def popup_window(pid):
    windows = []

    def matches(window):
        owner = n.w.DWORD()
        n.get_pid(window, n.c.byref(owner))
        return owner.value == pid and n.is_visible(window) and n.class_name(window) == "#32768"

    def collect(window, _):
        if matches(window):
            windows.append(window)
        return True

    win32gui.EnumWindows(collect, None)
    return next((window for window in windows if matches(window)), None)


def find_popup(app):
    window = n.wait(lambda: popup_window(app.process.pid), "context menu", 15)
    menu = n.send(window, messages.MN_GETHMENU)
    assert menu, "context menu handle"
    info = ThreadInfo()
    info.size = n.c.sizeof(info)
    thread = n.get_pid(app.frame, None)
    get_info = n.api(n.user, "GetGUIThreadInfo", n.w.BOOL, n.w.DWORD,
                     n.c.POINTER(ThreadInfo))
    n.wait(lambda: get_info(thread, n.c.byref(info)) and info.menu != 0,
           "context menu tracking", 10)
    count = n.api(n.user, "GetMenuItemCount", n.c.c_int, n.w.HMENU)(menu)
    assert count >= 0, "context menu item count"
    get_id = n.api(n.user, "GetMenuItemID", n.w.UINT, n.w.HMENU, n.c.c_int)
    return menu, [get_id(menu, i) for i in range(count)]


def dismiss(editor):
    win32gui.PostMessage(editor, win32con.WM_CANCELMODE, 0, 0)
    wait_closed(editor)


def wait_closed(editor):
    pid = n.w.DWORD()
    n.get_pid(editor, n.c.byref(pid))
    n.wait(lambda: not popup_window(pid.value), "context menu closed")


def menu_key(window, key):
    win32gui.PostMessage(window, win32con.WM_KEYDOWN, key, 0)
    if win32gui.IsWindow(window):
        win32gui.PostMessage(window, win32con.WM_KEYUP, key, 0)


def wait_clipboard_available():
    """Let any other clipboard owner finish before the app starts its copy."""
    def available():
        try:
            clipboard.OpenClipboard()
        except Exception:
            return False
        clipboard.CloseClipboard()
        return True
    n.wait(available, "clipboard available")


def clipboard_text():
    """Read the copied text; another process may hold the clipboard briefly."""
    for _ in range(50):
        try:
            clipboard.OpenClipboard()
        except Exception:
            time.sleep(0.02)
            continue
        try:
            return (
                clipboard.GetClipboardData(win32con.CF_UNICODETEXT)
                if clipboard.IsClipboardFormatAvailable(win32con.CF_UNICODETEXT) else None
            )
        finally:
            clipboard.CloseClipboard()
    return None

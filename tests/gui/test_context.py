"""Context menus use native popup loops, preserving the declaration selection."""
import struct
import time
import win32clipboard as clipboard
import win32con
from support import messages
from support import native as n


def popup(app, editor, location=-1, keyboard=False, mouse=False):
    if keyboard:
        n.key_message(editor, win32con.VK_APPS)
    elif mouse:
        point = n.w.POINT(n.c.c_short(location & 0xFFFF).value, n.c.c_short((location >> 16) & 0xFFFF).value)
        assert n.api(n.user, "ScreenToClient", n.w.BOOL, n.w.HWND, n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
        local = (point.x & 0xFFFF) | ((point.y & 0xFFFF) << 16)
        assert n.post(editor, win32con.WM_RBUTTONDOWN, 2, local)
        assert n.post(editor, win32con.WM_RBUTTONUP, 0, local)
    else:
        assert n.post(editor, win32con.WM_CONTEXTMENU, editor, location)

    def find():
        for hwnd in n.windows():
            pid = n.w.DWORD()
            n.get_pid(hwnd, n.c.byref(pid))
            if pid.value == app.process.pid and n.class_name(hwnd) == "#32768":
                return hwnd
    window = n.wait(find, "context menu")
    menu = n.send(window, messages.MN_GETHMENU)
    count = n.api(n.user, "GetMenuItemCount", n.c.c_int, n.w.HMENU)(menu)
    get_id = n.api(n.user, "GetMenuItemID", n.w.UINT, n.w.HMENU, n.c.c_int)
    return menu, [get_id(menu, i) for i in range(count)]


def dismiss(editor):
    n.key_message(editor, win32con.VK_ESCAPE)
    wait_closed(editor)


def wait_closed(editor):
    pid = n.w.DWORD()
    n.get_pid(editor, n.c.byref(pid))
    def closed():
        for hwnd in n.windows():
            owner = n.w.DWORD()
            n.get_pid(hwnd, n.c.byref(owner))
            if owner.value == pid.value and n.class_name(hwnd) == "#32768":
                return False
        return True
    n.wait(closed, "context menu closed")


def character_point(app, editor, index):
    raw = bytes(8)
    assert n.write_memory(app.handle, app.memory, raw, len(raw), None)
    n.send(editor, win32con.EM_POSFROMCHAR, app.memory, index)
    value = n.c.create_string_buffer(8)
    assert n.read_memory(app.handle, app.memory, value, 8, None)
    x, y = struct.unpack("<ii", value.raw)
    point = n.w.POINT(x + 1, y + 1)
    assert n.api(n.user, "ClientToScreen", n.w.BOOL, n.w.HWND, n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
    return (point.x & 0xFFFF) | ((point.y & 0xFFFF) << 16)


def test_numeric_context_and_selection_survive_architecture_changes(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "24(0x18) bytes" in app.content(doc), "layout", 45)
    editor = n.get_item(doc, 1203)
    index = app.content(doc).replace("\r\n", "\r").index("24(0x18)")
    n.send(editor, win32con.EM_SETSEL, index, index + 2)
    menu, items = popup(app, editor, keyboard=True)
    assert items == [2071, 2072, 2073, 0, 2074]
    get_state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)
    n.key_message(editor, win32con.VK_DOWN)  # Down reaches the popup loop, not the editor
    n.wait(lambda: any(get_state(menu, i, win32con.MF_BYPOSITION) & win32con.MF_HILITE for i in range(len(items))), "menu keyboard navigation")
    dismiss(editor)
    n.send(editor, win32con.WM_NULL)  # wait for the popup loop to finish
    assert n.send(editor, win32con.EM_GETSEL) == ((index + 2) << 16) | index

    # Mouse context uses the hit character, not the existing selection.
    n.send(editor, win32con.EM_SETSEL, 0, 2)
    with n.window_dpi_context(app.frame):
        location = character_point(app, editor, index)
        _, items = popup(app, editor, location, mouse=True)
        assert 2072 in items
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
    assert n.send(editor, win32con.EM_GETSEL) == (2 << 16)

    n.send(editor, win32con.EM_SETSEL, 0, 0)
    menu, items = popup(app, editor)
    assert items == [2074]
    state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)(menu, 2074, 0)
    assert state & 3  # disabled without a selection
    dismiss(editor)
    n.send(editor, win32con.WM_NULL)

    app.choose(doc, "x86")
    n.wait(lambda: "12(0xC) bytes" in app.content(doc), "new layout", 45)
    index = app.content(doc).replace("\r\n", "\r").index("12(0xC)")
    n.send(editor, win32con.EM_SETSEL, index, index)
    menu, items = popup(app, editor)
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT, n.w.LPWSTR, n.c.c_int, n.w.UINT)
    buffer = n.c.create_unicode_buffer(256)
    get_label(menu, 2072, buffer, 256, 0)
    assert buffer.value.endswith("\t0xC")
    dismiss(editor)


def clipboard_text():
    """Read the copied text; another process may hold the clipboard briefly."""
    for _ in range(50):
        try:
            clipboard.OpenClipboard()
        except Exception:
            time.sleep(0.02)
            continue
        try:
            return clipboard.GetClipboardData(win32con.CF_UNICODETEXT) if clipboard.IsClipboardFormatAvailable(win32con.CF_UNICODETEXT) else None
        finally:
            clipboard.CloseClipboard()
    return None


def test_numeric_copy_formats(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "24(0x18) bytes" in app.content(doc), "layout", 45)
    editor = n.get_item(doc, 1203)
    index = app.content(doc).replace("\r\n", "\r").index("24(0x18)")
    for down, expected in ((0, "24"), (1, "0x18"), (2, "24(0x18) bytes"), (3, "24")):
        n.send(editor, win32con.EM_SETSEL, index, index + 2)
        popup(app, editor)
        # A fresh popup has no highlight; Down selects the first item.
        for _ in range(down + 1):
            n.key_message(editor, win32con.VK_DOWN)
        n.key_message(editor, win32con.VK_RETURN)
        wait_closed(editor)
        n.send(editor, win32con.WM_NULL)

        n.wait(lambda: clipboard_text() == expected, "copied numeric value")
        assert n.send(editor, win32con.EM_GETSEL) == ((index + 2) << 16) | index

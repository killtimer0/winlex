"""Context menus use native popup loops, preserving the declaration selection."""
import struct
import shutil
import subprocess
import sys
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

    return find_popup(app)


def find_popup(app):
    def find():
        for hwnd in n.windows():
            pid = n.w.DWORD()
            n.get_pid(hwnd, n.c.byref(pid))
            if pid.value == app.process.pid and n.class_name(hwnd) == "#32768" and n.is_visible(hwnd):
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
    path = settings[2] / "来源😀.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "24 bytes" in app.content(doc), "layout", 45)
    editor = n.get_item(doc, 1203)
    prefix = app.content(doc).replace("\r\n", "\r").split("24 bytes")[0]
    index = len(prefix.encode("utf-16-le")) // 2
    n.send(editor, win32con.EM_SETSEL, index, index + 2)
    menu, items = popup(app, editor, keyboard=True)
    assert items == [2071, 2072, 2073, 0, 2074]
    get_state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)
    n.key_message(editor, win32con.VK_DOWN)  # Down reaches the popup loop, not the editor
    n.wait(lambda: any(get_state(menu, i, win32con.MF_BYPOSITION) & win32con.MF_HILITE for i in range(len(items))), "menu keyboard navigation")
    dismiss(editor)
    n.send(editor, win32con.WM_NULL)  # wait for the popup loop to finish
    assert n.send(editor, win32con.EM_GETSEL) == ((index + 2) << 16) | index

    # Right-clicking inside the half-open selection preserves it and its copy action.
    with n.window_dpi_context(app.frame):
        location = character_point(app, editor, index + 1)
        menu, items = popup(app, editor, location, mouse=True)
        assert items == [2071, 2072, 2073, 0, 2074]
        assert not get_state(menu, 2074, 0) & 3
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
        assert n.send(editor, win32con.EM_GETSEL) == ((index + 2) << 16) | index

        # The selection end is outside: collapse there before capturing the menu.
        location = character_point(app, editor, index + 2)
        menu, items = popup(app, editor, location, mouse=True)
        assert items == [2071, 2072, 2073, 0, 2074]
        assert get_state(menu, 2074, 0) & 3
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
        assert n.send(editor, win32con.EM_GETSEL) == ((index + 2) << 16) | (index + 2)

    # A mouse click outside the selection moves the caret before choosing context.
    n.send(editor, win32con.EM_SETSEL, 0, 2)
    with n.window_dpi_context(app.frame):
        location = character_point(app, editor, index)
        _, items = popup(app, editor, location, mouse=True)
        assert items == [2071, 2072, 2073, 0, 2074]
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
    assert n.send(editor, win32con.EM_GETSEL) == (index << 16) | index

    n.send(editor, win32con.EM_SETSEL, 0, 0)
    menu, items = popup(app, editor)
    assert items == [2074]
    state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)(menu, 2074, 0)
    assert state & 3  # disabled without a selection
    dismiss(editor)
    n.send(editor, win32con.WM_NULL)

    app.choose(doc, "x86")
    n.wait(lambda: "12 bytes" in app.content(doc), "new layout", 45)
    prefix = app.content(doc).replace("\r\n", "\r").split("12 bytes")[0]
    index = len(prefix.encode("utf-16-le")) // 2
    n.send(editor, win32con.EM_SETSEL, index, index)
    menu, items = popup(app, editor)
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT, n.w.LPWSTR, n.c.c_int, n.w.UINT)
    buffer = n.c.create_unicode_buffer(256)
    get_label(menu, 2072, buffer, 256, 0)
    assert buffer.value.endswith("\t0xC")
    dismiss(editor)


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
            return clipboard.GetClipboardData(win32con.CF_UNICODETEXT) if clipboard.IsClipboardFormatAvailable(win32con.CF_UNICODETEXT) else None
        finally:
            clipboard.CloseClipboard()
    return None


def test_numeric_copy_formats(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "24 bytes" in app.content(doc), "layout", 45)
    editor = n.get_item(doc, 1203)
    index = app.content(doc).replace("\r\n", "\r").index("24 bytes")
    for down, expected in ((0, "24"), (1, "0x18"), (2, "24 bytes"), (3, "24")):
        wait_clipboard_available()
        sequence = clipboard.GetClipboardSequenceNumber()
        n.send(editor, win32con.EM_SETSEL, index, index + 2)
        popup(app, editor)
        # A fresh popup has no highlight; Down selects the first item.
        for _ in range(down + 1):
            n.key_message(editor, win32con.VK_DOWN)
        n.key_message(editor, win32con.VK_RETURN)
        wait_closed(editor)
        n.send(editor, win32con.WM_NULL)

        n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "clipboard changed")
        assert clipboard_text() == expected
        assert n.send(editor, win32con.EM_GETSEL) == ((index + 2) << 16) | index


def test_numeric_context_covers_labels_units_and_constant_annotations(launch, settings):
    app = launch([settings[1]])
    cases = [
        ("Windows.Win32.Security.SECURITY_ATTRIBUTES", "size: 24 bytes", "24", "0x18", True),
        ("Windows.Win32.System.Com.IUnknown", "slot 2", "2", "0x2", False),
        ("Windows.Win32.System.Com.IUnknown", "vtable +0x10", "16", "0x10", True),
        ("STATUS_ACCESS_VIOLATION", "-1073741819", "-1073741819", "0xC0000005", False),
        ("STATUS_ACCESS_VIOLATION", "0xC0000005", "-1073741819", "0xC0000005", False),
        ("SE_DEBUG_NAME", "length: 16 UTF-16 code units", "16", "0x10", True),
    ]
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT, n.w.LPWSTR, n.c.c_int, n.w.UINT)
    for query, region, decimal, hexadecimal, has_unit in cases:
        app.query(query)
        doc = app.open_row()
        app.choose(doc, "x64")
        n.wait(lambda: region in app.content(doc), "numeric context region", 45)
        editor = n.get_item(doc, 1203)
        body = app.content(doc).replace("\r\n", "\r")
        start = len(body[:body.index(region)].encode("utf-16-le")) // 2
        for offset in (0, len(region) // 2, len(region) - 1, len(region)):
            index = start + offset
            n.send(editor, win32con.EM_SETSEL, index, index)
            for mouse in (False, True):
                with n.window_dpi_context(app.frame):
                    location = character_point(app, editor, index) if mouse else -1
                    menu, items = popup(app, editor, location, mouse=mouse)
                    assert items == ([2071, 2072, 2073, 0, 2074] if has_unit else [2071, 2072, 0, 2074])
                    for command, expected in ((2071, decimal), (2072, hexadecimal)):
                        label = n.c.create_unicode_buffer(256)
                        get_label(menu, command, label, 256, 0)
                        assert label.value.endswith("\t" + expected)
                    dismiss(editor)
                    n.send(editor, win32con.WM_NULL)
        for index in (start - 1, start + len(region) + 1):
            n.send(editor, win32con.EM_SETSEL, index, index)
            _, items = popup(app, editor)
            assert items == [2074]
            dismiss(editor)
            n.send(editor, win32con.WM_NULL)
        for down, expected in ((0, decimal), (1, hexadecimal)):
            n.send(editor, win32con.EM_SETSEL, start, start)
            wait_clipboard_available()
            sequence = clipboard.GetClipboardSequenceNumber()
            popup(app, editor)
            for _ in range(down + 1):
                n.key_message(editor, win32con.VK_DOWN)
            n.key_message(editor, win32con.VK_RETURN)
            wait_closed(editor)
            n.send(editor, win32con.WM_NULL)
            n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "numeric value copied")
            assert clipboard_text() == expected


def test_numeric_context_requires_the_whole_selection_in_one_region(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Foundation.RECT")
    doc = app.open_row()
    n.wait(lambda: "offset: 0 bytes, size: 4 bytes" in app.content(doc), "field layout", 45)
    editor = n.get_item(doc, 1203)
    body = app.content(doc).replace("\r\n", "\r")
    start = body.index("offset: 0 bytes")
    end = start + len("offset: 0 bytes")
    for first, last, expected in [
        (start, end, [2071, 2072, 2073, 0, 2074]),
        (start - 1, end, [2074]),
        (start, end + 1, [2074]),
        (start, start + len("offset: 0 bytes, size: 4 bytes"), [2074]),
    ]:
        n.send(editor, win32con.EM_SETSEL, first, last)
        with n.window_dpi_context(app.frame):
            location = character_point(app, editor, start + 1)
            for keyboard in (True, False):
                _, items = popup(app, editor, location, keyboard=keyboard, mouse=not keyboard)
                assert items == expected
                dismiss(editor)
                n.send(editor, win32con.WM_NULL)
                assert n.send(editor, win32con.EM_GETSEL) == (last << 16) | first


def test_numeric_context_uses_updated_caret_after_mouse_right_click(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Foundation.RECT")
    doc = app.open_row()
    n.wait(lambda: "offset: 0 bytes, size: 4 bytes" in app.content(doc), "field layout", 45)
    editor = n.get_item(doc, 1203)
    app.focus(editor)
    body = app.content(doc).replace("\r\n", "\r")
    regions = [(body.index("offset: 0 bytes"), "offset: 0 bytes", "0"),
               (body.index("size: 4 bytes"), "size: 4 bytes", "4")]
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT, n.w.LPWSTR, n.c.c_int, n.w.UINT)
    with n.window_dpi_context(app.frame):
        for start, region, expected in regions:
            opposite = regions[1 if expected == "0" else 0][0]
            location = character_point(app, editor, opposite)
            for offset in (region.index("bytes"), region.index("bytes") + 2):
                left = character_point(app, editor, start + offset)
                right = character_point(app, editor, start + offset + 1)
                x = n.c.c_short(left & 0xFFFF).value - 1
                next_x = n.c.c_short(right & 0xFFFF).value - 1
                y = n.c.c_short((left >> 16) & 0xFFFF).value + 3
                for fraction in (0.25, 0.5, 0.75):
                    point = n.w.POINT(round(x + (next_x - x) * fraction), y)
                    assert n.api(n.user, "ScreenToClient", n.w.BOOL, n.w.HWND, n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
                    local = (point.x & 0xFFFF) | ((point.y & 0xFFFF) << 16)
                    n.send(editor, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, local)
                    n.send(editor, win32con.WM_LBUTTONUP, 0, local)
                    selection = n.send(editor, win32con.EM_GETSEL)
                    assert start <= (selection & 0xFFFF) < start + len(region)
                    assert selection & 0xFFFF == selection >> 16
                    for keyboard in (True, False):
                        menu, items = popup(app, editor, location, keyboard=keyboard, mouse=not keyboard)
                        assert items == [2071, 2072, 2073, 0, 2074]
                        label = n.c.create_unicode_buffer(256)
                        get_label(menu, 2071, label, 256, 0)
                        assert label.value.endswith("\t" + (expected if keyboard else ("4" if expected == "0" else "0")))
                        dismiss(editor)
                        n.send(editor, win32con.WM_NULL)
                        assert n.send(editor, win32con.EM_GETSEL) == (selection if keyboard else (opposite << 16) | opposite)


def test_copy_wrapped_declaration_keeps_original_line_breaks(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Storage.FileSystem.CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    body = app.content(doc).replace("\r\n", "\n").replace("\r", "\n")
    editor = n.get_item(doc, 1203)
    n.send(app.mdi, win32con.WM_MDIRESTORE, doc)
    with n.window_dpi_context(app.frame):
        assert n.set_pos(doc, None, 0, 0, 340, 450,
                         win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
    assert n.send(editor, win32con.EM_GETLINECOUNT) > len(body.splitlines())
    n.send(editor, win32con.EM_SETSEL, 0, -1)
    wait_clipboard_available()
    sequence = clipboard.GetClipboardSequenceNumber()
    _, items = popup(app, editor)
    assert items == [2074]
    n.key_message(editor, win32con.VK_DOWN)
    n.key_message(editor, win32con.VK_RETURN)
    wait_closed(editor)
    n.send(editor, win32con.WM_NULL)
    n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "declaration copied")
    assert clipboard_text() == body.replace("\n", "\r\n")


def test_copy_contention_preserves_clipboard_and_allows_retry(launch, settings, tmp_path):
    app = launch([settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "24 bytes" in app.content(doc), "layout", 45)
    editor = n.get_item(doc, 1203)
    index = app.content(doc).replace("\r\n", "\r").index("24 bytes")
    n.send(editor, win32con.EM_SETSEL, index, index + 2)
    wait_clipboard_available()
    ready = tmp_path / "clipboard-held"
    script = """
import pathlib, sys, win32clipboard as c, win32con
c.OpenClipboard()
try:
    c.EmptyClipboard()
    c.SetClipboardData(win32con.CF_UNICODETEXT, 'contention sentinel')
finally:
    c.CloseClipboard()
c.OpenClipboard()
try:
    pathlib.Path(sys.argv[1]).write_text('ready')
    sys.stdin.readline()
finally:
    c.CloseClipboard()
"""
    holder = subprocess.Popen([sys.executable, "-c", script, str(ready)],
                              stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              text=True)
    try:
        n.wait(lambda: ready.exists() or holder.poll() is not None, "clipboard holder")
        assert ready.exists(), "clipboard holder exited before acquiring the clipboard"
        popup(app, editor)
        n.key_message(editor, win32con.VK_DOWN)
        n.key_message(editor, win32con.VK_RETURN)
        wait_closed(editor)
        n.send(editor, win32con.WM_NULL)
        assert "Open clipboard" in app.status_text()
    finally:
        holder.communicate("release\n", timeout=5)
    assert holder.returncode == 0
    assert clipboard_text() == "contention sentinel"
    sequence = clipboard.GetClipboardSequenceNumber()
    popup(app, editor)
    n.key_message(editor, win32con.VK_DOWN)
    n.key_message(editor, win32con.VK_RETURN)
    wait_closed(editor)
    n.send(editor, win32con.WM_NULL)
    n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "copy after contention")
    assert clipboard_text() == "24"

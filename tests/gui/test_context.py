"""Context menus use native popup loops, preserving the declaration selection."""
import shutil
import subprocess
import sys
import time
import win32clipboard as clipboard
import win32con
import win32gui
import pytest
import uiautomation as auto
from support import messages
from support import native as n


def popup(app, editor, location=-1, keyboard=False, mouse=False, protocol_mouse=False):
    if mouse:
        auto.RightClick(n.c.c_short(location & 0xFFFF).value,
                        n.c.c_short((location >> 16) & 0xFFFF).value)
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


def find_popup(app):
    popup = auto.MenuControl(searchDepth=3, ClassName="#32768", ProcessId=app.process.pid)
    assert popup.Exists(10), "context menu"
    window = popup.NativeWindowHandle
    menu = n.send(window, messages.MN_GETHMENU)
    count = n.api(n.user, "GetMenuItemCount", n.c.c_int, n.w.HMENU)(menu)
    get_id = n.api(n.user, "GetMenuItemID", n.w.UINT, n.w.HMENU, n.c.c_int)
    return menu, [get_id(menu, i) for i in range(count)]


def dismiss(editor):
    win32gui.PostMessage(editor, win32con.WM_CANCELMODE, 0, 0)
    wait_closed(editor)


def wait_closed(editor):
    pid = n.w.DWORD()
    n.get_pid(editor, n.c.byref(pid))
    popup = auto.MenuControl(searchDepth=3, ClassName="#32768", ProcessId=pid.value)
    n.wait(lambda: not popup.Exists(0, 0), "context menu closed")



def selection_packed(editor):
    selection = n.rich_edit_document(editor).Selection
    return (selection.End << 16) | selection.Start


def select_text(editor, first, last):
    document = n.rich_edit_document(editor)
    if last == -1:
        last = document.Range(0, 0).StoryLength - 1
    win32gui.SendMessage(editor, win32con.EM_SETSEL, first, last)
    def selected():
        selection = document.Selection
        return (selection.Start, selection.End) == (first, last)
    try:
        n.wait(selected, "editor selection", 45)
    except AssertionError as error:
        selection = document.Selection
        raise AssertionError("Expected selection %r, observed %r" %
                             ((first, last), (selection.Start, selection.End))) from error

def character_point(app, editor, index):
    x, y = n.rich_edit_document(editor).Range(index, index + 1).GetPoint(32 | 256 | 512)
    point = n.w.POINT(x + 1, y + 1)
    assert n.api(n.user, "ClientToScreen", n.w.BOOL, n.w.HWND,
                 n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
    return (point.x & 0xFFFF) | ((point.y & 0xFFFF) << 16)


@pytest.mark.parametrize('kind', ['declaration', 'tool'])
@pytest.mark.parametrize('gesture', ['mouse', pytest.param('apps', marks=pytest.mark.interactive),
                                    pytest.param('shift-f10', marks=pytest.mark.interactive)])
def test_readonly_result_context_input_preserves_selection(launch, settings, kind, gesture):
    from test_tools import tool, query
    app = launch([settings[1]])
    if kind == 'tool':
        doc = tool(app)
        query(app, doc, '5')
        editor = n.get_item(doc, 1816)
    else:
        app.query('CreateFileW')
        doc = app.open_row()
        n.wait(lambda: 'CreateFileW(' in app.content(doc), 'declaration')
        editor = n.get_item(doc, 1203)
    app.focus(editor)
    select_text(editor, 2, 8)
    selection = selection_packed(editor)
    first = n.send(editor, win32con.EM_GETFIRSTVISIBLELINE)
    if gesture == 'mouse':
        popup(app, editor, character_point(app, editor, 4), protocol_mouse=True)
    elif gesture == 'apps':
        auto.SendKeys('{Apps}')
        find_popup(app)
    else:
        auto.SendKeys('{Shift}{F10}')
        find_popup(app)
    try:
        assert selection_packed(editor) == selection
        assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == first
    finally:
        dismiss(editor)
    assert app.active_document() == doc


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
    select_text(editor, index, index + 2)
    menu, items = popup(app, editor, keyboard=True)
    assert {2071, 2072, 2073, 2074} <= set(items)
    get_state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)
    win32gui.PostMessage(editor, win32con.WM_KEYDOWN, win32con.VK_DOWN, 1)
    win32gui.PostMessage(editor, win32con.WM_KEYUP, win32con.VK_DOWN, -1073741823)
    n.wait(
        lambda: any(get_state(menu, i, win32con.MF_BYPOSITION) & win32con.MF_HILITE
                    for i in range(len(items))),
        "menu keyboard navigation",
    )
    dismiss(editor)
    n.send(editor, win32con.WM_NULL)  # wait for the popup loop to finish
    assert selection_packed(editor) == ((index + 2) << 16) | index

    # Right-clicking inside the half-open selection preserves it and its copy action.
    with n.window_dpi_context(app.frame):
        location = character_point(app, editor, index + 1)
        menu, _ = popup(app, editor, location, protocol_mouse=True)
        assert not get_state(menu, 2074, 0) & 3
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
        assert selection_packed(editor) == ((index + 2) << 16) | index

        # The selection end is outside: collapse there before capturing the menu.
        location = character_point(app, editor, index + 2)
        menu, _ = popup(app, editor, location, protocol_mouse=True)
        assert get_state(menu, 2074, 0) & 3
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
        assert selection_packed(editor) == ((index + 2) << 16) | (index + 2)

    # A mouse click outside the selection moves the caret before choosing context.
    select_text(editor, 0, 2)
    with n.window_dpi_context(app.frame):
        location = character_point(app, editor, index)
        popup(app, editor, location, protocol_mouse=True)
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
    assert selection_packed(editor) == (index << 16) | index

    select_text(editor, 0, 0)
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
    select_text(editor, index, index)
    menu, items = popup(app, editor)
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT, n.w.LPWSTR, n.c.c_int, n.w.UINT)
    buffer = n.c.create_unicode_buffer(256)
    get_label(menu, 2072, buffer, 256, 0)
    assert buffer.value.endswith("\t0xC")
    dismiss(editor)


def test_preview_context_tracks_the_current_declaration(launch, settings):
    app = launch([settings[1]])
    doc = None
    for query, marker, decimal in [
        ("Windows.Win32.Security.SECURITY_ATTRIBUTES", "24 bytes", "24"),
        ("Windows.Win32.Foundation.BOOL", "4 bytes", "4"),
        ("Windows.Win32.Security.SECURITY_ATTRIBUTES", "24 bytes", "24"),
    ]:
        app.query(query)
        current = app.preview_row(0)
        if doc is not None:
            assert current == doc
        doc = current
        app.choose(doc, "x64")
        n.wait(lambda: marker in app.content(doc), "current preview layout", 45)
        editor = n.get_item(doc, 1203)
        prefix = app.content(doc).replace("\r\n", "\r").split(marker)[0]
        index = len(prefix.encode("utf-16-le")) // 2
        select_text(editor, index, index + len(decimal))
        menu, _ = popup(app, editor)
        get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT,
                          n.w.LPWSTR, n.c.c_int, n.w.UINT)
        buffer = n.c.create_unicode_buffer(256)
        get_label(menu, 2071, buffer, 256, 0)
        assert buffer.value.endswith("\t" + decimal)
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
        n.send(app.frame, win32con.WM_SYSCOLORCHANGE)
        menu, _ = popup(app, editor)
        get_label(menu, 2071, buffer, 256, 0)
        assert buffer.value.endswith("\t" + decimal)
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
            return (
                clipboard.GetClipboardData(win32con.CF_UNICODETEXT)
                if clipboard.IsClipboardFormatAvailable(win32con.CF_UNICODETEXT) else None
            )
        finally:
            clipboard.CloseClipboard()
    return None


@pytest.mark.interactive
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
        select_text(editor, index, index + 2)
        popup(app, editor)
        # A fresh popup has no highlight; Down selects the first item.
        for _ in range(down + 1):
            auto.SendKeys("{Down}")
        auto.SendKeys("{Enter}")
        wait_closed(editor)
        n.send(editor, win32con.WM_NULL)

        n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "clipboard changed")
        assert clipboard_text() == expected
        assert selection_packed(editor) == ((index + 2) << 16) | index


def test_numeric_context_covers_labels_units_and_constant_annotations(launch, settings):
    app = launch([settings[1]])
    cases = [
        ("Windows.Win32.Security.SECURITY_ATTRIBUTES", "size: 24 bytes", "24", "0x18", True),
        ("STATUS_ACCESS_VIOLATION", "-1073741819", "-1073741819", "0xC0000005", False),
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
        for offset in (0, len(region)):
            index = start + offset
            select_text(editor, index, index)
            menu, items = popup(app, editor, keyboard=True)
            assert {2071, 2072, 2074} <= set(items)
            assert (2073 in items) == has_unit
            for command, expected in ((2071, decimal), (2072, hexadecimal)):
                label = n.c.create_unicode_buffer(256)
                get_label(menu, command, label, 256, 0)
                assert label.value.endswith("\t" + expected)
            dismiss(editor)
            n.send(editor, win32con.WM_NULL)
        for index in (start - 1, start + len(region) + 1):
            select_text(editor, index, index)
            _, items = popup(app, editor)
            assert items == [2074]
            dismiss(editor)
            n.send(editor, win32con.WM_NULL)


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
        select_text(editor, first, last)
        with n.window_dpi_context(app.frame):
            location = character_point(app, editor, start + 1)
            for keyboard in (True, False):
                _, items = popup(app, editor, location, keyboard=keyboard, protocol_mouse=not keyboard)
                assert items == expected
                dismiss(editor)
                n.send(editor, win32con.WM_NULL)
                assert selection_packed(editor) == (last << 16) | first


def test_numeric_context_uses_updated_caret_after_mouse_right_click(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Foundation.RECT")
    doc = app.open_row()
    n.wait(lambda: "offset: 0 bytes, size: 4 bytes" in app.content(doc), "field layout", 45)
    editor = n.get_item(doc, 1203)
    app.focus(editor)
    body = app.content(doc).replace("\r\n", "\r")
    start = body.index("offset: 0 bytes")
    opposite = body.index("size: 4 bytes")
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT, n.w.LPWSTR, n.c.c_int, n.w.UINT)
    with n.window_dpi_context(app.frame):
        location = character_point(app, editor, start + len("offset: 0 by"))
        point = n.w.POINT(n.c.c_short(location & 0xFFFF).value,
                          n.c.c_short((location >> 16) & 0xFFFF).value)
        assert n.api(n.user, "ScreenToClient", n.w.BOOL, n.w.HWND,
                     n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
        local = (point.x & 0xffff) | ((point.y & 0xffff) << 16)
        n.send(editor, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, local)
        n.send(editor, win32con.WM_LBUTTONUP, 0, local)
        selection = selection_packed(editor)
        assert start <= (selection & 0xFFFF) < start + len("offset: 0 bytes")
        assert selection & 0xFFFF == selection >> 16
        for keyboard, expected in ((True, "0"), (False, "4")):
            location = character_point(app, editor, opposite)
            menu, _ = popup(app, editor, location, keyboard=keyboard, protocol_mouse=not keyboard)
            label = n.c.create_unicode_buffer(256)
            get_label(menu, 2071, label, 256, 0)
            assert label.value.endswith("\t" + expected)
            dismiss(editor)
            n.send(editor, win32con.WM_NULL)
            current = selection_packed(editor)
            if keyboard:
                assert current == selection
            else:
                assert opposite <= (current & 0xFFFF) < opposite + len("size: 4 bytes")
                assert current & 0xFFFF == current >> 16


@pytest.mark.interactive
def test_copy_wrapped_declaration_keeps_original_line_breaks(launch, settings):
    from test_document import resize, window_state

    app = launch([settings[1]])
    app.query("Windows.Win32.Storage.FileSystem.CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    body = app.content(doc).replace("\r\n", "\n").replace("\r", "\n")
    editor = n.get_item(doc, 1203)
    window_state(doc, auto.WindowVisualState.Normal)
    with n.window_dpi_context(app.frame):
        resize(doc, 340, 450)
    assert n.send(editor, win32con.EM_GETLINECOUNT) > len(body.splitlines())
    select_text(editor, 0, -1)
    wait_clipboard_available()
    sequence = clipboard.GetClipboardSequenceNumber()
    _, items = popup(app, editor)
    assert items == [2074]
    auto.SendKeys("{Down}")
    auto.SendKeys("{Enter}")
    wait_closed(editor)
    n.send(editor, win32con.WM_NULL)
    n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "declaration copied")
    assert clipboard_text() == body.replace("\n", "\r\n")


@pytest.mark.interactive
def test_copy_contention_preserves_clipboard_and_allows_retry(launch, settings, tmp_path):
    app = launch([settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "24 bytes" in app.content(doc), "layout", 45)
    editor = n.get_item(doc, 1203)
    index = app.content(doc).replace("\r\n", "\r").index("24 bytes")
    select_text(editor, index, index + 2)
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
        auto.SendKeys("{Down}")
        auto.SendKeys("{Enter}")
        wait_closed(editor)
        n.send(editor, win32con.WM_NULL)
        assert "Open clipboard" in app.status_text()
    finally:
        holder.communicate("release\n", timeout=5)
    assert holder.returncode == 0
    assert clipboard_text() == "contention sentinel"
    sequence = clipboard.GetClipboardSequenceNumber()
    popup(app, editor)
    auto.SendKeys("{Down}")
    auto.SendKeys("{Enter}")
    wait_closed(editor)
    n.send(editor, win32con.WM_NULL)
    n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "copy after contention")
    assert clipboard_text() == "24"

"""Search result menus operate on the selected metadata item."""
import struct
from commctrl import LVM_SETITEMSTATE, LVIS_FOCUSED, LVIS_SELECTED
import pytest
import win32clipboard as clipboard
import win32api
import win32gui
import win32con
import win32process
import uiautomation as auto

from support import native as n
from support.session import select_combo
from test_context import clipboard_text, dismiss, popup, popup_window, wait_clipboard_available, wait_closed


def select_result(app, row):
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_WRITE,
                                  False, app.process.pid)
    memory = None
    try:
        memory = win32process.VirtualAllocEx(process, 0, 128, win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        item = bytearray(128)
        state = LVIS_FOCUSED | LVIS_SELECTED
        struct.pack_into('<IIIII', item, 0, 0, 0, 0, 0, state)
        buffer = n.c.create_string_buffer(bytes(item))
        assert n.write_process_memory(int(process), memory, buffer, len(item), None)
        n.send(app.results, LVM_SETITEMSTATE, -1, memory)
        struct.pack_into('<I', item, 12, state)
        buffer = n.c.create_string_buffer(bytes(item))
        assert n.write_process_memory(int(process), memory, buffer, len(item), None)
        assert n.send(app.results, LVM_SETITEMSTATE, row, memory)
        assert app.selected_row() == row
    finally:
        try:
            if memory is not None:
                win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        finally:
            win32api.CloseHandle(process)


def result(app, name):
    if not n.is_visible(app.results):
        app.command(1040)
    app.query(name)
    row = app.named_rows(name)[0]
    select_result(app, row)
    app.focus(app.results)
    return row


def choose(app, ids, command):
    actions = [ident for ident in ids if ident and ident != 0xffffffff]
    for code in [win32con.VK_DOWN] * (actions.index(command) + 1) + [win32con.VK_RETURN]:
        win32gui.PostMessage(app.results, win32con.WM_KEYDOWN, code, 1)
        win32gui.PostMessage(app.results, win32con.WM_KEYUP, code, -1073741823)
    wait_closed(app.results)
    n.send(app.results, win32con.WM_NULL)


@pytest.mark.parametrize("query,condition,expected", [
    ("ERROR_ACCESS_DENIED", False, {1, 2, 3, 4, 5}),
    ("CreateFileW", False, {1, 2, 3}),
    ('guid = "0000000c-0000-0000-c000-000000000046"', True, {1, 2, 3, 4}),
], ids=["integer", "method", "guid"])
def test_result_menu_actions_available_by_result_kind(launch, settings, query, condition, expected):
    app = launch([settings[1]])
    if condition:
        mode = n.get_item(app.frame, 1114)
        select_combo(mode, app.choices(mode)[1])
        app.query(query)
        assert app.count() > 0
        app.select(0)
        app.focus(app.results)
    else:
        result(app, query)
    _, ids = popup(app, app.results, keyboard=True)
    try:
        assert expected <= set(ids)
        for command in [4, 5]:
            if command not in expected:
                assert command not in ids
    finally:
        dismiss(app.results)


def test_search_entry_enter_does_not_open_selected_result(launch, settings):
    app = launch([settings[1]])
    result(app, "CreateFileW")
    app.focus(app.search)
    win32gui.PostMessage(app.search, win32con.WM_KEYDOWN, win32con.VK_RETURN, 0)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "explicit search completed", 45)
    assert not app.documents()
    assert app.focused() == app.search


@pytest.mark.interactive
@pytest.mark.parametrize("name,command,expected", [
    ("ERROR_ACCESS_DENIED", 3, None),
    ("ERROR_ACCESS_DENIED", 4, "5"),
    ("ERROR_ACCESS_DENIED", 5, "0x00000005"),
    ("E_FAIL", 4, "-2147467259"),
    ("E_FAIL", 5, "0x80004005"),
])
def test_result_menu_copy(launch, settings, name, command, expected):
    app = launch([settings[1]])
    row = result(app, name)
    if expected is None:
        expected = app.cell(row).splitlines()[0]
    wait_clipboard_available()
    sequence = clipboard.GetClipboardSequenceNumber()
    _, ids = popup(app, app.results, keyboard=True)
    choose(app, ids, command)
    n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "result copied")
    assert clipboard_text() == expected


@pytest.mark.interactive
def test_result_menu_captures_value_before_search_refresh(launch, settings):
    app = launch([settings[1]])
    result(app, "ERROR_ACCESS_DENIED")
    wait_clipboard_available()
    sequence = clipboard.GetClipboardSequenceNumber()
    _, ids = popup(app, app.results, keyboard=True)
    value = n.c.create_unicode_buffer('CreateFileW')
    assert n.send(app.search, win32con.WM_SETTEXT, 0, n.c.addressof(value))
    n.wait(lambda: app.count() == 0, "old results cleared during menu")
    assert popup_window(app.process.pid), "result menu closed during search update"
    choose(app, ids, 4)
    n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "captured result copied")
    assert clipboard_text() == "5"


def test_result_right_click_preserves_document_and_new_window_duplicates(launch, settings):
    app = launch([settings[1]])
    result(app, "CreateFileW")
    original = app.open_row(app.selected_row())
    n.wait(lambda: "CreateFileW(" in app.content(original), "original declaration", 45)
    content = app.content(original)
    row = result(app, "ERROR_ACCESS_DENIED")
    with n.window_dpi_context(app.frame):
        left, top, right, bottom = app.item_rect(row)
        x, y = win32gui.ClientToScreen(app.results, (left + 8, (top + bottom) // 2))
        _, ids = popup(app, app.results, location=(x & 0xffff) | ((y & 0xffff) << 16), protocol_mouse=True)
    assert app.active_document() == original
    assert app.content(original) == content
    choose(app, ids, 1)
    opened = app.wait_document("result opened", "ERROR_ACCESS_DENIED")
    assert opened != original
    app.focus(app.results)
    _, ids = popup(app, app.results, keyboard=True)
    choose(app, ids, 2)
    duplicate = n.wait(lambda: app.active_document() if app.active_document() not in (original, opened)
                       and app.active_document() in app.documents() else None, "duplicate document")
    assert duplicate != opened
    assert original in app.documents() and app.content(original) == content

"""Result actions capture the chosen item independently of search refresh."""
import pytest
import win32clipboard as clipboard
import win32gui
import win32con

from support import native as n
from support.session import select_combo
from driver.menus import clipboard_text, dismiss, popup, popup_window, wait_clipboard_available
from driver.results import choose, result


@pytest.mark.parametrize("query,condition,expected", [
    ("ERROR_ACCESS_DENIED", False, {1, 2, 3, 4, 5, 100}),
    ("CreateFileW", False, {1, 2, 3}),
    ('guid = "0000000c-0000-0000-c000-000000000046"', True, {1, 2, 3, 4, 100}),
], ids=["integer", "method", "guid"])
def test_result_menu_actions_available_by_result_kind(launch, settings, query, condition, expected):
    app = launch([settings[1]])
    if condition:
        mode = n.get_item(app.frame, 1114)
        select_combo(mode, app.choices(mode)[1])
        app.query(query)
        app.select(0)
    else:
        result(app, query)
    _, ids = popup(app, app.results, keyboard=True)
    try:
        assert expected <= set(ids)
        for command in [4, 5, 100]:
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
    _, ids = popup(app, app.results, keyboard=True)
    choose(app, ids, 2)
    n.wait(lambda: doc if (doc := app.active_document()) not in (original, opened)
           and doc in app.documents() else None, "duplicate document")
    assert original in app.documents() and app.content(original) == content

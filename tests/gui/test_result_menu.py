"""Search result menus operate on the selected metadata item."""
import pytest
import win32clipboard as clipboard
import win32con
import win32gui

from support import native as n
from test_context import clipboard_text, dismiss, popup, wait_clipboard_available, wait_closed


def result(app, name):
    app.command(1040)
    app.query(name)
    row = app.named_rows(name)[0]
    app.select(row)
    app.focus(app.results)
    return row


def menu_items(menu, ids):
    get_label = n.api(n.user, "GetMenuStringW", n.c.c_int, n.w.HMENU, n.w.UINT,
                      n.w.LPWSTR, n.c.c_int, n.w.UINT)
    labels = {}
    for ident in ids:
        if ident and ident != 0xffffffff:
            text = n.c.create_unicode_buffer(256)
            get_label(menu, ident, text, len(text), 0)
            labels[ident] = text.value
    return labels


def choose(app, ids, command):
    actions = [ident for ident in ids if ident and ident != 0xffffffff]
    for _ in range(actions.index(command) + 1):
        n.key_message(app.results, win32con.VK_DOWN)
    n.key_message(app.results, win32con.VK_RETURN)
    wait_closed(app.results)
    n.send(app.results, win32con.WM_NULL)


def test_result_menu_shapes_and_keyboard(launch, settings, request):
    app = launch([settings[1]])
    for name, expected in [("ERROR_ACCESS_DENIED", [1, 2, 3, 4, 5]),
                           ("CreateFileW", [1, 2, 3])]:
        result(app, name)
        menu, ids = popup(app, app.results, keyboard=True)
        try:
            labels = {ident: label for ident, label in menu_items(menu, ids).items() if ident < 100}
            assert list(labels) == expected
            if request.config.getoption("--language") == "en-US":
                names = {1: "Open", 2: "Open in New Window", 3: "Copy Full Name",
                         4: "Copy Value", 5: "Copy Hexadecimal Value"}
                assert labels == {ident: names[ident] for ident in expected}
        finally:
            dismiss(app.results)


def test_guid_result_menu_has_value_without_hexadecimal(launch, settings):
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    n.send(mode, win32con.CB_SETCURSEL, 1)
    app.command(1114, code=win32con.CBN_SELENDOK)
    app.query('guid = "0000000c-0000-0000-c000-000000000046"')
    assert app.count() > 0
    app.select(0)
    app.focus(app.results)
    menu, ids = popup(app, app.results, keyboard=True)
    try:
        assert list(menu_items(menu, ids)) == [1, 2, 3, 4]
    finally:
        dismiss(app.results)


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
    _, ids = popup(app, app.results, keyboard=True)
    n.set_text(app.search, "CreateFileW")
    n.wait(lambda: app.count() == 0, "old results cleared during menu")
    choose(app, ids, 4)
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
        _, ids = popup(app, app.results, location=(x & 0xffff) | ((y & 0xffff) << 16), mouse=True)
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

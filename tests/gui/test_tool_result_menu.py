"""Tool actions are contributed to metadata result menus by the tool layer."""
import uiautomation as auto
import win32con
import win32gui

from support import native as n
from test_context import dismiss, find_popup, popup, popup_window
from test_result_menu import choose, result
from test_tools import activate, body, choose_scope, query, tool, tools


def test_tool_action_reuses_recent_instance_and_queries_default_scope(launch, settings):
    app = launch([settings[1]])
    first = tool(app)
    second = tool(app, new=True)
    activate(first)
    choose_scope(app, first, 5)
    auto.ControlFromHandle(n.get_item(first, 1813)).GetValuePattern().SetValue(str(settings[2] / "missing.dll"))
    auto.ControlFromHandle(n.get_item(first, 1812)).GetValuePattern().SetValue('0x0804')
    query(app, first, "87")
    result(app, "ERROR_ACCESS_DENIED")
    win32gui.PostMessage(app.results, win32con.WM_CONTEXTMENU, app.results, -1)
    _, ids = find_popup(app)
    assert 100 in ids
    choose(app, ids, 100)
    n.wait(lambda: app.active_document() == first, "recent error code tool activated")
    assert set(tools(app)) == {first, second}
    for identifier in (1811, 1812):
        control = n.get_item(first, identifier)
        assert auto.ControlFromHandle(control).GetValuePattern().Value == app.choices(control)[0]
    assert n.window_text(n.get_item(first, 1810)) == "0x00000005"
    n.wait(lambda: "0x00000005" in body(first), "error code queried immediately", 30)


def test_tool_action_creates_instance_without_replacing_metadata_document(launch, settings):
    app = launch([settings[1]])
    result(app, "CreateFileW")
    original = app.open_row(app.selected_row())
    result(app, "E_FAIL")
    win32gui.PostMessage(app.results, win32con.WM_CONTEXTMENU, app.results, -1)
    _, ids = find_popup(app)
    assert not tools(app)
    choose(app, ids, 100)
    doc = n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                 else None, "error code tool created")
    n.wait(lambda: "0x80004005" in body(doc), "signed HRESULT queried", 30)
    assert original in app.documents()


def test_tool_action_captures_code_before_search_refresh(launch, settings):
    app = launch([settings[1]])
    result(app, "ERROR_ACCESS_DENIED")
    _, ids = popup(app, app.results, keyboard=True)
    assert 100 in ids
    dismiss(app.results)
    assert not tools(app), "cancelling an action must not create a tool"
    _, ids = popup(app, app.results, keyboard=True)
    value = n.c.create_unicode_buffer('WinlexNoSuchSymbolActionSnapshot')
    assert n.send(app.search, win32con.WM_SETTEXT, 0, n.c.addressof(value))
    n.wait(lambda: app.count() == 0, "source results cleared while menu is open")
    assert popup_window(app.process.pid), "search refresh must leave the menu open"
    choose(app, ids, 100)
    doc = n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                 else None, "captured action opens tool")
    assert n.window_text(n.get_item(doc, 1810)) == "0x00000005"
    n.wait(lambda: "0x00000005" in body(doc), "captured code queried after row disappeared", 30)

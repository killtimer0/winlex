"""Tool actions are contributed to metadata result menus by the tool layer."""
import win32con

from support import native as n
from test_context import popup
from test_result_menu import result, menu_items, choose
from test_tools import body, choose_scope, query, tool, tools


def test_tool_action_reuses_recent_instance_and_queries_default_scope(launch, settings, request):
    app = launch([settings[1]])
    first = tool(app)
    second = tool(app, new=True)
    n.send(app.mdi, win32con.WM_MDIACTIVATE, first)
    choose_scope(app, first, 5)
    n.set_text(n.get_item(first, 1813), str(settings[2] / "missing.dll"))
    n.set_text(n.get_item(first, 1812), "0x0804")
    app.command(1812, first, win32con.CBN_EDITCHANGE)
    query(app, first, "87")
    result(app, "ERROR_ACCESS_DENIED")
    menu, ids = popup(app, app.results, keyboard=True)
    assert ids == [1, 2, 0, 3, 4, 5, 0, 100]
    if request.config.getoption("--language") == "en-US":
        assert menu_items(menu, ids)[100] == "Search Error Code"
    choose(app, ids, 100)
    n.wait(lambda: app.active_document() == first, "recent error code tool activated")
    assert set(tools(app)) == {first, second}
    assert n.send(n.get_item(first, 1811), win32con.CB_GETCURSEL) == 0
    assert n.send(n.get_item(first, 1812), win32con.CB_GETCURSEL) == 0
    assert n.window_text(n.get_item(first, 1810)) == "0x00000005"
    n.wait(lambda: "0x00000005" in body(first), "error code queried immediately", 30)


def test_tool_action_creates_instance_without_replacing_metadata_document(launch, settings):
    app = launch([settings[1]])
    result(app, "CreateFileW")
    original = app.open_row(app.selected_row())
    result(app, "E_FAIL")
    _, ids = popup(app, app.results, keyboard=True)
    assert not tools(app)
    choose(app, ids, 100)
    doc = n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                 else None, "error code tool created")
    n.wait(lambda: "0x80004005" in body(doc), "signed HRESULT queried", 30)
    assert original in app.documents()

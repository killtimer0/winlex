"""Tool instances share MDI/history, but own requests and live-only state."""
import win32con
import win32gui
import winreg
import pytest
import uiautomation as auto

from support import native as n
from support.session import invoke, select_combo
from test_history import enabled, history_menu, HISTORY_BACK, HISTORY_FORWARD
from test_theme import selected_index
from test_metadata_guide import resource_text


def tools(app):
    return [item.NativeWindowHandle for item, _ in auto.WalkControl(auto.ControlFromHandle(app.mdi))
            if item.ClassName == 'Winlex.Document' and n.get_item(item.NativeWindowHandle, 1816)]


def initialize_command_menu(app, command):
    menu = win32gui.GetMenu(app.frame)
    n.send(app.frame, win32con.WM_INITMENU, menu)
    for index in range(win32gui.GetMenuItemCount(menu)):
        submenu = win32gui.GetSubMenu(menu, index)
        if submenu and any(win32gui.GetMenuItemID(submenu, position) == command
                           for position in range(win32gui.GetMenuItemCount(submenu))):
            n.send(app.frame, win32con.WM_INITMENUPOPUP, submenu, index)
            return
    raise AssertionError('Menu command %s not found' % command)


def activate(doc):
    mdi = win32gui.GetParent(doc)
    n.send(mdi, win32con.WM_MDIACTIVATE, doc)
    n.wait(lambda: n.send(mdi, win32con.WM_MDIGETACTIVE) == doc, "tool document activated")


def close(doc):
    assert auto.ControlFromHandle(doc).GetWindowPattern().Close()


def visual_state(hwnd, state):
    assert auto.ControlFromHandle(hwnd).GetWindowPattern().SetWindowVisualState(state)


def tool(app, new=False):
    panel = app.tool_list
    if not n.is_visible(panel):
        app.command(1800)
        panel = n.wait(lambda: app.tool_list, "tools panel")
    app.select(0, panel)
    before = set(tools(app))
    app.command(1049 if new else 1050)
    return n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                  and (not new or app.active_document() not in before)
                  else None, "active fixed tool")


def choose_scope(app, doc, index):
    combo = n.get_item(doc, 1811)
    select_combo(combo, app.choices(combo)[index])


def edit(doc, identifier, value):
    control = auto.ControlFromHandle(n.get_item(doc, identifier))
    assert control.GetValuePattern().SetValue(value)


def body(doc):
    return n.rich_edit_text(n.get_item(doc, 1816))


def query(app, doc, value):
    edit(doc, 1810, value)
    invoke(n.get_item(doc, 1815))
    return n.wait(lambda: body(doc) if win32gui.IsWindowEnabled(n.get_item(doc, 1815))
                    and body(doc) else None, "tool explicit query", 30)


def test_tool_completion_is_dispatched_inside_modal_dialog(launch):
    from test_search_help import open_help

    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 1)
    original = query(app, doc, "5")
    edit(doc, 1810, "2")
    dialog = open_help(app)
    try:
        assert not win32gui.IsWindowEnabled(app.frame)
        assert body(doc) == original, 'query must still be pending when modal dialog opens'
        n.send(doc, win32con.WM_COMMAND, 1815)
        result = n.wait(lambda: body(doc) if body(doc) != original else None, 'modal completion', 30)
        assert result != original and "0x00000002" in result
        assert n.is_window(dialog) and not win32gui.IsWindowEnabled(app.frame)
    finally:
        invoke(n.get_item(dialog, win32con.IDOK))
        n.wait(lambda: not n.is_window(dialog), "modal help closed")


def test_tool_completion_is_dispatched_during_menu_tracking(launch):
    from test_context import find_popup, wait_closed

    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 1)
    original = query(app, doc, "5")
    panel = app.tool_list
    edit(doc, 1810, "2")
    win32gui.PostMessage(panel, win32con.WM_CONTEXTMENU, panel, -1)
    menu, _ = find_popup(app)
    try:
        assert body(doc) == original, 'query must still be pending when menu opens'
        n.send(doc, win32con.WM_COMMAND, 1815)
        result = n.wait(lambda: body(doc) if body(doc) != original else None, 'menu completion', 30)
        assert result != original and "0x00000002" in result
        assert find_popup(app)[0] == menu
    finally:
        popup = auto.MenuControl(searchDepth=3, ClassName='#32768', ProcessId=app.process.pid)
        win32gui.PostMessage(popup.NativeWindowHandle, win32con.WM_KEYDOWN, win32con.VK_ESCAPE, 1)
        wait_closed(panel)


def test_tool_list_context_menu_offers_instance_actions_without_opening(launch):
    from test_context import find_popup, wait_closed

    app = launch()
    app.command(1800)
    panel = app.tool_list
    app.select(0, panel)
    app.row(0, panel).GetSelectionItemPattern().RemoveFromSelection()
    assert app.selected_row(panel) == -1
    for command in (1049, 1050):
        app.command(command)
        assert not tools(app), "opening without a selected tool must be harmless"
    app.select(0, panel)
    app.focus(panel)
    win32gui.PostMessage(panel, win32con.WM_CONTEXTMENU, panel, -1)
    _, ids = find_popup(app)
    try:
        assert {1, 2}.issubset(ids)
        assert not tools(app)
    finally:
        popup = auto.MenuControl(searchDepth=3, ClassName='#32768', ProcessId=app.process.pid)
        win32gui.PostMessage(popup.NativeWindowHandle, win32con.WM_KEYDOWN, win32con.VK_ESCAPE, 1)
        wait_closed(panel)


@pytest.mark.parametrize("active_panel", [0, 2])
def test_tool_list_rows_align_after_initialization_and_panel_switches(launch, settings, profile, active_panel):
    profile.set("ActivePanel", winreg.REG_DWORD, active_panel)
    app = launch()
    panel = app.tool_list
    if not n.is_visible(panel):
        app.command(1800)
        panel = n.wait(lambda: app.tool_list, "tools panel")
    original = app.item_rect(0, panel)
    assert original[1] == 0 and original[3] > original[1], original
    app.select(0, panel)
    app.pick([settings[1]])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "source loaded", 45)
    app.command(1040)
    app.query("CreateFileW")
    search = app.item_rect(0)
    app.command(1021)
    source = app.item_rect(0, app.sources)
    assert source[1] == search[1] == 0
    assert source[3] == search[3] == original[3]
    for width, height in [(800, 600), (950, 700)]:
        assert auto.ControlFromHandle(app.frame).GetTransformPattern().Resize(width, height)
        app.command(1800)
        row = app.item_rect(0, panel)
        assert row[1] == 0 and row[3] == original[3], row
        assert app.selected_row(panel) == 0
        n.send(app.frame, win32con.WM_SETTINGCHANGE, 0, 0)
        row = app.item_rect(0, panel)
        assert row[1] == 0 and row[3] == original[3], row
        assert app.selected_row(panel) == 0
        app.command(1021)


def test_tool_host_find_commands_are_disabled_and_harmless(launch):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 1)
    original = query(app, doc, "5")
    assert "0x00000005" in original
    code = n.get_item(doc, 1810)
    edit(doc, 1810, "87")
    app.focus(code)
    status = app.status_text()
    _, index, menu = history_menu(app)
    n.send(app.frame, win32con.WM_INITMENUPOPUP, menu, index)
    app.focus(code)
    for command in [1060, 1061, 1062]:
        assert not enabled(app, command)
        # Disabled find commands must reject even an injected command.
        n.send(app.frame, win32con.WM_COMMAND, command)
        assert app.active_document() == doc
        assert app.focused() == code
        assert n.window_text(code) == "87"
        assert body(doc) == original and app.status_text() == status
        assert not n.get_item(doc, 1220), "host find must not create tool controls"
    assert "0x00000057" in query(app, doc, "87")


def test_tool_errno_language_and_scope_switches_are_explicit(launch, settings, ui_language):
    app = launch()
    doc = tool(app)
    previous = query(app, doc, "2")
    language = n.get_item(doc, 1812)
    choose_scope(app, doc, 4)
    assert body(doc) == previous, "scope initialization must not query"
    assert not win32gui.IsWindowEnabled(language)
    assert '0x0409' in n.window_text(language)
    assert not n.is_visible(n.get_item(doc, 1813))
    result = query(app, doc, "2")
    errno_label = resource_text(settings[0], 2339, ui_language.langid)
    assert errno_label in result and 'msvcrt.dll' in result
    assert 'No such file or directory' in result
    missing = query(app, doc, "15")
    assert resource_text(settings[0], 2324, ui_language.langid) in missing
    choose_scope(app, doc, 5)
    assert win32gui.IsWindowEnabled(language)
    assert n.is_visible(n.get_item(doc, 1813))
    assert body(doc) == missing
    choose_scope(app, doc, 0)
    assert win32gui.IsWindowEnabled(language)
    assert errno_label not in query(app, doc, "2")


def test_tool_compact_layout_and_narrow_fallback(launch):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    with n.window_dpi_context(doc):
        dpi = (n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc)
               if hasattr(n.user, "GetDpiForWindow") else 96)
        scale = lambda value: round(value * dpi / 96)
        visual_state(doc, auto.WindowVisualState.Normal)
        for width, paired in [(900, True), (320, False), (220, False), (900, True)]:
            assert auto.ControlFromHandle(doc).GetTransformPattern().Resize(scale(width), scale(560))
            code, scope, language, module, browse, execute, result, copy = [
                n.rectangle(n.get_item(doc, control))
                for control in [1810, 1811, 1812, 1813, 1814, 1815, 1816, 1818]]
            if paired:
                assert abs(scope[1] - language[1]) <= 1
                assert scope[2] < language[0]
            else:
                assert scope[3] <= language[1]
            assert code[3] <= scope[1]
            assert language[3] <= module[1]
            assert module[2] < browse[0] or module[3] <= browse[1]
            assert max(module[3], browse[3]) <= execute[1]
            assert execute[2] <= copy[0] or execute[3] <= copy[1]
            assert max(execute[3], copy[3]) < result[1] < result[3]
            _, _, client_width, client_height = n.rectangle(doc, client=True)
            left, top = win32gui.ClientToScreen(doc, (0, 0))
            controls = [n.get_item(doc, control) for control in
                        (1810, 1811, 1812, 1813, 1814, 1815, 1816, 1818)]
            controls += [item.NativeWindowHandle for item in auto.ControlFromHandle(doc).GetChildren()
                         if item.ClassName.lower() == "static" and item.NativeWindowHandle
                         and n.is_visible(item.NativeWindowHandle)]
            assert all(control and n.is_visible(control) for control in controls)
            rectangles = [n.rectangle(control) for control in controls]
            for index, rect in enumerate(rectangles):
                assert left <= rect[0] < rect[2] <= left + client_width
                assert top <= rect[1] < rect[3] <= top + client_height
                for other in rectangles[index + 1:]:
                    assert (rect[2] <= other[0] or other[2] <= rect[0]
                            or rect[3] <= other[1] or other[3] <= rect[1])


def test_tool_input_tooltip_registration(launch, settings):
    from support.pe import is_64_bit
    from test_tooltips import registered_tip

    app = launch()
    doc = tool(app)
    code = n.get_item(doc, 1810)
    bits = 64 if is_64_bit(settings[0]) else 32
    handle, hint = registered_tip(app, code, bits)
    assert all(value in hint for value in ('0x', '0o', '0b', '010', '8'))
    close(doc)
    n.wait(lambda: not n.is_window(doc), 'tool closed')
    assert not n.is_window(handle)


def test_shared_status_tracks_active_instance_and_catalog_refresh(launch, settings, ui_language):
    app = launch([settings[1]])
    first = tool(app)
    edit(first, 1810, "invalid")
    invoke(n.get_item(first, 1815))
    invalid = app.status_text()
    assert invalid.startswith(resource_text(settings[0], 2334, ui_language.langid) + ':')
    second = tool(app, new=True)
    ready = app.status_text()
    assert ready and ready != invalid
    activate(first)
    assert app.status_text() == invalid
    activate(second)
    assert app.status_text() == ready
    activate(first)
    edit(first, 1810, "5")
    invoke(n.get_item(first, 1815))
    activate(second)
    assert app.active_document() == second and app.status_text() == ready
    n.wait(lambda: "0x00000005" in body(first), "queried tool result available", 30)
    assert app.active_document() == second and app.status_text() == ready
    app.command(1040)
    assert auto.ControlFromHandle(app.search).GetValuePattern().SetValue("CreateFileW")
    n.wait(lambda: app.count() and app.named_rows("CreateFileW"), "background catalog search", 45)
    assert app.active_document() == second
    search_prefix = resource_text(settings[0], 2025, ui_language.langid).split('%1!s!')[0]
    search_status = n.wait(lambda: app.status_text() if app.status_text().startswith(search_prefix)
                           else None, "search context status", 45)
    activate(first)
    assert not app.status_text()
    close(first)
    assert app.active_document() == second and app.status_text() == ready
    close(second)
    n.wait(lambda: app.status_text() == search_status, "catalog status restored", 45)


def test_search_error_and_document_focus_select_their_own_status(launch, settings, ui_language):
    app = launch([settings[1]])
    doc = tool(app)
    ready = app.status_text()
    app.command(1040)
    mode = n.get_item(app.frame, 1114)
    select_combo(mode, app.choices(mode)[1])
    assert auto.ControlFromHandle(app.search).GetValuePattern().SetValue("value =")
    incomplete = resource_text(settings[0], 2099, ui_language.langid)
    n.wait(lambda: incomplete in app.status_text(), "search error over active tool")
    error = app.status_text()
    assert app.active_document() == doc and error != ready
    app.focus(n.get_item(doc, 1810))
    n.wait(lambda: app.status_text() == ready, "same document focus restores tool status")
    app.focus(app.search)
    n.wait(lambda: app.status_text() == error, "search focus restores query error")


def test_tools_without_metadata_are_fixed_independent_and_reused_by_recency(launch):
    app = launch()
    app.command(1021)
    assert app.count(app.sources) == 0
    app.command(1800)
    panel = n.wait(lambda: app.tool_list, "tools panel")
    app.select(0, panel)
    assert not tools(app), "selection alone must not create content"
    first = tool(app)
    assert all(not value.startswith(("Preview · ", "预览 · "))
               for value in app.tab_descriptions())
    choose_scope(app, first, 1)
    initial = query(app, first, "5")
    assert "0x00000005" in initial
    assert tool(app) == first and body(first) == initial
    second = tool(app, new=True)
    assert second != first
    assert not n.window_text(n.get_item(second, 1810)) and not body(second)
    assert selected_index(app, n.get_item(second, 1811)) == 0
    assert tool(app) == second
    activate(first)
    assert tool(app) == first, "normal open chooses most recently visited live instance"
    assert body(first) == initial and not body(second)
    close(first)
    n.wait(lambda: first not in tools(app), "tool closed")
    assert tool(app) == second
    close(second)
    n.wait(lambda: not tools(app), "all tools closed")
    fresh = tool(app)
    assert not n.window_text(n.get_item(fresh, 1810)) and not body(fresh)


def test_tool_tab_name_tracks_native_caption_across_queries_and_recreation(launch):
    app = launch()
    doc = tool(app)
    initial = n.window_text(doc)
    assert initial and app.tab_labels() == [initial]
    choose_scope(app, doc, 1)
    query(app, doc, "5")
    updated = n.window_text(doc)
    assert updated != initial
    assert app.tab_labels() == [updated]
    edit(doc, 1810, "invalid")
    invoke(n.get_item(doc, 1815))
    assert n.window_text(doc) == updated and app.tab_labels() == [updated]
    close(doc)
    n.wait(lambda: not tools(app), "queried tool closed")
    assert not app.tab_labels()
    fresh = tool(app)
    assert n.window_text(fresh) == initial and app.tab_labels() == [initial]


def test_explicit_query_stale_results_checked_numbers_and_language(launch, settings, ui_language):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 2)
    original = query(app, doc, "0x80070005")
    assert "0x80070005" in original and "0x00000005" in original
    assert "2147024891" in original
    assert "0x80070005" in query(app, doc, "-2147024891")
    old = body(doc)
    edit(doc, 1810, "4294967296")
    assert body(doc) == old
    assert app.status_text()
    invoke(n.get_item(doc, 1815))
    assert body(doc) == old
    invalid = resource_text(settings[0], 2334, ui_language.langid) + ':'
    assert app.status_text().startswith(invalid)
    choose_scope(app, doc, 1)
    assert "0x00000008" in query(app, doc, "010")
    language = n.get_item(doc, 1812)
    edit(doc, 1812, "-1")
    old = body(doc)
    invoke(n.get_item(doc, 1815))
    assert body(doc) == old
    assert app.status_text().startswith(invalid)
    edit(doc, 1812, "0x0409")
    assert "0x0409" in query(app, doc, "0")


def test_tool_history_preserves_current_results_and_skips_closed_instances(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    declaration = app.open_row(app.named_rows("CreateFileW")[0])
    doc = tool(app)
    first = query(app, doc, "5")
    app.command(HISTORY_BACK)
    n.wait(lambda: app.active_document() == declaration, "back to declaration")
    app.command(HISTORY_FORWARD)
    n.wait(lambda: app.active_document() == doc, "forward to live tool")
    assert body(doc) == first
    second = query(app, doc, "87")
    assert "0x00000057" in second
    app.command(HISTORY_BACK)
    n.wait(lambda: app.active_document() == declaration, "query did not add history")
    app.command(HISTORY_FORWARD)
    n.wait(lambda: app.active_document() == doc, "live tool restored")
    assert body(doc) == second, "history never rolls back queries"
    close(doc)
    n.wait(lambda: not tools(app), "tool closed")
    app.command(HISTORY_BACK)
    app.command(HISTORY_FORWARD)
    assert not tools(app), "history must not recreate a closed tool"
    assert not enabled(app, HISTORY_FORWARD)


def test_tool_history_expires_without_reviving_recreated_instances(launch):
    app = launch()
    first = tool(app)
    second = tool(app, new=True)
    activate(first)
    app.command(HISTORY_BACK)
    n.wait(lambda: app.active_document() == second, "back to second tool")
    app.command(HISTORY_FORWARD)
    n.wait(lambda: app.active_document() == first, "forward to first tool")
    close(first)
    n.wait(lambda: tools(app) == [second], "first tool removed")
    assert not enabled(app, HISTORY_BACK)
    fresh = tool(app, new=True)
    assert not body(fresh)
    app.command(HISTORY_BACK)
    n.wait(lambda: app.active_document() == second, "back skips expired first instance")
    assert not enabled(app, HISTORY_BACK), "recreation must not revive older visits"
    app.command(HISTORY_FORWARD)
    n.wait(lambda: app.active_document() == fresh, "forward to fresh instance")
    assert not enabled(app, HISTORY_FORWARD)
    close(fresh)
    close(second)
    n.wait(lambda: not tools(app), "all visited tools closed")
    for direction in (HISTORY_BACK, HISTORY_FORWARD):
        assert not enabled(app, direction)
        app.command(direction)
        assert not tools(app), "expired tool visits must not recreate instances"


def test_custom_module_scope_candidates_are_not_system_language_promises(launch, settings, ui_language):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    assert n.is_visible(n.get_item(doc, 1813)) and n.is_visible(n.get_item(doc, 1814))
    assert len(app.choices(n.get_item(doc, 1812))) == 1
    missing = settings[2] / "missing-message-module.dll"
    edit(doc, 1813, str(missing))
    edit(doc, 1810, "0x12345678")
    invoke(n.get_item(doc, 1815))
    n.wait(lambda: "0x12345678" in body(doc), "module failure result")
    assert str(missing) in body(doc)
    assert resource_text(settings[0], 2327, ui_language.langid) in body(doc)
    choose_scope(app, doc, 0)
    assert not n.is_visible(n.get_item(doc, 1813))
    assert len(app.choices(n.get_item(doc, 1812))) >= 2
    assert "0x00000005" in query(app, doc, "5")


def test_tool_sidebar_and_environment_do_not_reset_queries(launch):
    app = launch()
    doc = tool(app)
    result = query(app, doc, "0xC0000005")
    tool_status = app.status_text()
    app.command(1040)
    sidebar_status = app.status_text()
    assert sidebar_status
    assert body(doc) == result
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    n.send(app.frame, 0x031a)  # WM_THEMECHANGED, absent from older pywin32 constants.
    assert body(doc) == result
    assert n.window_text(n.get_item(doc, 1810)) == "0xC0000005"
    assert app.status_text() == sidebar_status
    app.focus(n.get_item(doc, 1810))
    n.wait(lambda: app.status_text() == tool_status, "tool status restored after environment refresh")
    app.command(1800)
    assert tool(app) == doc


def test_tool_keyboard_open_enter_query_and_focus_regions(launch):
    app = launch()
    app.command(1800)
    panel = app.tool_list
    app.select(0, panel)
    app.focus(panel)
    win32gui.PostMessage(panel, win32con.WM_KEYDOWN, win32con.VK_DOWN, 1)
    win32gui.PostMessage(panel, win32con.WM_KEYUP, win32con.VK_DOWN, -1073741823)
    assert not tools(app)
    win32gui.PostMessage(panel, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(panel, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    doc = n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                 else None, "Enter opens selected tool")
    code = n.get_item(doc, 1810)
    n.wait(lambda: app.focused() == code, "tool input focused")
    edit(doc, 1810, "5")
    win32gui.PostMessage(code, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(code, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    n.wait(lambda: "0x00000005" in body(doc), "Enter executes explicit query")
    app.command(1025)
    n.wait(lambda: app.focused() == panel, "F6 to sidebar")
    app.command(1025)
    n.wait(lambda: app.focused() == n.get_item(app.frame, 1104), "F6 to tabs")
    app.command(1025)
    n.wait(lambda: app.focused() == code, "F6 to active tool")
    app.command(1024)
    n.wait(lambda: app.focused() == app.search, "Ctrl+L retains search focus semantics")
    assert app.active_document() == doc and "0x00000005" in body(doc)


def test_tool_module_languages_and_messages_use_selected_resource(launch, settings, ui_language):
    module = settings[0].with_name('messages-x64.dll')
    if not module.exists():
        pytest.skip('bilingual message resource fixture was not deployed')
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    edit(doc, 1813, str(module))
    app.focus(n.get_item(doc, 1813))
    app.focus(n.get_item(doc, 1810))
    language = n.get_item(doc, 1812)
    n.wait(lambda: len(app.choices(language)) == 3,
           'automatic plus module English and Chinese languages')
    for lang, expected in [('0x0409', 'Fixture'), ('0x0804', '测试消息')]:
        edit(doc, 1812, lang)
        result = query(app, doc, '0x20000001')
        assert expected in result and '%1' in result
        assert str(module) in result
    legacy = query(app, doc, '0x20000002')
    assert '"0x%08lx" referenced "0x%08lx"; operation "%s".' in legacy
    result = query(app, doc, '5')
    assert resource_text(settings[0], 2324, ui_language.langid) in result
    assert '拒绝访问' not in result and 'Access is denied' not in result

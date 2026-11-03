"""Tool instances share MDI/history, but own requests and live-only state."""
import struct
import win32con
import win32gui
import pytest

from support import native as n
from test_history import enabled, HISTORY_BACK, HISTORY_FORWARD


def tools(app):
    return [h for h in n.windows(app.mdi)
            if n.class_name(h) == "Winlex.Document" and n.get_item(h, 1816)]


def tool(app, new=False):
    panel = n.get_item(app.frame, 1801)
    if not n.is_visible(panel):
        app.command(1800)
    app.select(0, panel)
    app.command(1802 if new else 1050)
    return n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                  else None, "active fixed tool")


def choose_scope(app, doc, index):
    n.send(n.get_item(doc, 1811), win32con.CB_SETCURSEL, index)
    app.command(1811, doc, win32con.CBN_SELENDOK)


def body(doc):
    return n.window_text(n.get_item(doc, 1816))


def query(app, doc, value):
    n.set_text(n.get_item(doc, 1810), value)
    app.command(1815, doc)
    return n.wait(lambda: body(doc) if not app.status_text()
                  and body(doc) else None, "tool explicit query", 30)


def test_tool_compact_layout_and_narrow_fallback(launch):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 4)
    with n.window_dpi_context(doc):
        dpi = (n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc)
               if hasattr(n.user, "GetDpiForWindow") else 96)
        scale = lambda value: round(value * dpi / 96)
        n.send(app.mdi, win32con.WM_MDIRESTORE, doc)
        for width, paired in [(900, True), (320, False), (900, True)]:
            assert n.set_pos(doc, 0, 0, 0, scale(width), scale(560),
                             win32con.SWP_NOMOVE | win32con.SWP_NOZORDER |
                             win32con.SWP_NOACTIVATE)
            code, scope, language, module, browse, execute, result, copy = [
                n.rectangle(n.get_item(doc, control))
                for control in [1810, 1811, 1812, 1813, 1814, 1815, 1816, 1818]]
            if paired:
                assert abs(scope[1] - language[1]) <= 1
                assert scope[2] < language[0]
            else:
                assert scope[3] <= language[1]
            assert code[3] <= scope[1]
            assert language[3] <= module[1] and module[2] < browse[0]
            assert module[3] <= execute[1]
            assert execute[2] <= copy[0] or execute[3] <= copy[1]
            assert max(execute[3], copy[3]) < result[1] < result[3]
            _, _, client_width, client_height = n.rectangle(doc, client=True)
            left, top = win32gui.ClientToScreen(doc, (0, 0))
            assert left <= result[0] < result[2] <= left + client_width
            assert top <= result[1] < result[3] <= top + client_height
    assert not n.get_item(doc, 1817), "tool has no local status EDIT"


def test_tool_labels_and_input_tooltip_registration(launch):
    app = launch()
    doc = tool(app)
    children = n.windows(doc)
    labels = [n.window_text(h) for h in children
              if n.class_name(h).lower() in ("static", "button")]
    assert labels and all("&" not in label for label in labels)
    assert not any("010" in label for label in labels), "syntax help is not a static row"
    code = n.get_item(doc, 1810)
    pointer = "<Q" if app.is64 else "<I"
    data = bytearray(128)
    struct.pack_into("<I", data, 0, 64 if app.is64 else 44)
    struct.pack_into(pointer, data, 8, code)
    struct.pack_into(pointer, data, 16 if app.is64 else 12, code)
    struct.pack_into(pointer, data, 48 if app.is64 else 36, app.memory + 128)
    tooltip = None
    for hwnd in n.windows():
        if (n.class_name(hwnd) == "tooltips_class32"
                and win32gui.GetWindow(hwnd, win32con.GW_OWNER) == app.frame):
            assert n.write_memory(app.handle, app.memory, bytes(data), len(data), None)
            if n.send(hwnd, win32con.WM_USER + 53, 0, app.memory):
                tooltip = hwnd
                break
    assert tooltip, "code input has a registered native tooltip"
    assert n.write_memory(app.handle, app.memory, bytes(data), len(data), None)
    assert n.write_memory(app.handle, app.memory + 128, bytes(4096), 4096, None)
    n.send(tooltip, win32con.WM_USER + 56, 0, app.memory)
    hint = app.remote_text(app.memory + 128, 4096)
    assert all(value in hint for value in ("0x", "0o", "0b", "010", "8"))
    n.send(doc, win32con.WM_CLOSE)
    assert not n.is_window(tooltip)


def test_shared_status_tracks_active_instance_and_catalog_refresh(launch, settings):
    app = launch([settings[1]])
    first = tool(app)
    n.set_text(n.get_item(first, 1810), "invalid")
    app.command(1815, first)
    invalid = app.status_text()
    assert invalid.startswith(("Invalid input", "输入无效"))
    second = tool(app, new=True)
    ready = app.status_text()
    assert ready and ready != invalid
    n.send(app.mdi, win32con.WM_MDIACTIVATE, first)
    assert app.status_text() == invalid
    n.send(app.mdi, win32con.WM_MDIACTIVATE, second)
    assert app.status_text() == ready
    n.set_text(n.get_item(first, 1810), "5")
    app.command(1815, first)
    n.wait(lambda: "0x00000005" in body(first), "inactive tool completes", 30)
    assert app.status_text() == ready, "inactive completion cannot replace active status"
    app.command(1040)
    n.set_text(app.search, "CreateFileW")
    n.wait(lambda: app.count() and app.named_rows("CreateFileW"), "background catalog search", 45)
    assert app.active_document() == second and app.status_text() == ready
    n.send(app.mdi, win32con.WM_MDIACTIVATE, first)
    assert not app.status_text()
    n.send(first, win32con.WM_CLOSE)
    assert app.active_document() == second and app.status_text() == ready
    n.send(second, win32con.WM_CLOSE)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "catalog status restored", 45)


def test_tools_without_metadata_are_fixed_independent_and_reused_by_recency(launch):
    app = launch()
    assert app.count(app.sources) == 0
    panel = n.get_item(app.frame, 1801)
    app.command(1800)
    assert app.count(panel) == 1
    app.select(0, panel)
    assert not tools(app), "selection alone must not create content"
    first = tool(app)
    assert not n.get_item(first, 1047) and not n.get_item(first, 1211)
    assert all(not value.startswith(("Preview · ", "预览 · "))
               for value in app.tab_descriptions())
    choose_scope(app, first, 1)
    initial = query(app, first, "5")
    assert "0x00000005" in initial
    assert tool(app) == first and body(first) == initial
    second = tool(app, new=True)
    assert second != first
    assert not n.window_text(n.get_item(second, 1810)) and not body(second)
    assert n.send(n.get_item(second, 1811), win32con.CB_GETCURSEL) == 0
    assert tool(app) == second
    n.send(app.mdi, win32con.WM_MDIACTIVATE, first)
    assert tool(app) == first, "normal open chooses most recently visited live instance"
    assert body(first) == initial and not body(second)
    n.send(first, win32con.WM_CLOSE)
    n.wait(lambda: first not in tools(app), "tool closed")
    assert tool(app) == second
    n.send(second, win32con.WM_CLOSE)
    n.wait(lambda: not tools(app), "all tools closed")
    fresh = tool(app)
    assert not n.window_text(n.get_item(fresh, 1810)) and not body(fresh)


def test_explicit_query_stale_results_checked_numbers_and_language(launch):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 2)
    original = query(app, doc, "0x80070005")
    assert "0x80070005" in original and "0x00000005" in original
    assert "2147024891" in original
    assert "0x80070005" in query(app, doc, "-2147024891")
    old = body(doc)
    n.set_text(n.get_item(doc, 1810), "4294967296")
    assert body(doc) == old
    assert app.status_text()
    app.command(1815, doc)
    assert body(doc) == old
    assert app.status_text().startswith(("Invalid input", "输入无效"))
    choose_scope(app, doc, 1)
    assert "0x00000008" in query(app, doc, "010")
    language = n.get_item(doc, 1812)
    n.set_text(language, "-1")
    app.command(1812, doc, win32con.CBN_EDITCHANGE)
    old = body(doc)
    app.command(1815, doc)
    assert body(doc) == old
    assert app.status_text().startswith(("Invalid input", "输入无效"))
    n.set_text(language, "0x0409")
    app.command(1812, doc, win32con.CBN_EDITCHANGE)
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
    n.send(doc, win32con.WM_CLOSE)
    n.wait(lambda: not tools(app), "tool closed")
    app.command(HISTORY_BACK)
    app.command(HISTORY_FORWARD)
    assert not tools(app), "history must not recreate a closed tool"
    assert not enabled(app, HISTORY_FORWARD)


def test_custom_module_scope_candidates_are_not_system_language_promises(launch, settings):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 4)
    assert n.is_visible(n.get_item(doc, 1813)) and n.is_visible(n.get_item(doc, 1814))
    assert n.send(n.get_item(doc, 1812), win32con.CB_GETCOUNT) == 1
    missing = settings[2] / "missing-message-module.dll"
    n.set_text(n.get_item(doc, 1813), str(missing))
    n.set_text(n.get_item(doc, 1810), "0x12345678")
    app.command(1815, doc)
    n.wait(lambda: "0x12345678" in body(doc), "module failure result")
    assert str(missing) in body(doc)
    assert "failed" in body(doc).lower() or "失败" in body(doc)
    choose_scope(app, doc, 0)
    assert not n.is_visible(n.get_item(doc, 1813))
    assert n.send(n.get_item(doc, 1812), win32con.CB_GETCOUNT) >= 2
    assert "0x00000005" in query(app, doc, "5")


def test_tool_sidebar_and_environment_do_not_reset_queries(launch):
    app = launch()
    doc = tool(app)
    result = query(app, doc, "0xC0000005")
    app.command(1040)
    assert body(doc) == result
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    n.send(app.frame, 0x031a)  # WM_THEMECHANGED, absent from older pywin32 constants.
    assert body(doc) == result
    assert n.window_text(n.get_item(doc, 1810)) == "0xC0000005"
    assert not app.status_text()
    app.command(1800)
    assert tool(app) == doc


def test_tool_keyboard_open_enter_query_and_focus_regions(launch):
    app = launch()
    app.command(1800)
    panel = n.get_item(app.frame, 1801)
    app.select(0, panel)
    app.focus(panel)
    n.key_message(panel, win32con.VK_DOWN)
    assert not tools(app)
    n.key_message(panel, win32con.VK_RETURN)
    doc = n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                 else None, "Enter opens selected tool")
    code = n.get_item(doc, 1810)
    n.wait(lambda: app.focused() == code, "tool input focused")
    n.set_text(code, "5")
    n.key_message(code, win32con.VK_RETURN)
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


def test_tool_module_languages_and_messages_use_selected_resource(launch, settings):
    module = settings[0].with_name('messages-x64.dll')
    if not module.exists():
        pytest.skip('bilingual message resource fixture was not deployed')
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 4)
    n.set_text(n.get_item(doc, 1813), str(module))
    app.command(1813, doc, win32con.EN_KILLFOCUS)
    language = n.get_item(doc, 1812)
    n.wait(lambda: n.send(language, win32con.CB_GETCOUNT) == 3,
           'automatic plus module English and Chinese languages')
    for lang, expected in [('0x0409', 'Fixture'), ('0x0804', '测试消息')]:
        n.set_text(language, lang)
        app.command(1812, doc, win32con.CBN_EDITCHANGE)
        result = query(app, doc, '0x20000001')
        assert expected in result and '%1' in result
        assert str(module) in result
    legacy = query(app, doc, '0x20000002')
    assert '"0x%08lx" referenced "0x%08lx"; operation "%s".' in legacy
    result = query(app, doc, '5')
    assert 'no message found' in result.lower() or '未找到' in result
    assert '拒绝访问' not in result and 'Access is denied' not in result

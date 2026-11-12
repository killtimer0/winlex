"""Actual mouse/keyboard delivery, deliberately separate from message checks."""
import pytest
import uiautomation as auto
import win32con
import win32gui
from support import native as n
from support.input_state import InputState

pytestmark = pytest.mark.interactive


@pytest.mark.parametrize("command,control", [(1040, 1103), (1021, 1112), (1800, 1801)])
def test_real_sidebar_context_keys(launch, settings, command, control):
    from support.input_state import foreground
    from test_context import find_popup, wait_closed

    with InputState() as inputs:
        app = launch([settings[1]])
        app.query("ERROR_ACCESS_DENIED")
        with foreground(app, inputs):
            target = n.get_item(app.frame, control)
            if not n.is_visible(target):
                app.command(command)
                target = n.wait(lambda: n.get_item(app.frame, control), "sidebar control exposed")
            assert target and n.is_visible(target)
            app.select(0, target)
            app.focus(target)
            for gesture in ("apps", "shift-f10"):
                if gesture == "apps":
                    inputs.key(win32con.VK_APPS)
                else:
                    inputs.state["held"].append(win32con.VK_SHIFT)
                    inputs.save()
                    auto.PressKey(win32con.VK_SHIFT)
                    try:
                        inputs.key(win32con.VK_F10)
                    finally:
                        auto.ReleaseKey(win32con.VK_SHIFT)
                        inputs.state["held"].remove(win32con.VK_SHIFT)
                        inputs.save()
                _, ids = find_popup(app)
                assert ids
                inputs.key(win32con.VK_ESCAPE)
                wait_closed(target)
                assert app.selected_row(target) == 0


def test_real_tool_toolbar_and_list_click(launch):
    with InputState() as input_state:
        app = launch()
        frame = auto.ControlFromHandle(app.frame)
        assert frame.SetTopmost(True)
        try:
            toolbar = n.get_item(app.frame, 1113)
            button = auto.ControlFromHandle(toolbar).Control(Name=app.toolbar_text(1800))
            button.Click()
            panel = n.wait(lambda: app.tool_list, "tool panel opened by toolbar click")
            item = app.row(0, panel)
            item.Click()
            assert app.active_document() is None
            item.DoubleClick()
            n.wait(lambda: n.get_item(app.active_document(), 1816), "tool opened by list double click")
        finally:
            assert frame.SetTopmost(False)


def test_real_search_keyboard(launch, settings):
    with InputState() as input_state:
        app = launch([settings[1]])
        n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
        # Expose the window for a real mouse click on the search input.
        frame = auto.ControlFromHandle(app.frame)
        assert frame.SetTopmost(True)
        try:
            with n.window_dpi_context(app.frame):
                left, top, right, bottom = n.rectangle(app.search)
                input_state.click((left+right)//2, (top+bottom)//2)
            n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "foreground ownership")
            n.wait(lambda: app.focused() == app.search, "search input focus")
        finally:
            assert frame.SetTopmost(False)
        input_state.disable_ime(app.search)
        for character in "CREATEFILEW":
            input_state.key(ord(character))
        n.wait(lambda: n.window_text(app.search).casefold() == "createfilew", "real input delivered")
        n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "real-input search completed", 45)
        assert "CreateFileW" in app.cell(0)
        mode = n.get_item(app.frame, 1114)
        input_state.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == mode, "real Tab reaches mode")
        input_state.key(win32con.VK_HOME)
        input_state.key(win32con.VK_DOWN)
        input_state.key(win32con.VK_RETURN)
        n.wait(lambda: auto.ControlFromHandle(mode).GetValuePattern().Value
               == app.choices(mode)[1], "real keyboard selects condition mode")
        n.wait(lambda: "condition" in app.status_text() or "条件" in app.status_text(), "condition diagnostic", 45)
        assert app.count() == 0
        input_state.key(win32con.VK_HOME)
        input_state.key(win32con.VK_RETURN)
        n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "quick search restored", 45)
        assert app.count() > 0
        input_state.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == app.results, "real Tab reaches results")


def test_real_tool_find_keyboard(launch):
    from support.input_state import foreground
    from test_tools import edit
    from test_navigation import editor_selection
    from test_find import replace_input

    with InputState() as inputs:
        app = launch()
        app.command(1800)
        app.select(1, app.tool_list)
        app.command(1050)
        doc = n.wait(lambda: app.active_document(), "demangle tool opened")
        editor = n.get_item(doc, 1823)
        replace_input(n.get_item(doc, 1820), "_Z3foov\n_Z3fooi")
        n.wait(lambda: "foo" in n.rich_edit_text(editor), "demangle result")
        with foreground(app, inputs):
            app.focus(n.get_item(doc, 1820))
            inputs.state["held"].append(win32con.VK_CONTROL)
            inputs.save()
            auto.PressKey(win32con.VK_CONTROL, waitTime=0)
            try:
                inputs.key(ord("F"))
            finally:
                auto.ReleaseKey(win32con.VK_CONTROL, waitTime=0)
                inputs.state["held"].remove(win32con.VK_CONTROL)
                inputs.save()
            find = n.get_item(doc, 1220)
            n.wait(lambda: n.is_visible(find) and app.focused() == find, "Ctrl+F opens tool find")
            inputs.disable_ime(find)
            edit(doc, 1220, "foo")
            n.wait(lambda: n.rich_edit_document(editor).Selection.Text == "foo", "first match")
            first = editor_selection(app, editor)
            inputs.key(win32con.VK_F3)
            n.wait(lambda: editor_selection(app, editor)[0] > first[0], "F3 finds next")
            inputs.state["held"].append(win32con.VK_SHIFT)
            inputs.save()
            auto.PressKey(win32con.VK_SHIFT, waitTime=0)
            try:
                inputs.key(win32con.VK_F3)
            finally:
                auto.ReleaseKey(win32con.VK_SHIFT, waitTime=0)
                inputs.state["held"].remove(win32con.VK_SHIFT)
                inputs.save()
            n.wait(lambda: editor_selection(app, editor) == first, "Shift+F3 finds previous")
            inputs.key(win32con.VK_ESCAPE)
            n.wait(lambda: not n.is_visible(find) and app.focused() == editor, "Esc returns to result")
            assert n.window_text(n.get_item(doc, 1820)).replace("\r", "") == "_Z3foov\n_Z3fooi"

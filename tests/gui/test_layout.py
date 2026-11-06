import pytest
import win32con
import win32gui
import uiautomation as auto
from support.diagnostics import capture
from support.input_state import InputState, foreground
from support import messages
from support import native as n
from test_context import select_text
from test_tools import close, visual_state


def test_toolbar_geometry_activation_and_focus(launch):
    app = launch()
    with n.window_dpi_context(app.frame):
        check_toolbar_and_search(app)


@pytest.mark.interactive
@pytest.mark.parametrize("sources", [False, True])
def test_list_selection_paint_follows_focus(launch, settings, sources):
    app = launch([settings[1]])
    app.query("CreateFileW")
    if sources:
        app.command(1021)
    control = app.sources if sources else app.results
    other = n.get_item(app.frame, 1020) if sources else app.search
    app.select(0, control)

    def background():
        with n.window_dpi_context(app.frame):
            _, _, width, height = n.rectangle(control, client=True)
            left, top, right, bottom = app.item_rect(0, control)
            with capture(control, client=True) as (bitmap, _, _):
                rgb = bitmap.GetPixelColor(min(right, width) - 4, (top + bottom) // 2 - 3)
                return ((rgb & 255) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 255)

    with InputState() as inputs, foreground(app, inputs):
        for theme_name in ("Explorer", ""):
            theme = n.c.WinDLL("uxtheme")
            assert n.api(theme, "SetWindowTheme", n.c.c_long, n.w.HWND,
                         n.w.LPCWSTR, n.w.LPCWSTR)(control, theme_name, None if theme_name else "") == 0
            app.focus(control)
            assert app.focused() == control
            selected = background()
            app.focus(other)
            unfocused = background()
            assert app.selected_row(control) == 0  # Losing focus hides the highlight, not selection.
            assert selected != unfocused, (theme_name, selected, unfocused)
            assert unfocused == win32gui.GetSysColor(win32con.COLOR_WINDOW)


def check_toolbar_and_search(app):
    toolbar = n.get_item(app.frame, 1113)
    buttons = auto.ControlFromHandle(toolbar).GetChildren()
    def button_rect(index):
        rect = buttons[index].BoundingRectangle
        left, top, _, _ = n.rectangle(toolbar)
        return rect.left - left, rect.top - top, rect.right - left, rect.bottom - top

    first, second = button_rect(0), button_rect(1)
    area = n.rectangle(toolbar, client=True)
    assert first[0] == second[0] and first[3] <= second[1]
    assert 0 <= first[0] < first[2] <= area[2]
    assert 0 <= second[1] < second[3] <= area[3]
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)

    x, y = (second[0] + second[2]) // 2, (second[1] + second[3]) // 2
    point = (x & 0xffff) | ((y & 0xffff) << 16)
    n.send(toolbar, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
    n.send(toolbar, win32con.WM_LBUTTONUP, 0, point)
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    n.wait(lambda: app.focused() == app.sources, "mouse activation focuses sources")
    frame = auto.ControlFromHandle(app.frame)
    rect = frame.BoundingRectangle
    transform = frame.GetTransformPattern()
    assert transform.Resize(rect.width() + 1, rect.height())
    assert transform.Resize(rect.width(), rect.height())
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    assert app.focused() == app.sources, "layout and environment updates must not move focus"
    n.send(toolbar, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
    n.send(toolbar, win32con.WM_LBUTTONUP, 0, point)
    assert not n.is_visible(app.sources)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    n.wait(lambda: app.focused() == toolbar, "mouse collapse focuses toolbar")
    x, y = (first[0] + first[2]) // 2, (first[1] + first[3]) // 2
    point = (x & 0xffff) | ((y & 0xffff) << 16)
    n.send(toolbar, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
    n.send(toolbar, win32con.WM_LBUTTONUP, 0, point)
    assert n.is_visible(app.results) and n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    n.wait(lambda: app.focused() == app.search, "mouse activation focuses search")
    n.send(toolbar, messages.TB_SETHOTITEM, 0)
    win32gui.PostMessage(toolbar, win32con.WM_KEYDOWN, win32con.VK_SPACE, 1)
    win32gui.PostMessage(toolbar, win32con.WM_KEYUP, win32con.VK_SPACE, -1073741823)
    n.wait(lambda: not n.is_visible(app.results), "toolbar Space collapses panel")
    n.wait(lambda: app.focused() == toolbar, "keyboard collapse focuses toolbar")
    n.send(toolbar, messages.TB_SETHOTITEM, 0)
    win32gui.PostMessage(toolbar, win32con.WM_KEYDOWN, win32con.VK_DOWN, 1)
    win32gui.PostMessage(toolbar, win32con.WM_KEYUP, win32con.VK_DOWN, -1073741823)
    n.wait(lambda: n.send(toolbar, messages.TB_GETHOTITEM) == 1, "toolbar Down selects next item")
    win32gui.PostMessage(toolbar, win32con.WM_KEYDOWN, win32con.VK_SPACE, 1)
    win32gui.PostMessage(toolbar, win32con.WM_KEYUP, win32con.VK_SPACE, -1073741823)
    n.wait(lambda: n.is_visible(app.sources), "toolbar keyboard switches panel")
    n.wait(lambda: app.focused() == app.sources, "keyboard activation focuses sources")
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    n.send(toolbar, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
    n.send(toolbar, win32con.WM_LBUTTONUP, 0, point)

    edit, results = n.rectangle(app.search), n.rectangle(app.results)
    mode = n.rectangle(n.get_item(app.frame, 1114))
    assert edit[0] == results[0] and mode[2] == results[2]
    assert edit[2] < mode[0]
    assert edit[1] == mode[1]
    assert edit[3] < results[1]


def test_environment_refresh_preserves_open_document(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "declaration", 45)
    app.command(1013)  # restore before testing ordinary-window wrapping
    app.command(1021)
    editor = n.get_item(doc, 1203)
    button = n.get_item(app.frame, 1020)
    content = app.content(doc)
    select_text(editor, 2, 8)
    with n.window_dpi_context(app.frame):
        outer = n.rectangle(doc)
        n.send(app.frame, win32con.WM_SETTINGCHANGE)  # without changing global settings
        assert n.rectangle(doc) == outer, "free-placement document moved"
        assert n.send(button, win32con.WM_GETFONT) != 0
        height, face = app.declaration_font(doc)
        assert height > 0 and face == "Courier New"
        assert app.content(doc) == content
        assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2
        controls = [n.get_item(app.frame, id) for id in (1020, 1022, 1044)]
        assert all(h and n.is_visible(h) for h in controls)
        rectangles = [n.rectangle(h) for h in controls]
        bounds = n.rectangle(app.sources)
        for i, r in enumerate(rectangles):
            assert bounds[0] <= r[0] <= r[2] <= bounds[2]
            assert r[3] <= bounds[1]
            for other in rectangles[i + 1:]:
                assert r[2] <= other[0] or other[2] <= r[0] or r[3] <= other[1] or other[3] <= r[1]


def test_mdi_maximize_inheritance_and_one_shot_arrangement(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    original = app.open_row()
    visual_state(original, auto.WindowVisualState.Maximized)
    assert n.is_zoomed(original)
    other = app.open_new()
    assert n.is_zoomed(other), "new document must inherit MDI maximization"
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(original) if hasattr(n.user, "GetDpiForWindow") else 96
        for command in (1011, 1012, None):
            if command is None:
                # The application exposes no cascade command; exercise the MDI contract directly.
                n.send(app.mdi, win32con.WM_MDICASCADE)
            else:
                app.command(command)
            for doc in app.documents():
                left, top, right, bottom = n.rectangle(doc)
                assert not n.is_zoomed(doc)
                assert right - left >= (320 * dpi + 48) // 96
                assert bottom - top >= (400 * dpi + 48) // 96
    app.command(1011)  # one-shot vertical tile
    assert not any(n.is_zoomed(h) for h in app.documents())
    before = {h: n.rectangle(h) for h in app.documents()}
    assert before[original] != before[other]
    app.query("CloseHandle")
    third = app.open_row()
    assert third not in before
    assert {h: n.rectangle(h) for h in before} == before, "creation must not retile existing documents"
    close(third)
    assert {h: n.rectangle(h) for h in before} == before, "closing must not retile remaining documents"
    frame = n.rectangle(app.frame)
    assert auto.ControlFromHandle(app.frame).GetTransformPattern().Resize(
        frame[2] - frame[0] + 80, frame[3] - frame[1] + 60)
    assert {h: n.rectangle(h) for h in before} == before, "frame resize must not retile documents"


def test_accelerator_region_navigation_and_tab_enter(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    editor = n.get_item(doc, 1203)

    app.focus(editor)
    n.wait(lambda: app.focused() == editor, "document focus")
    win32gui.PostMessage(editor, win32con.WM_KEYDOWN, win32con.VK_F6, 1)
    win32gui.PostMessage(editor, win32con.WM_KEYUP, win32con.VK_F6, -1073741823)
    n.wait(lambda: app.focused() == app.search, "F6 to sidebar")
    win32gui.PostMessage(app.search, win32con.WM_KEYDOWN, win32con.VK_F6, 1)
    win32gui.PostMessage(app.search, win32con.WM_KEYUP, win32con.VK_F6, -1073741823)
    tabs = n.get_item(app.frame, 1104)
    n.wait(lambda: app.focused() == tabs, "F6 to tabs")
    win32gui.PostMessage(tabs, win32con.WM_KEYDOWN, win32con.VK_F6, 1)
    win32gui.PostMessage(tabs, win32con.WM_KEYUP, win32con.VK_F6, -1073741823)
    n.wait(lambda: app.focused() == editor, "F6 to document")
    win32gui.PostMessage(editor, win32con.WM_KEYDOWN, win32con.VK_F6, 1)
    win32gui.PostMessage(editor, win32con.WM_KEYUP, win32con.VK_F6, -1073741823)
    n.wait(lambda: app.focused() == app.search, "sidebar before tab Enter")
    win32gui.PostMessage(tabs, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(tabs, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    n.wait(lambda: app.focused() == editor, "tab Enter to document")


def test_mdi_remembers_last_user_state_after_all_documents_close(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    assert n.is_zoomed(doc), "first document defaults to maximized"

    def reopen(previous):
        close(previous)
        n.wait(lambda: not app.documents(), "all documents closed")
        return app.open_row()

    doc = reopen(doc)
    assert n.is_zoomed(doc), "closing all must retain maximized preference"
    win32gui.SendMessage(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE, 0)
    assert not n.is_zoomed(doc)
    doc = reopen(doc)
    assert not n.is_zoomed(doc), "closing all must retain restored preference"
    win32gui.SendMessage(doc, win32con.WM_SYSCOMMAND, win32con.SC_MAXIMIZE, 0)
    assert n.is_zoomed(doc)
    doc = reopen(doc)
    assert n.is_zoomed(doc)
    app.command(1011)  # explicit tiling changes the preference to normal
    assert not n.is_zoomed(doc)
    doc = reopen(doc)
    assert not n.is_zoomed(doc)
    app.command(1014)  # application Window > Maximize command
    doc = reopen(doc)
    assert n.is_zoomed(doc)

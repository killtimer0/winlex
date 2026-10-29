import struct
import win32con
import win32gui
from support.diagnostics import capture
from support import messages
from support import native as n


def test_toolbar_geometry_activation_and_focus(launch):
    app = launch()
    with n.window_dpi_context(app.frame):
        check_toolbar_and_search(app)


def test_list_selection_paint_follows_focus(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    app.select(0)

    def background():
        with n.window_dpi_context(app.frame):
            _, _, width, height = n.rectangle(app.results, client=True)
            left, top, right, bottom = app.item_rect(0)
            with capture(app.results, client=True) as (target, _, _):
                return win32gui.GetPixel(target, min(right, width) - 4,
                                        (top + bottom) // 2 - 3)

    for classic in (False, True):
        if classic:
            theme = n.c.WinDLL("uxtheme")
            assert n.api(theme, "SetWindowTheme", n.c.c_long, n.w.HWND,
                         n.w.LPCWSTR, n.w.LPCWSTR)(app.results, "", "") == 0
        app.focus(app.results)
        assert app.focused() == app.results
        selected = background()
        app.focus(app.search)
        unfocused = background()
        assert app.selected_row() == 0  # Losing focus hides the highlight, not selection.
        assert selected != unfocused, (classic, selected, unfocused)
        if classic:
            assert selected == win32gui.GetSysColor(win32con.COLOR_HIGHLIGHT)
            assert unfocused == win32gui.GetSysColor(win32con.COLOR_WINDOW)


def check_toolbar_and_search(app):
    toolbar = n.get_item(app.frame, 1113)
    assert n.class_name(toolbar) == "ToolbarWindow32"
    assert n.send(toolbar, messages.TB_BUTTONCOUNT) == 2

    def button_rect(index):
        assert n.send(toolbar, messages.TB_GETITEMRECT, index, app.memory)
        buffer = n.c.create_string_buffer(16)
        assert n.read_memory(app.handle, app.memory, buffer, 16, None)
        return struct.unpack("<4i", buffer.raw)

    first, second = button_rect(0), button_rect(1)
    area = n.rectangle(toolbar, client=True)
    assert first[0] == second[0] and first[3] <= second[1]
    assert 0 <= first[0] < first[2] <= area[2]
    assert 0 <= second[1] < second[3] <= area[3]
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)

    def click(rect):
        x, y = (rect[0] + rect[2]) // 2, (rect[1] + rect[3]) // 2
        position = x | (y << 16)
        n.send(toolbar, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, position)
        n.send(toolbar, win32con.WM_LBUTTONUP, 0, position)

    click(second)  # Message-driven toolbar activation, not actual mouse input.
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    n.wait(lambda: app.focused() == app.sources, "mouse activation focuses sources")
    n.send(app.frame, win32con.WM_SIZE)
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    assert app.focused() == app.sources, "layout and environment updates must not move focus"
    click(second)
    assert not n.is_visible(app.sources)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    n.wait(lambda: app.focused() == toolbar, "mouse collapse focuses toolbar")
    click(first)
    assert n.is_visible(app.results) and n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    n.wait(lambda: app.focused() == app.search, "mouse activation focuses search")
    n.send(toolbar, messages.TB_SETHOTITEM, 0)
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: not n.is_visible(app.results), "toolbar Space collapses panel")
    n.wait(lambda: app.focused() == toolbar, "keyboard collapse focuses toolbar")
    n.send(toolbar, messages.TB_SETHOTITEM, 0)
    n.key_message(toolbar, win32con.VK_DOWN)
    n.wait(lambda: n.send(toolbar, messages.TB_GETHOTITEM) == 1, "toolbar Down selects next item")
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: n.is_visible(app.sources), "toolbar keyboard switches panel")
    n.wait(lambda: app.focused() == app.sources, "keyboard activation focuses sources")
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    click(first)

    get_style = n.api(n.user, "GetWindowLongW", n.c.c_long, n.w.HWND, n.c.c_int)
    assert get_style(app.search, win32con.GWL_EXSTYLE) & win32con.WS_EX_CLIENTEDGE
    edit, results = n.rectangle(app.search), n.rectangle(app.results)
    assert edit[0] == results[0] and edit[2] == results[2]
    assert edit[3] < results[1]


def test_environment_remeasures_open_document(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "declaration", 45)
    app.command(1012)  # restore before testing ordinary-window wrapping
    app.command(1021)
    editor = n.get_item(doc, 1203)
    button = n.get_item(app.frame, 1020)
    content = app.content(doc)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    with n.window_dpi_context(app.frame):
        before = n.rectangle(app.sources)
        outer = n.rectangle(doc)
        n.set_text(button, "Open metadata with a substantially longer action label")
        n.send(app.frame, win32con.WM_SETTINGCHANGE)  # without changing global settings
        assert n.rectangle(doc) == outer, "free-placement document moved"
        assert n.rectangle(app.sources)[1] > before[1], "long label did not trigger wrapping"
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
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    original = app.open_row()
    n.send(app.mdi, win32con.WM_MDIMAXIMIZE, original)
    assert n.is_zoomed(original)
    other = app.open_new()
    assert n.is_zoomed(other), "new document must inherit MDI maximization"
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(original) if hasattr(n.user, "GetDpiForWindow") else 96
        for message, flags in ((win32con.WM_MDITILE, win32con.MDITILE_VERTICAL),
                               (win32con.WM_MDITILE, win32con.MDITILE_HORIZONTAL),
                               (win32con.WM_MDICASCADE, 0)):
            n.send(app.mdi, message, flags)
            for doc in app.documents():
                left, top, right, bottom = n.rectangle(doc)
                assert not n.is_zoomed(doc)
                assert right - left >= (320 * dpi + 48) // 96
                assert bottom - top >= (400 * dpi + 48) // 96
    app.command(1010)  # one-shot vertical tile
    assert not any(n.is_zoomed(h) for h in app.documents())
    before = {h: n.rectangle(h) for h in app.documents()}
    assert before[original] != before[other]
    app.query("CloseHandle")
    third = app.open_row()
    assert third not in before
    assert {h: n.rectangle(h) for h in before} == before, "creation must not retile existing documents"
    n.send(app.mdi, win32con.WM_MDIDESTROY, third)
    assert {h: n.rectangle(h) for h in before} == before, "closing must not retile remaining documents"
    frame = n.rectangle(app.frame)
    n.set_pos(app.frame, None, 0, 0, frame[2] - frame[0] + 80, frame[3] - frame[1] + 60,
              win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
    assert {h: n.rectangle(h) for h in before} == before, "frame resize must not retile documents"


def test_accelerator_region_navigation_and_tab_enter(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    editor = n.get_item(doc, 1203)

    n.send(doc, win32con.WM_SETFOCUS)  # document restores its editor on the owning thread
    n.wait(lambda: app.focused() == editor, "document focus")
    n.key_message(editor, win32con.VK_F6)  # F6 through TranslateAccelerator
    n.wait(lambda: app.focused() == app.search, "F6 to sidebar")
    n.key_message(app.search, win32con.VK_F6)
    n.wait(lambda: app.focused() == editor, "F6 to document")
    n.key_message(editor, win32con.VK_F6)
    n.wait(lambda: app.focused() == app.search, "sidebar before tab Enter")
    n.key_message(n.get_item(app.frame, 1104), win32con.VK_RETURN)
    n.wait(lambda: app.focused() == editor, "tab Enter to document")


def test_mdi_remembers_last_user_state_after_all_documents_close(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    assert n.is_zoomed(doc), "first document defaults to maximized"

    def reopen(previous):
        n.send(previous, win32con.WM_CLOSE)  # through the document host
        n.wait(lambda: not app.documents(), "all documents closed")
        return app.open_row()

    doc = reopen(doc)
    assert n.is_zoomed(doc), "closing all must retain maximized preference"
    n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)  # as sent by the native system menu
    assert not n.is_zoomed(doc)
    doc = reopen(doc)
    assert not n.is_zoomed(doc), "closing all must retain restored preference"
    n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_MAXIMIZE)
    assert n.is_zoomed(doc)
    doc = reopen(doc)
    assert n.is_zoomed(doc)
    app.command(1010)  # explicit tiling changes the preference to normal
    assert not n.is_zoomed(doc)
    doc = reopen(doc)
    assert not n.is_zoomed(doc)
    app.command(1013)  # application Window > Maximize command
    doc = reopen(doc)
    assert n.is_zoomed(doc)

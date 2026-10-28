import struct
import win32con
from support import messages
from support import native as n


def test_toolbar_and_search_geometry(launch):
    app = launch()
    with n.window_dpi_context(app.frame):
        check_toolbar_and_search(app)


def check_toolbar_and_search(app):
    toolbar = n.get_item(app.frame, 1113)
    assert n.class_name(toolbar) == "ToolbarWindow32"
    assert not n.get_item(app.frame, 1040)
    assert not n.get_item(app.frame, 1021)
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
    click(second)
    assert not n.is_visible(app.sources)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    click(first)
    assert n.is_visible(app.results) and n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    n.send(toolbar, messages.TB_SETHOTITEM, 0)
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: not n.is_visible(app.results), "toolbar Space collapses panel")
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: n.is_visible(app.results), "toolbar Space expands panel")
    # Activation now hands focus to the panel; collapse to navigate the toolbar again.
    click(first)
    n.wait(lambda: not n.is_visible(app.results), "collapse returns toolbar focus")
    n.send(toolbar, messages.TB_SETHOTITEM, 0)
    n.key_message(toolbar, win32con.VK_DOWN)
    n.wait(lambda: n.send(toolbar, messages.TB_GETHOTITEM) == 1, "toolbar Down selects next item")
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: n.is_visible(app.sources), "toolbar keyboard switches panel")
    assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
    assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
    click(first)

    get_style = n.api(n.user, "GetWindowLongW", n.c.c_long, n.w.HWND, n.c.c_int)
    assert get_style(app.search, win32con.GWL_EXSTYLE) & win32con.WS_EX_CLIENTEDGE
    edit, results = n.rectangle(app.search), n.rectangle(app.results)
    assert edit[0] == results[0] and edit[2] == results[2]
    assert edit[3] < results[1]


def test_native_mdi_and_sidebar(launch, profile, settings):
    app = launch(["--winmd", settings[1]])
    assert not n.get_item(app.frame, 1101)
    assert not n.get_item(app.frame, 1110)
    assert not app.documents()
    app.query("CreateFileW")
    original = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(original), "declaration", 45)
    old = app.content(original)
    other = app.open_new()
    # A document host can be enumerated before WM_CREATE has created its controls.
    assert app.choices(app.architecture(other)) == ["x86", "x64", "arm64"]
    app.choose(other, "x64")
    n.wait(lambda: 'CreateFileW(' in app.content(other), "other declaration", 45)
    assert "x86" in n.window_text(original) and "x64" in n.window_text(other)
    assert app.content(original) == old
    app.command(1047, other, win32con.CBN_SELENDOK)
    app.command(1047, original, win32con.CBN_SELENDOK)
    assert len(app.documents()) == 2
    for doc in app.documents():
        assert n.get_item(doc, 1204)
        assert n.class_name(n.get_item(doc, 1203)) == "RichEdit20W"
        for removed in (1201, 1202):
            assert not n.get_item(doc, removed)
    app.command(1010)
    rectangles = [n.rectangle(h) for h in app.documents()]
    assert rectangles[0] != rectangles[1]
    app.command(1040)
    assert not n.is_visible(app.results)
    app.close()
    assert profile.get("PanelCollapsed") == 1
    restored = launch()
    assert not n.is_visible(restored.results)
    restored.command(1040)
    assert n.is_visible(restored.results)


def test_environment_remeasures_open_document(launch, settings):
    app = launch(["--winmd", settings[1]])
    n.wait(lambda: app.count(app.sources) > 0, "discovered source")
    app.command(1021)
    doc = app.open_row(source=True)
    n.wait(lambda: '"sources"' in app.content(doc), "source details", 45)
    app.command(1012)  # restore before testing ordinary-window wrapping
    editor = n.get_item(doc, 1203)
    button = n.get_item(doc, 1020)
    content = app.content(doc)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    with n.window_dpi_context(app.frame):
        before = n.rectangle(editor)
        outer = n.rectangle(doc)
        n.set_text(button, "Load metadata with a substantially longer action label")
        n.send(app.frame, win32con.WM_SETTINGCHANGE)  # without changing global settings
        assert n.rectangle(doc) == outer, "free-placement document moved"
        assert n.rectangle(editor)[1] > before[1], "long label did not trigger wrapping"
        assert n.send(button, win32con.WM_GETFONT) != 0
        height, face = app.declaration_font(doc)
        assert height > 0 and face == "Courier New"
        assert app.content(doc) == content
        assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2
        assert n.set_pos(doc, None, 0, 0, 280, 340,
                         win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
        controls = [n.get_item(doc, id) for id in (1020, 1022, 1023, 1043, 1044)]
        assert all(h and n.is_visible(h) for h in controls)
        rectangles = [n.rectangle(h) for h in controls]
        bounds = n.rectangle(editor)
        for i, r in enumerate(rectangles):
            assert bounds[0] <= r[0] <= r[2] <= bounds[2]
            assert r[3] <= bounds[1]
            for other in rectangles[i + 1:]:
                assert r[2] <= other[0] or other[2] <= r[0] or r[3] <= other[1] or other[3] <= r[1]


def test_mdi_maximize_inheritance_and_one_shot_arrangement(launch, settings):
    app = launch(["--winmd", settings[1]])
    zoomed = n.api(n.user, "IsZoomed", n.w.BOOL, n.w.HWND)
    app.query("CreateFileW")
    original = app.open_row()
    n.send(app.mdi, win32con.WM_MDIMAXIMIZE, original)
    assert zoomed(original)
    app.open_new()
    n.wait(lambda: len(app.documents()) == 2, "maximized new document")
    other = next(h for h in app.documents() if h != original)
    assert zoomed(other), "new document must inherit MDI maximization"
    app.command(1010)  # one-shot vertical tile
    assert not any(zoomed(h) for h in app.documents())
    before = {h: n.rectangle(h) for h in app.documents()}
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

    toolbar = n.get_item(app.frame, 1113)

    def click_panel(index):
        assert n.send(toolbar, messages.TB_GETITEMRECT, index, app.memory)
        buffer = n.c.create_string_buffer(16)
        assert n.read_memory(app.handle, app.memory, buffer, 16, None)
        left, top, right, bottom = struct.unpack("<4i", buffer.raw)
        point = ((left + right) // 2) | (((top + bottom) // 2) << 16)
        n.send(toolbar, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
        n.send(toolbar, win32con.WM_LBUTTONUP, 0, point)

    click_panel(1)
    n.wait(lambda: app.focused() == app.sources, "mouse activation delegates focus to sources")
    n.send(app.frame, win32con.WM_SIZE)  # ordinary layout
    n.send(app.frame, win32con.WM_SETTINGCHANGE)  # environment/font refresh
    assert app.focused() == app.sources, "layout and environment updates must not move focus"
    click_panel(1)
    n.wait(lambda: app.focused() == toolbar, "collapse focuses toolbar")
    click_panel(0)
    n.wait(lambda: app.focused() == app.search, "mouse activation delegates focus to search")
    n.send(toolbar, messages.TB_SETHOTITEM, 1)
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: app.focused() == app.sources, "keyboard activation delegates focus to sources")
    n.key_message(toolbar, win32con.VK_SPACE)
    n.wait(lambda: app.focused() == toolbar, "keyboard collapse focuses toolbar")


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
    with n.window_dpi_context(app.frame):
        left, top, right, bottom = n.rectangle(doc)
        assert right > left and bottom > top
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

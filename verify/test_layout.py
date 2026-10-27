from support import native as n
import struct


def test_toolbar_and_search_geometry(launch):
    app = launch()
    with n.window_dpi_context(app.frame):
        check_toolbar_and_search(app)


def check_toolbar_and_search(app):
    toolbar = n.get_item(app.frame, 1113)
    assert n.class_name(toolbar) == "ToolbarWindow32"
    assert not n.get_item(app.frame, 1040)
    assert not n.get_item(app.frame, 1021)
    assert n.send(toolbar, 0x418) == 2  # TB_BUTTONCOUNT

    def button_rect(index):
        assert n.send(toolbar, 0x41D, index, app.memory)  # TB_GETITEMRECT
        buffer = n.c.create_string_buffer(16)
        assert n.read_memory(app.handle, app.memory, buffer, 16, None)
        return struct.unpack("<4i", buffer.raw)

    first, second = button_rect(0), button_rect(1)
    area = n.rectangle(toolbar, client=True)
    assert first[0] == second[0] and first[3] <= second[1]
    assert 0 <= first[0] < first[2] <= area[2]
    assert 0 <= second[1] < second[3] <= area[3]
    assert n.send(toolbar, 0x40A, 1040)  # TB_ISBUTTONCHECKED
    assert not n.send(toolbar, 0x40A, 1021)

    def click(rect):
        x, y = (rect[0] + rect[2]) // 2, (rect[1] + rect[3]) // 2
        position = x | (y << 16)
        n.send(toolbar, 0x201, 1, position)
        n.send(toolbar, 0x202, 0, position)

    click(second)  # Message-driven toolbar activation, not actual mouse input.
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    assert n.send(toolbar, 0x40A, 1021) and not n.send(toolbar, 0x40A, 1040)
    click(second)
    assert not n.is_visible(app.sources)
    assert not n.send(toolbar, 0x40A, 1021)
    click(first)
    assert n.is_visible(app.results) and n.send(toolbar, 0x40A, 1040)
    n.send(toolbar, 0x448, 0)  # TB_SETHOTITEM
    n.key_message(toolbar, 32)
    n.wait(lambda: not n.is_visible(app.results), "toolbar Space collapses panel")
    n.key_message(toolbar, 32)
    n.wait(lambda: n.is_visible(app.results), "toolbar Space expands panel")
    n.key_message(toolbar, 40)
    n.wait(lambda: n.send(toolbar, 0x447) == 1, "toolbar Down selects next item")
    n.key_message(toolbar, 32)
    n.wait(lambda: n.is_visible(app.sources), "toolbar keyboard switches panel")
    assert n.send(toolbar, 0x40A, 1021) and not n.send(toolbar, 0x40A, 1040)
    click(first)

    get_style = n.api(n.user, "GetWindowLongW", n.c.c_long, n.w.HWND, n.c.c_int)
    assert get_style(app.search, -20) & 0x200  # WS_EX_CLIENTEDGE
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
    n.wait(lambda: '"parameters"' in app.content(original), "declaration", 45)
    old = app.content(original)
    app.command(1047, original)
    n.wait(lambda: len(app.documents()) == 2, "other architecture")
    other = next(h for h in app.documents() if h != original)
    n.wait(lambda: '"parameters"' in app.content(other), "other declaration", 45)
    assert ("x86" in n.window_text(original)) != ("x86" in n.window_text(other))
    assert app.content(original) == old
    app.command(1047, other)
    app.command(1047, original)
    assert len(app.documents()) == 2
    for doc in app.documents():
        for removed in (1201, 1202, 1204):
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
    editor = n.get_item(doc, 1203)
    button = n.get_item(doc, 1020)
    content = app.content(doc)
    n.send(editor, 0xB1, 2, 8)
    with n.window_dpi_context(app.frame):
        before = n.rectangle(editor)
        outer = n.rectangle(doc)
        n.set_text(button, "Load metadata with a substantially longer action label")
        n.send(app.frame, 0x1A)  # WM_SETTINGCHANGE, without changing global settings.
        assert n.rectangle(doc) == outer, "free-placement document moved"
        assert n.rectangle(editor)[1] > before[1], "long label did not trigger wrapping"
        assert n.send(editor, 0x31) == n.send(button, 0x31) != 0  # WM_GETFONT
        assert app.content(doc) == content
        assert n.send(editor, 0xB0) == (8 << 16) | 2
        assert n.set_pos(doc, None, 0, 0, 280, 340, 0x16)
        controls = [n.get_item(doc, id) for id in (1020, 1022, 1023, 1043, 1044)]
        assert all(h and n.is_visible(h) for h in controls)
        rectangles = [n.rectangle(h) for h in controls]
        bounds = n.rectangle(editor)
        for i, r in enumerate(rectangles):
            assert bounds[0] <= r[0] <= r[2] <= bounds[2]
            assert r[3] <= bounds[1]
            for other in rectangles[i + 1:]:
                assert r[2] <= other[0] or other[2] <= r[0] or r[3] <= other[1] or other[3] <= r[1]

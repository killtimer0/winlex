import shutil
import winreg
import win32con
import win32gui
from support.diagnostics import capture, screenshot
from support import native as n


def test_sources_share_two_line_paint_and_native_selection(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    with n.window_dpi_context(app.frame):
        _, top, _, bottom = app.item_rect(0)
        result_height = bottom - top
    app.command(1021)
    n.wait(lambda: win32gui.IsWindowVisible(app.sources), "sources visible")
    with n.window_dpi_context(app.frame):
        left, top, right, bottom = app.item_rect(0, app.sources)
        assert bottom - top == result_height
        with capture(app.sources, client=True) as (dc, _, _):
            background = win32gui.GetSysColor(win32con.COLOR_WINDOW)
            midpoint = (top + bottom) // 2
            for start, end in [(top, midpoint), (midpoint, bottom)]:
                assert any(win32gui.GetPixel(dc, x, y) != background
                           for x in range(left, min(right, left + 200))
                           for y in range(start, end)), "missing source line"
    assert str(settings[1]) in app.cell(0, app.sources)
    app.select(0, app.sources)
    assert app.selected_row(app.sources) == 0
    screenshot(app.sources, settings[2] / "source-row-selected.bmp")
    app.command(1044)
    n.wait(lambda: n.c.c_int(app.selected_row(app.sources)).value == -1
           and app.cell(0, app.sources).splitlines()[1].startswith(("Loaded", "已加载")),
           "source reloaded", 45)
    assert app.count(app.sources) == 1
    assert str(settings[1]) in app.cell(0, app.sources)


def test_open_loads_and_close_keeps_document_snapshot(launch, profile, settings):
    app = launch()
    app.pick([settings[1]])
    n.wait(lambda: app.count(app.sources) == 1, "file loaded", 45)
    n.wait(lambda: profile.get("AutoLoad") == [str(settings[1])], "successful files saved")
    app.query("CreateFileW")
    entity = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(entity), "declaration", 45)
    old = app.content(entity)
    editor = n.get_item(entity, 1203)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    app.command(1021)
    app.select(0, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0 and profile.get("AutoLoad") == [], "source closed")
    assert app.count() == 0
    assert app.content(entity) == old
    assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2
    app.choose(entity, "arm64")
    n.wait(lambda: 'CreateFileW(' in app.content(entity), "query from closed snapshot", 45)
    assert "arm64" in n.window_text(entity)
    assert app.documents() == [entity]
    app.close()
    restored = launch()
    assert restored.count(restored.sources) == 0


def test_restore_successful_files_ignores_legacy_discovery_locations(launch, profile, settings):
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    profile.set("Files", winreg.REG_MULTI_SZ, [str(settings[2] / "not-opened.winmd")])
    profile.set("Directories", winreg.REG_MULTI_SZ, [str(settings[1].parent)])
    app = launch()
    n.wait(lambda: app.count(app.sources) == 1, "saved source loaded", 45)
    assert str(settings[1]) in app.cell(0, app.sources)
    app.query("CreateFileW")
    assert app.count() > 0
    app.close()
    restored = launch()
    n.wait(lambda: restored.count(restored.sources) == 1, "source restored again", 45)


def test_temporary_inputs_preserve_saved_files(launch, profile, settings):
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch([str(settings[2] / "missing.winmd")])
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(), "load error")
    assert app.count(app.sources) == 0
    assert profile.get("AutoLoad") == [str(settings[1])]
    app.close()
    temporary = launch([settings[1]])
    n.wait(lambda: temporary.count(temporary.sources) == 1, "temporary input loaded", 45)
    temporary.select(0, temporary.sources)
    temporary.command(1022)
    n.wait(lambda: temporary.count(temporary.sources) == 0, "temporary source closed")
    assert profile.get("AutoLoad") == [str(settings[1])]


def test_failed_restore_preserves_saved_files(launch, profile, settings):
    missing = str(settings[2] / "missing-restored.winmd")
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [missing])
    app = launch()
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(), "restore error")
    assert profile.get("AutoLoad") == [missing]
    assert app.count(app.sources) == 0


def test_multiple_positional_inputs_preserve_order(launch, profile, settings):
    first = settings[2] / "first source.winmd"
    second = settings[2] / "第二个来源.winmd"
    for path in (first, second):
        shutil.copyfile(settings[1], path)
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch([first, second])
    n.wait(lambda: app.count(app.sources) == 2, "both inputs loaded", 45)
    assert str(first) in app.cell(0, app.sources)
    assert str(second) in app.cell(1, app.sources)
    assert profile.get("AutoLoad") == [str(settings[1])]
    app.query("CreateFileW")
    assert len(app.named_rows("CreateFileW")) == 2


def test_long_unicode_input(launch, settings):
    from support.paths import extended
    directory = settings[2] / ("长路径" * 40) / ("metadata-" * 12)
    extended(directory).mkdir(parents=True, exist_ok=True)
    path = directory / "来源.winmd"
    assert len(str(path)) > 260
    shutil.copyfile(settings[1], extended(path))
    app = launch([path])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "long path loaded", 45)
    app.query("CreateFileW")
    assert app.cell(0, app.sources).splitlines()[0] == str(path)
    assert app.cell(0).splitlines()[3] == str(path)
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "declaration", 45)
    assert str(path) in app.content(doc)


def test_list_text_preserves_native_utf16_path(launch, settings):
    path = settings[2] / "source-\ud800.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    try:
        app.query("CreateFileW")
        assert app.cell(0, app.sources).splitlines()[0] == str(path)
        lines = app.cell(0).splitlines()
        assert lines[1].endswith(path.name)
        assert lines[3] == str(path)
    finally:
        app.close()
        path.unlink()

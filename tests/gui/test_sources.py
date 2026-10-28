import shutil
import winreg
import win32con
from support import native as n


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
    app = launch(["--winmd", str(settings[2] / "missing.winmd")])
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(), "load error")
    assert app.count(app.sources) == 0
    assert profile.get("AutoLoad") == [str(settings[1])]
    app.close()
    temporary = launch(["--winmd", settings[1]])
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


def test_long_unicode_input(launch, settings):
    directory = settings[2] / ("长路径" * 40) / ("metadata-" * 12)
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "来源.winmd"
    assert len(str(path)) > 260
    shutil.copyfile(settings[1], path)
    app = launch(["--winmd", path])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "long path loaded", 45)
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "declaration", 45)
    app.command(1048, doc)
    assert str(path) in n.window_text(n.get_item(doc, 1205))

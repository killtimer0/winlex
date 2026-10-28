import shutil
import win32con
from support import native as n


def test_reload_keeps_old_document_snapshot(launch, settings):
    path = settings[2] / "version.winmd"
    shutil.copyfile(settings[1], path)
    app = launch(["--winmd", path])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("CreateFileW")
    entity = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(entity), "old declaration", 45)
    old = app.content(entity)
    app.command(1021)
    assert app.count(app.sources) == 1
    app.select(0, app.sources)
    editor = n.get_item(entity, 1203)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    app.command(1044)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "reload", 45)
    n.wait(lambda: "Earlier catalog snapshot" in n.window_text(n.get_item(entity, 1204)) or "先前的目录快照" in n.window_text(n.get_item(entity, 1204)), "earlier snapshot", 45)
    assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2
    app.command(1040)
    app.query("CreateFileW")
    current = app.open_row()
    assert current != entity
    n.wait(lambda: 'CreateFileW(' in app.content(current), "current declaration", 45)
    header = n.window_text(n.get_item(current, 1204))
    assert "Earlier catalog snapshot" not in header
    assert "先前的目录快照" not in header
    assert app.content(entity) == old
    assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2

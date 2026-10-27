import shutil
from support import native as n


def test_reload_keeps_old_document_snapshot(launch, settings):
    path = settings[2] / "version.winmd"
    shutil.copyfile(settings[1], path)
    app = launch(["--winmd", path])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("CreateFileW")
    entity = app.open_row()
    n.wait(lambda: '"parameters"' in app.content(entity), "old declaration", 45)
    old = app.content(entity)
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "discovery")
    source = app.open_row(source=True)
    n.wait(lambda: '"sources"' in app.content(source), "inspect", 45)
    app.command(1044, source)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "reload", 45)
    n.wait(lambda: "Earlier catalog snapshot" in app.content(entity) or "先前的目录快照" in app.content(entity), "earlier snapshot", 45)
    before = set(app.documents())
    app.command(1045, entity)
    current = n.wait(lambda: next(iter(set(app.documents()) - before), None), "open current")
    n.wait(lambda: '"parameters"' in app.content(current), "current declaration", 45)
    assert "Earlier catalog snapshot" not in app.content(current)
    assert "先前的目录快照" not in app.content(current)
    assert old in app.content(entity)

import shutil
from support import native as n


def test_same_name_sources_open_and_unload_independently_with_failed_load_rollback(launch, profile, settings):
    directory = settings[2] / "independent"
    directory.mkdir()
    alias = directory / settings[1].name
    corrupt = settings[2] / "corrupt.winmd"
    shutil.copyfile(settings[1], alias)
    corrupt.write_bytes(b"not metadata")
    app = launch()
    app.pick([settings[1], alias])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("CreateFileW")
    assert app.named_rows("CreateFileW") == [0, 1]
    rows = [app.cell(i) for i in range(2)]
    assert rows[0].splitlines()[0] == rows[1].splitlines()[0]
    assert rows[0].splitlines()[-1] != rows[1].splitlines()[-1]
    first = app.open_row(0)
    n.wait(lambda: 'CreateFileW(' in app.content(first), "first declaration", 45)
    second = app.open_row(1)
    n.wait(lambda: 'CreateFileW(' in app.content(second), "second declaration", 45)
    assert first != second
    assert n.window_text(first) == n.window_text(second)
    assert str(alias) in app.content(second)
    app.command(1021)
    assert app.count(app.sources) == 2
    saved = profile.get("AutoLoad")
    assert saved == [str(settings[1]), str(alias)]
    app.pick([corrupt])
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(), "load rollback", 45)
    assert profile.get("AutoLoad") == saved
    assert app.count(app.sources) == 2
    app.command(1040)
    app.query("CreateFileW")
    assert app.named_rows("CreateFileW") == [0, 1]
    app.command(1021)
    alias_row = next(i for i in range(2) if str(alias) in app.cell(i, app.sources))
    app.select(alias_row, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 1, "only selected location removed", 45)
    app.query("CreateFileW")
    assert app.named_rows("CreateFileW") == [0]
    assert profile.get("AutoLoad") == [str(settings[1])]
    assert 'CreateFileW(' in app.content(second)

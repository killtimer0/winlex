import shutil
import winreg
from support import native as n


def test_duplicate_content_unloads_independently_and_failed_load_rolls_back(launch, profile, settings):
    alias = settings[2] / "副本.winmd"
    corrupt = settings[2] / "corrupt.winmd"
    shutil.copyfile(settings[1], alias)
    corrupt.write_bytes(b"not metadata")
    profile.set("Files", winreg.REG_MULTI_SZ, [str(settings[1]), str(alias), str(corrupt)])
    app = launch(["--winmd", settings[1], "--winmd", alias])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("CreateFileW")
    assert app.count() == 2
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 3, "discovery", 45)
    bad_row = next(i for i in range(3) if "corrupt.winmd" in app.cell(i, app.sources))
    bad = app.open_row(bad_row, source=True)
    n.wait(lambda: "Invalid" in app.content(bad) or "invalid" in app.content(bad), "parse failure", 45)
    app.command(1020, bad)
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(), "load rollback", 45)
    assert str(corrupt) in profile.get("AutoLoad")
    app.command(1040)
    app.query("CreateFileW")
    assert app.count() == 2
    app.command(1021)
    alias_row = next(i for i in range(3) if str(alias) in app.cell(i, app.sources))
    source = app.open_row(alias_row, source=True)
    n.wait(lambda: '"sources"' in app.content(source), "alias inspection", 45)
    app.command(1022, source)
    n.wait(lambda: app.count() == 1, "only selected location removed", 45)
    assert profile.get("AutoLoad") == [str(corrupt)]


def test_same_name_sources_remain_distinct(launch, settings):
    directory = settings[2] / "independent"
    directory.mkdir()
    independent = directory / settings[1].name
    shutil.copyfile(settings[1], independent)
    app = launch(["--winmd", settings[1], "--winmd", independent])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "independent sources loaded", 45)
    app.query("CreateFileW")
    assert app.count() == 2
    rows = [app.cell(i) for i in range(2)]
    assert rows[0].splitlines()[0] == rows[1].splitlines()[0]
    assert rows[0].splitlines()[-1] != rows[1].splitlines()[-1]
    first = app.open_row(0)
    n.wait(lambda: 'CreateFileW(' in app.content(first), "first declaration", 45)
    second = app.open_row(1)
    n.wait(lambda: 'CreateFileW(' in app.content(second), "second declaration", 45)
    assert first != second
    assert n.window_text(first) == n.window_text(second)
    assert str(independent) in n.window_text(n.get_item(second, 1205))

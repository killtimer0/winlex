import winreg
import shutil
from support import native as n


def test_discovery_restores_selection_by_location(launch, profile, settings):
    directory = settings[2] / "selection"
    directory.mkdir()
    for name in ("b.winmd", "c.winmd"):
        (directory / name).write_bytes(name.encode())
    profile.set("Directories", winreg.REG_MULTI_SZ, [str(directory)])
    app = launch()
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 3, "initial locations")
    selected = directory / "b.winmd"
    row = next(i for i in range(3) if str(selected) in app.cell(i, app.sources))
    app.select(row, app.sources)
    (directory / "a.winmd").write_bytes(b"new earlier location")
    app.command(1042)
    n.wait(lambda: app.count(app.sources) == 4, "inserted location")
    row = n.c.c_int(n.send(app.sources, 0x100C, -1, 2)).value  # LVM_GETNEXTITEM / LVNI_SELECTED
    assert row >= 0 and str(selected) in app.cell(row, app.sources)
    selected.unlink()
    app.command(1042)
    n.wait(lambda: app.count(app.sources) == 3, "removed selected location")
    assert n.c.c_int(n.send(app.sources, 0x100C, -1, 2)).value == -1


def test_discovery_is_not_loading(launch, profile, settings):
    profile.set("Files", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch()
    n.wait(lambda: app.count(app.sources) == 1, "source discovery")
    app.command(1021)
    doc = app.open_row(source=True)
    for removed in (1201, 1202, 1204, 1047):
        assert not n.get_item(doc, removed)
    n.wait(lambda: '"sources"' in app.content(doc), "inspection", 45)
    n.send(doc, 0x112, 0xF020)  # WM_SYSCOMMAND / SC_MINIMIZE
    is_iconic = n.api(n.user, "IsIconic", n.w.BOOL, n.w.HWND)
    assert is_iconic(doc)
    app.select(0, app.sources)
    n.key_message(app.sources, 13)
    n.wait(lambda: not is_iconic(doc), "restore existing source document")
    assert app.documents() == [doc]
    assert profile.get("AutoLoad") is None
    app.command(1020, doc)
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "source loaded", 45)
    assert profile.get("AutoLoad") == [str(settings[1])]
    app.command(1040)
    app.query("CreateFileW")
    entity = app.open_row()
    n.wait(lambda: '"parameters"' in app.content(entity), "declaration", 45)
    old = app.content(entity)
    declaration = n.get_item(entity, 1203)
    n.send(declaration, 0xB1, 2, 8)  # EM_SETSEL
    app.command(1022, doc)
    n.wait(lambda: profile.get("AutoLoad") == [], "unload persisted")
    assert app.content(entity) == old
    assert n.send(declaration, 0xB0) == (8 << 16) | 2  # EM_GETSEL
    app.command(1047, entity)
    n.wait(lambda: len(app.documents()) == 3, "unloaded snapshot document")
    other = next(h for h in app.documents() if h not in (doc, entity))
    n.wait(lambda: '"parameters"' in app.content(other), "unloaded snapshot architecture query", 45)
    assert ("x86" in n.window_text(entity)) != ("x86" in n.window_text(other))
    assert app.content(entity) == old


def test_native_add_is_discovery_only(launch, profile, settings):
    app = launch()
    app.pick([])
    assert profile.get("Files") is None
    app.pick([settings[1]])
    n.wait(lambda: app.count(app.sources) == 1, "native add discovery", 45)
    assert profile.get("AutoLoad") is None
    assert profile.get("Files") == [str(settings[1])]


def test_temporary_inputs_preserve_intent(launch, profile, settings):
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch(["--winmd", str(settings[2] / "missing.winmd")])
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(), "load error")
    assert profile.get("AutoLoad") == [str(settings[1])]
    entries = [app.cell(i, app.sources) for i in range(app.count(app.sources))]
    missing = next(value for value in entries if "missing.winmd" in value)
    assert "Missing" in missing or "缺失" in missing


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
    n.wait(lambda: '"parameters"' in app.content(doc), "declaration", 45)
    assert str(path) in app.content(doc)


def test_directory_discovery_is_nonrecursive_and_removable(launch, profile, settings):
    directory = settings[2] / "directory"
    nested = directory / "nested"
    nested.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(settings[1], directory / "visible.winmd")
    shutil.copyfile(settings[1], nested / "hidden.winmd")
    profile.set("Directories", winreg.REG_MULTI_SZ, [str(directory)])
    app = launch()
    n.wait(lambda: app.count(app.sources) == 2, "directory and immediate file", 45)
    rows = [app.cell(i, app.sources) for i in range(2)]
    assert any("visible.winmd" in row for row in rows)
    assert not any("hidden.winmd" in row for row in rows)
    assert profile.get("AutoLoad") is None
    index = next(i for i, row in enumerate(rows) if row.splitlines()[0] == str(directory))
    app.select(index, app.sources)
    app.command(1046)
    n.wait(lambda: app.count(app.sources) == 0, "location forgotten", 45)
    assert profile.get("Directories") == []

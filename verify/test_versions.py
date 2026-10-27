import shutil
import struct
from support import native as n


def test_changed_source_keeps_old_document(launch, profile, settings):
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
    data = bytearray(path.read_bytes())
    data[struct.unpack_from("<I", data, 0x3c)[0] + 8] ^= 1
    path.write_bytes(data)
    app.command(1042)
    n.wait(lambda: "Old version" in app.content(entity) or "旧版本" in app.content(entity), "same-size change discovered", 45)
    assert old in app.content(entity)
    app.command(1044, source)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "reload", 45)
    before = set(app.documents())
    app.command(1045, entity)
    current = n.wait(lambda: next(iter(set(app.documents()) - before), None), "open current")
    n.wait(lambda: '"parameters"' in app.content(current), "current declaration", 45)
    assert app.content(current) != old
    assert old in app.content(entity)

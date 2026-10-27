from support import native as n


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

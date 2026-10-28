"""Transient documents: single-click previews, explicit opens keep them."""
import win32con

from support import native as n


def title(app, row=0):
    return app.cell(row).splitlines()[0].rsplit(".", 1)[-1]


def body(app, doc):
    return n.get_item(doc, 1203)


def test_single_click_replaces_the_preview_in_place(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    name = title(app)
    first = app.preview_row()
    assert app.tab_labels()[0].startswith(name + " — x86 · ")
    n.wait(lambda: "CreateFileW(" in app.content(first), "declaration", 45)

    app.query("CloseHandle")
    name = title(app)
    second = app.preview_row()
    assert second == first
    assert len(app.documents()) == 1
    assert n.window_text(second) == name + " — x86"
    assert app.tab_labels()[0].startswith(name + " — x86 · ")


def test_preview_keeps_the_selected_architecture(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row()
    app.choose(doc, "x64")
    n.wait(lambda: "x64" in n.window_text(doc), "x64 selection")

    app.query("CloseHandle")
    assert app.preview_row() == doc
    assert app.choice(doc) == "x64"
    assert len(app.documents()) == 1


def test_explicit_open_pins_the_preview(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row()
    assert " · " in app.tab_labels()[0]

    # Enter is handled by the UI loop, so wait for the tab marker to disappear.
    app.select(0)
    n.key_message(app.results, win32con.VK_RETURN)
    n.wait(lambda: " · " not in app.tab_labels()[0], "pinned")
    assert app.documents() == [doc]

    app.query("CloseHandle")
    app.preview_row()
    n.wait(lambda: len(app.documents()) == 2, "preview next to the pinned document")
    assert "CreateFileW(" in app.content(doc)


def test_preview_rebinds_architecture_choices_and_body(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row()
    app.choose(doc, "x64")

    app.query("WHV_ARM64_PENDING_EVENT_TYPE")
    assert app.preview_row() == doc
    assert app.choices(app.architecture(doc)) == ["arm64"]
    assert app.choice(doc) == "arm64"
    assert n.window_text(doc).endswith(" — arm64")
    n.wait(lambda: "enum WHV_ARM64_PENDING_EVENT_TYPE" in app.content(doc), "ARM64 preview", 45)

    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    assert app.preview_row() == doc
    assert app.choices(app.architecture(doc)) == ["x86", "x64", "arm64"]
    assert app.choice(doc) == "arm64"
    n.wait(lambda: "// [arm64] size: 24(0x18) bytes" in app.content(doc), "inherited ARM64 preview", 45)
    app.choose(doc, "x86")
    n.wait(lambda: "// [x86] size: 12(0xC) bytes" in app.content(doc), "rebound selector", 45)


def test_reopening_an_open_document_activates_it(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    name = title(app)
    doc = app.open_row()
    assert app.open_row() == doc
    assert len(app.documents()) == 1

    app.command(1049)  # File > Open in new document
    n.wait(lambda: len(app.documents()) == 2, "forced document")
    assert sorted(app.tab_labels()) == [name + " — x86", name + " — x86 (2)"]


def test_preview_inherits_the_architecture_being_read(launch, profile, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "x64" in n.window_text(doc), "x64 selection")

    app.query("WHV_ARM64_PROCESSOR_FEATURES")
    arm = app.open_row()
    n.wait(lambda: app.choices(app.architecture(arm)) == ["arm64"], "arm64 only")
    assert app.choice(arm) == "arm64"

    app.query("CreateFileW")
    preview = app.preview_row()
    n.wait(lambda: app.choice(preview) == "arm64", "preview inherits the current architecture")
    assert profile.get("PreferredArchitecture") == "x64"
    assert "x64" in n.window_text(doc)


def test_space_previews_without_leaving_the_list(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)

    app.query("CloseHandle")
    name = title(app)
    app.focus(app.results)
    app.select(0)
    n.key_message(app.results, win32con.VK_SPACE)
    n.wait(lambda: n.window_text(doc) == name + " — x86", "space preview")
    assert len(app.documents()) == 1
    assert " · " in app.tab_labels()[0]
    assert app.focused() == app.results


def test_arrow_keys_only_move_the_selection(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFile")
    doc = app.preview_row()
    n.wait(lambda: "CREATEFILE" in app.content(doc), "declaration", 45)
    text = app.content(doc)
    label = app.tab_labels()[0]

    app.focus(app.results)
    app.select(0)
    first = app.selected_row()
    n.key_message(app.results, win32con.VK_DOWN)
    n.wait(lambda: app.selected_row() == first + 1, "selection moved")

    assert len(app.documents()) == 1
    assert app.active_document() == doc
    assert app.content(doc) == text
    assert app.tab_labels()[0] == label
    assert app.focused() == app.results


def test_preview_keeps_the_list_focus_and_opening_enters_the_body(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    app.focus(app.results)
    doc = app.preview_row()
    n.wait(lambda: app.focused() == app.results, "preview keeps the list focus")

    app.query("CloseHandle")
    app.focus(app.results)
    assert app.preview_row() == doc
    n.wait(lambda: app.focused() == app.results, "replacement keeps the list focus")

    app.select(0)
    n.key_message(app.results, win32con.VK_RETURN)  # Pin the preview.
    n.wait(lambda: " · " not in app.tab_labels()[0], "pinned")
    n.wait(lambda: app.focused() == body(app, doc), "enter focuses the body")

    app.query("CreateFileW")
    app.focus(app.results)
    opened = app.double_click_row(0)
    n.wait(lambda: len(app.documents()) == 2, "double click opens a document")
    n.wait(lambda: app.focused() == body(app, opened), "double click focuses the body")

    app.focus(app.results)
    app.command(1050)  # Open selected result activates the existing document.
    n.wait(lambda: app.focused() == body(app, opened), "menu open focuses the body")

    app.focus(app.results)
    app.command(1049)  # Open in new document
    n.wait(lambda: len(app.documents()) == 3, "new document")
    n.wait(lambda: app.focused() == body(app, app.active_document()),
           "forced open focuses the new body")

"""Transient documents: single-click previews, explicit opens keep them."""
import win32con

from support import native as n


def title(app, row=0):
    return app.cell(row).splitlines()[0].rsplit(".", 1)[-1]


def body(app, doc):
    return n.get_item(doc, 1203)


def test_single_click_replaces_preview_preserving_architecture_and_focus(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    name = title(app)
    app.focus(app.results)
    first = app.preview_row()
    n.wait(lambda: app.focused() == app.results, "preview keeps the list focus")
    assert app.tab_labels()[0] == name + " - x86"
    assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))
    n.wait(lambda: "CreateFileW(" in app.content(first), "declaration", 45)
    app.choose(first, "x64")

    app.query("CloseHandle")
    name = title(app)
    app.focus(app.results)
    second = app.preview_row()
    n.wait(lambda: app.focused() == app.results, "replacement keeps the list focus")
    assert second == first
    assert len(app.documents()) == 1
    assert app.choice(second) == "x64"
    assert n.window_text(second) == name + " - x64"
    assert app.tab_labels()[0] == name + " - x64"
    assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))
    n.wait(lambda: name in app.content(second), "replacement declaration", 45)


def test_preview_rebinds_architecture_choices_and_body(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row()
    app.choose(doc, "x64")

    app.query("WHV_ARM64_PENDING_EVENT_TYPE")
    assert app.preview_row() == doc
    assert app.choices(app.architecture(doc)) == ["arm64"]
    assert app.choice(doc) == "arm64"
    assert n.window_text(doc).endswith(" - arm64")
    n.wait(lambda: "enum WHV_ARM64_PENDING_EVENT_TYPE" in app.content(doc), "ARM64 preview", 45)

    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    assert app.preview_row() == doc
    assert app.choices(app.architecture(doc)) == ["x86", "x64", "arm64"]
    assert app.choice(doc) == "arm64"
    n.wait(lambda: "// [arm64] size: 24(0x18) bytes" in app.content(doc), "inherited ARM64 preview", 45)
    app.choose(doc, "x86")
    n.wait(lambda: "// [x86] size: 12(0xC) bytes" in app.content(doc), "rebound selector", 45)


def test_preview_inherits_the_architecture_being_read(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "x64" in n.window_text(doc), "x64 selection")

    app.query("WHV_ARM64_PENDING_EVENT_TYPE")
    arm = app.open_row()
    n.wait(lambda: app.choices(app.architecture(arm)) == ["arm64"], "arm64 only")
    assert app.choice(arm) == "arm64"
    assert n.window_text(arm).endswith(" - arm64")
    n.wait(lambda: "enum WHV_ARM64_PENDING_EVENT_TYPE" in app.content(arm),
           "ARM64-only declaration", 45)

    app.query("CreateFileW")
    preview = app.preview_row()
    n.wait(lambda: app.choice(preview) == "arm64", "preview inherits the current architecture")
    assert profile.get("PreferredArchitecture") == "x64"
    assert "x64" in n.window_text(doc)


def test_space_previews_without_leaving_the_list(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)

    app.query("CloseHandle")
    name = title(app)
    app.focus(app.results)
    app.select(0)
    n.key_message(app.results, win32con.VK_SPACE)
    n.wait(lambda: n.window_text(doc) == name + " - x86", "space preview")
    assert len(app.documents()) == 1
    assert app.tab_labels()[0] == name + " - x86"
    assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))
    assert app.focused() == app.results


def test_arrow_keys_only_move_the_selection(launch, settings):
    app = launch([settings[1]])
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


def test_pinning_and_open_commands_preserve_documents_and_focus(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    app.focus(app.results)
    doc = app.preview_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration before pinning", 45)
    pinned_body = app.content(doc)

    app.select(0)
    n.key_message(app.results, win32con.VK_RETURN)  # Pin the preview.
    n.wait(lambda: not app.tab_descriptions()[0].startswith(("Preview · ", "预览 · ")), "pinned")
    n.wait(lambda: app.focused() == body(app, doc), "enter focuses the body")
    assert app.documents() == [doc]

    app.query("CloseHandle")
    name = title(app)
    app.focus(app.results)
    preview = app.preview_row()
    assert preview != doc and len(app.documents()) == 2
    n.wait(lambda: name in app.content(preview), "preview beside pinned document", 45)
    assert app.content(doc) == pinned_body
    n.send(preview, win32con.WM_CLOSE)
    n.wait(lambda: app.documents() == [doc], "close transient preview")
    app.focus(app.results)
    opened = app.double_click_row(0)
    n.wait(lambda: len(app.documents()) == 2, "double click opens a document")
    assert app.tab_labels()[1] == name + " - x86"
    n.wait(lambda: app.focused() == body(app, opened), "double click focuses the body")

    n.send(app.mdi, win32con.WM_MDIACTIVATE, doc)
    assert app.active_document() == doc
    app.focus(app.results)
    app.command(1050)  # Open selected result activates the existing document.
    n.wait(lambda: app.focused() == body(app, opened), "menu open focuses the body")
    assert app.active_document() == opened and len(app.documents()) == 2

    app.focus(app.results)
    app.command(1049)  # Open in new document
    n.wait(lambda: len(app.documents()) == 3, "new document")
    n.wait(lambda: app.focused() == body(app, app.active_document()),
           "forced open focuses the new body")
    assert name + " - x86 (2)" in app.tab_labels()

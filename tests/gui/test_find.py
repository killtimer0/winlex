"""Document find uses native selections without adding navigation visits."""
import win32con
from support import native as n
from test_navigation import editor_selection
from test_history import enabled, HISTORY_BACK


def test_document_find_and_preview_reuse(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row(app.named_rows("CreateFileW")[0])
    n.wait(lambda: "CreateFileW" in app.content(doc), "declaration loaded", 45)
    editor = n.get_item(doc, 1203)
    query = n.get_item(doc, 1220)
    count = n.get_item(doc, 1223)
    assert not enabled(app, HISTORY_BACK)
    assert not n.is_visible(query)
    app.command(1060)
    n.wait(lambda: n.is_visible(query) and app.focused() == query, "find opened")
    n.set_text(query, "HANDLE")
    n.wait(lambda: n.window_text(count).startswith("1/"), "first match")
    first = editor_selection(app, editor)
    assert first[1] - first[0] == 6
    total = int(n.window_text(count).split("/")[1].split()[0])
    assert total > 1
    n.key_message(query, win32con.VK_RETURN)
    n.wait(lambda: editor_selection(app, editor)[0] > first[0], "Enter finds next")
    app.command(1062)
    assert editor_selection(app, editor) == first
    app.command(1062)
    assert n.window_text(count).startswith(f"{total}/{total}")
    assert editor_selection(app, editor)[0] > first[0]
    n.set_text(query, "not_present_anywhere_12345")
    n.wait(lambda: n.window_text(count).startswith("0/0"), "no matches")
    n.key_message(query, win32con.VK_ESCAPE)
    n.wait(lambda: not n.is_visible(query) and app.focused() == editor, "find dismissed")
    assert not enabled(app, HISTORY_BACK)
    app.command(1060)
    assert n.window_text(query) == "HANDLE"
    n.set_text(query, "HANDLE")
    saved = editor_selection(app, editor)
    app.query("CloseHandle")
    reused = app.preview_row(app.named_rows("CloseHandle")[0])
    assert reused == doc
    n.wait(lambda: "CloseHandle" in app.content(doc), "preview replaced", 45)
    assert editor_selection(app, editor) == (0, 0)
    assert n.window_text(query) == "HANDLE"
    n.key_message(editor, win32con.VK_F3)
    n.wait(lambda: editor_selection(app, editor)[1] > editor_selection(app, editor)[0], "F3 finds next")
    app.command(HISTORY_BACK)
    n.wait(lambda: "CreateFileW" in app.content(doc), "history restores source", 45)
    assert editor_selection(app, editor) == saved

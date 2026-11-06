"""Document find uses native selections without adding navigation visits."""
import win32con
import win32gui
import uiautomation as auto
from support import native as n
from test_navigation import editor_selection
from test_history import enabled, HISTORY_BACK


def test_document_find_controls_remain_visible_and_disjoint_when_narrow(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row(app.named_rows("CreateFileW")[0])
    n.wait(lambda: "CreateFileW" in app.content(doc), "declaration loaded", 45)
    app.command(1060)
    query = n.get_item(doc, 1220)
    n.wait(lambda: n.is_visible(query), "find opened")
    auto.ControlFromHandle(query).GetValuePattern().SetValue('HANDLE')
    n.wait(lambda: n.window_text(n.get_item(doc, 1223)).startswith("1/"), "first match")
    with n.window_dpi_context(doc):
        dpi = (n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc)
               if hasattr(n.user, "GetDpiForWindow") else 96)
        scale = lambda value: round(value * dpi / 96)
        win32gui.SendMessage(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE, 0)
        for width in (900, 320, 220, 900):
            left, top, _, _ = n.rectangle(doc)
            x, y = win32gui.ScreenToClient(app.mdi, (left, top))
            assert auto.ControlFromHandle(doc).MoveWindow(x, y, scale(width), scale(720))
            controls = [n.get_item(doc, control) for control in
                        (1224, 1220, 1062, 1061, 1063, 1221, 1222, 1223, 1203)]
            assert all(control and n.is_visible(control) for control in controls)
            rectangles = [n.rectangle(control) for control in controls]
            _, _, client_width, client_height = n.rectangle(doc, client=True)
            left, top = win32gui.ClientToScreen(doc, (0, 0))
            for index, rect in enumerate(rectangles):
                assert left <= rect[0] < rect[2] <= left + client_width
                assert top <= rect[1] < rect[3] <= top + client_height
                for other in rectangles[index + 1:]:
                    assert (rect[2] <= other[0] or other[2] <= rect[0]
                            or rect[3] <= other[1] or other[3] <= rect[1])
            assert n.window_text(query) == "HANDLE"


def test_document_find_and_preview_reuse(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.preview_row(app.named_rows("CreateFileW")[0])
    n.wait(lambda: "CreateFileW" in app.content(doc), "declaration loaded", 45)
    editor = n.get_item(doc, 1203)
    query = n.get_item(doc, 1220)
    assert not enabled(app, HISTORY_BACK)
    assert not n.is_visible(query)
    collapsed = n.rectangle(editor)
    app.command(1060)
    query = n.wait(lambda: n.get_item(doc, 1220), "find query exposed")
    count = n.wait(lambda: n.get_item(doc, 1223), "find count exposed")
    n.wait(lambda: n.is_visible(query) and app.focused() == query, "find opened")
    expanded = n.rectangle(editor)
    assert expanded[1] > collapsed[1]
    auto.ControlFromHandle(query).GetValuePattern().SetValue('HANDLE')
    n.wait(lambda: n.window_text(count).startswith("1/"), "first match")
    first = editor_selection(app, editor)
    assert first[1] - first[0] == 6
    total = int(n.window_text(count).split("/")[1].split()[0])
    assert total > 1
    win32gui.PostMessage(query, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(query, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    n.wait(lambda: editor_selection(app, editor)[0] > first[0], "Enter finds next")
    app.command(1062)
    assert editor_selection(app, editor) == first
    app.command(1062)
    assert n.window_text(count).startswith(f"{total}/{total}")
    assert editor_selection(app, editor)[0] > first[0]
    auto.ControlFromHandle(query).GetValuePattern().SetValue('not_present_anywhere_12345')
    n.wait(lambda: n.window_text(count).startswith("0/0"), "no matches")
    assert n.rectangle(editor) == expanded
    win32gui.PostMessage(query, win32con.WM_KEYDOWN, win32con.VK_ESCAPE, 1)
    win32gui.PostMessage(query, win32con.WM_KEYUP, win32con.VK_ESCAPE, -1073741823)
    n.wait(lambda: not n.is_visible(query) and app.focused() == editor
           and n.rectangle(editor) == collapsed, "find dismissed and body expanded")
    assert not enabled(app, HISTORY_BACK)
    app.command(1060)
    assert n.window_text(query) == "HANDLE"
    auto.ControlFromHandle(query).GetValuePattern().SetValue('HANDLE')
    saved = editor_selection(app, editor)
    app.query("CloseHandle")
    reused = app.preview_row(app.named_rows("CloseHandle")[0])
    assert reused == doc
    n.wait(lambda: "CloseHandle" in app.content(doc), "preview replaced", 45)
    assert editor_selection(app, editor) == (0, 0)
    assert n.window_text(query) == "HANDLE"
    win32gui.PostMessage(editor, win32con.WM_KEYDOWN, win32con.VK_F3, 1)
    win32gui.PostMessage(editor, win32con.WM_KEYUP, win32con.VK_F3, -1073741823)
    n.wait(lambda: editor_selection(app, editor)[1] > editor_selection(app, editor)[0], "F3 finds next")
    app.command(HISTORY_BACK)
    n.wait(lambda: "CreateFileW" in app.content(doc), "history restores source", 45)
    assert editor_selection(app, editor) == saved

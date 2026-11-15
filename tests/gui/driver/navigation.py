"""Follow declaration references and observe exact destination/reading state."""
import win32con
import win32gui
from support import native as n
from support import deadline
from support.native import select_text
from driver.menus import popup, wait_closed


def activate(app, doc):
    app.focus(doc)
    n.wait(lambda: app.active_document() == doc, "document activated")


def reference(app, doc, spelling):
    n.wait(lambda: spelling in app.content(doc), "reference text", 45)
    text = app.content(doc).replace("\r\n", "\r")
    return len(text[:text.index(spelling)].encode("utf-16-le")) // 2


def destination(app, source, name, architecture):
    doc = n.wait(lambda: (lambda doc: doc if doc in app.documents()
                         and n.window_text(doc).split(" (", 1)[0] == name + " - " + architecture else None)(
                             app.active_document()), "navigation destination")
    assert app.choice(doc) == architecture
    n.wait(lambda: app.focused() == n.get_item(doc, 1203), "destination editor focused")
    n.wait(lambda: name in app.content(doc), "destination declaration", 45)
    return doc


def pin_active_tab(app):
    with deadline.operation('measure active tab for pin gesture via UIA'):
        page = next(page for page in app._tabs() if page.GetSelectionItemPattern().IsSelected)
        rect = page.BoundingRectangle
        tabs = n.get_item(app.frame, 1104)
        left, top, _, _ = n.rectangle(tabs)
        point = ((rect.left - left + min(20, (rect.right - rect.left) // 2)) & 0xffff) | (
            (((rect.top + rect.bottom) // 2 - top) & 0xffff) << 16)
    n.send(tabs, win32con.WM_LBUTTONDBLCLK, win32con.MK_LBUTTON, point)
    n.send(tabs, win32con.WM_LBUTTONUP, 0, point)
    n.wait(lambda: not any(text.startswith(("Preview · ", "预览 · "))
                          for text in app.tab_descriptions()), "tab kept open")


def editor_selection(app, editor, value=None):
    if value is not None:
        select_text(editor, *value)
    with deadline.operation('read editor selection via TOM'):
        selected = n.rich_edit_document(editor).Selection
        return selected.Start, selected.End


def choose_reference(editor):
    for key in (win32con.VK_DOWN, win32con.VK_RETURN):
        win32gui.PostMessage(editor, win32con.WM_KEYDOWN, key, 1)
        win32gui.PostMessage(editor, win32con.WM_KEYUP, key, -1073741823)
    wait_closed(editor)


def follow_caret(app, doc, index):
    activate(app, doc)
    editor = n.get_item(doc, 1203)
    app.focus(editor)
    editor_selection(app, editor, (index, index))
    popup(app, editor, keyboard=True)
    choose_reference(editor)

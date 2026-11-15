"""User edits that emit the native change notification of an editing gesture."""
import win32con
from support import native as n


def open_declaration(app, name):
    """Search by API name and wait for the chosen declaration, not just a tab."""
    app.query(name)
    document = app.open_row(app.first_named_row(name.rsplit('.', 1)[-1]))
    spelling = name.rsplit('.', 1)[-1]
    n.wait(lambda: spelling in app.content(document), 'read declaration ' + spelling, 45)
    return document


def select_read_only_text(app, document, first, last):
    """Selection remains available even though typed text cannot edit a declaration."""
    editor = n.get_item(document, 1203)
    before = app.content(document)
    app.focus(editor)
    n.select_text(editor, first, last)
    n.send(editor, win32con.WM_CHAR, ord('X'))
    assert app.content(document) == before, 'reading a selection must not edit the declaration'
    return editor


def replace_input(hwnd, value):
    # WM_SETTEXT/ValuePattern does not emit EN_CHANGE for a multiline EDIT.
    n.send(hwnd, win32con.EM_SETSEL, 0, -1)
    buffer = n.c.create_unicode_buffer(value)
    n.send(hwnd, win32con.EM_REPLACESEL, 1, n.c.addressof(buffer))

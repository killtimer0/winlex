"""Find behavior uses literal ASCII word expectations, not RichEdit's matcher."""
import re

from driver import until
import win32con
import win32gui


def ranges(text, query, case=False, word=False):
    text = text.replace('\r\n', '\r')
    pattern = (r'\b' if word else '') + re.escape(query) + (r'\b' if word else '')
    return [(len(text[:m.start()].encode('utf-16-le')) // 2,
             len(text[:m.end()].encode('utf-16-le')) // 2)
            for m in re.finditer(pattern, text, 0 if case else re.IGNORECASE)]


def option(app, identifier, enabled):
    control = app.child(identifier, app.document())
    if bool(win32gui.SendMessage(control, win32con.BM_GETCHECK, 0, 0)) != enabled:
        win32gui.SendMessage(control, win32con.BM_CLICK, 0, 0)


def test_find_cycles_and_options_apply_without_changing_declaration(app):
    document = app.open_symbol('IStream')
    text = document.text_block()
    query = 'Read'
    expected = ranges(text, query)
    assert len(expected) > 1
    app.find(query)
    first = document.selection_indices()
    assert first in expected
    index = expected.index(first)
    for offset in range(1, len(expected) + 1):
        win32gui.SendMessage(app.frame, win32con.WM_COMMAND, 1061, 0)
        assert document.selection_indices() == expected[(index + offset) % len(expected)]
    win32gui.SendMessage(app.frame, win32con.WM_COMMAND, 1062, 0)
    assert document.selection_indices() == expected[(index - 1) % len(expected)]

    option(app, 1222, True)  # whole word excludes pcbRead
    words = ranges(text, query, word=True)
    assert words and len(words) < len(expected)
    assert document.selection_indices() in words
    option(app, 1221, True)
    before = document.selection_indices()
    app.find_query('read')  # no lowercase whole word in the SDK interface
    assert not ranges(text, 'read', case=True, word=True)
    win32gui.SendMessage(app.frame, win32con.WM_COMMAND, 1061, 0)
    assert document.selection_indices() == before
    app.find_query('Read')
    until('case-sensitive whole-word match', lambda: document.selection_indices() in words)
    assert document.text_block() == text


def test_find_conditions_are_per_document_and_empty_search_is_safe(app):
    first = app.open_symbol('CreateFileW')
    app.find('lpFileName')
    second = app.open_symbol('CloseHandle')
    app.find('hObject')
    first.activate()
    assert app.text(app.child(1220, first.window())) == 'lpFileName'
    first_text = first.text_block()
    app.find_query('no_such_winlex_token')
    assert not ranges(first_text, 'no_such_winlex_token')
    app.find_query('')
    assert first.text_block() == first_text
    second.activate()
    assert app.text(app.child(1220, second.window())) == 'hObject'
    assert second.selection_indices() in ranges(second.text_block(), 'hObject')

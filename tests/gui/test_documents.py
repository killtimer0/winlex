"""Document identity and lifetime contracts from the public GUI guide."""

import pytest
import win32gui

from driver import auto, until


def test_preview_reuses_document_until_pinned(app):
    preview = app.preview_symbol('CreateFileW')
    preview_window = preview.window()
    replacement = app.preview_symbol('CloseHandle')
    assert replacement.window() == preview_window
    assert 'CloseHandle' in preview.text_block() and 'CreateFileW' not in preview.text_block()

    pinned = app.open_symbol('CloseHandle')
    assert pinned.window() == preview_window
    pinned_text = pinned.text_block()
    another = app.preview_symbol('CreateFileW')
    assert another.window() != pinned.window()
    assert pinned.text_block() == pinned_text
    next_preview = app.preview_symbol('GetLastError')
    assert next_preview.window() == another.window()
    assert pinned.text_block() == pinned_text

    reopened = app.open_symbol('CloseHandle')
    assert reopened.window() == pinned.window()
    assert reopened.text_block() == pinned_text


def test_open_existing_document_and_create_independent_copy(app):
    original = app.open_symbol('CreateFileW')
    original_text = original.text_block()
    app.find('CreateFileW')
    original_selection = original.selection_indices()
    app.open_symbol('CloseHandle')
    assert app.open_symbol('CreateFileW').window() == original.window()

    duplicate = app.open_symbol('CreateFileW', new=True)
    assert duplicate.window() != original.window()
    assert duplicate.text_block() == original_text
    app.find('lpFileName')
    assert original.selection_indices() == original_selection
    original.activate()
    assert app.editor().text_block() == original_text
    duplicate.activate()
    assert app.editor().window() == duplicate.window()


def test_close_active_document_preserves_survivor_and_allows_reopen(app):
    survivor = app.open_symbol('CreateFileW')
    survivor_text = survivor.text_block()
    survivor_selection = app.find('lpFileName')
    closing = app.open_symbol('CloseHandle')
    closing_text = closing.text_block()
    closing.close()

    def survivor_active():
        assert app.editor().window() == survivor.window()
        assert survivor.text_block() == survivor_text
        assert survivor.selection_indices() == survivor_selection
        return True

    until('remaining document retains its declaration and selection', survivor_active)
    # The remaining document must still accept reading actions, not just exist.
    next_selection = app.find('dwDesiredAccess')
    assert next_selection != survivor_selection

    reopened = app.open_symbol('CloseHandle')
    assert reopened.text_block() == closing_text
    survivor.activate()
    assert survivor.text_block() == survivor_text
    assert survivor.selection_indices() == next_selection


@pytest.mark.real_input
def test_history_restores_reading_position_and_reopens_closed_declaration(app):
    assert auto.AU3_WinActivateByHandle(app.frame)
    until('history application foreground', lambda: win32gui.GetForegroundWindow() == app.frame)
    survivor = app.open_symbol('CloseHandle')
    survivor.select_architecture('x86')
    until('x86 architecture selected', lambda: survivor.architecture() == 'x86')
    document = app.open_symbol('IStream')
    until('IStream x86 declaration', lambda: document.architecture() == 'x86'
          and 'Clone' in document.text_block())
    declaration = document.text_block()
    selection = app.find('Clone')
    # IStream has many multiline COM method signatures. Scroll the real viewport
    # independently of the selected method, so a restored selection alone fails.
    document.scroll_lines(-100000)
    document.scroll_lines(10)
    line = until('IStream viewport below the top', document.first_visible_line)
    assert line > 0

    def restored():
        active = app.editor()
        assert active.text_block() == declaration
        assert active.architecture() == 'x86'
        assert active.selection_indices() == selection
        assert active.first_visible_line() == line
        return active

    # Store a different reading state in the same live document. Merely
    # reactivating an untouched tab would not prove history restores positions.
    document.select_architecture('x64')
    until('IStream x64 rendering', lambda: document.architecture() == 'x64'
          and 'Clone' in document.text_block() and document.text_block() != declaration)
    changed_selection = app.find('IStream')
    document.scroll_lines(-100000)
    assert changed_selection != selection
    assert document.first_visible_line() == 0
    app.command(1055)  # Navigation > Back
    document = until('history restores IStream reading position', restored)
    document.close()
    until('surviving declaration active',
          lambda: app.editor().window() == survivor.window())
    # The source remains loaded: Forward must reopen the closed declaration,
    # including the reading position, rather than merely showing its text.
    app.command(1056)  # Navigation > Forward
    until('history reopens IStream at its reading position', restored)

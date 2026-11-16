"""Public behavior smoke coverage; no transient loading-window assertions."""
import shutil
from pathlib import Path

import pytest
from driver import until


def test_search_read_find_and_exit(app):
    editor = app.open_symbol('CreateFileW')
    text = editor.text_block()
    assert 'CreateFileW' in text and 'HANDLE' in text
    start, end = app.find('CreateFileW')
    # Native selection offsets are UTF-16 units, not Python code point offsets.
    selected = text.replace('\r\n', '\r').encode('utf-16-le')[start * 2:end * 2].decode('utf-16-le')
    assert selected == 'CreateFileW'
    app.close()


def test_history_returns_to_previous_declaration(app):
    app.open_symbol('CreateFileW')
    app.open_symbol('CloseHandle')
    app.command(1055)
    until('history back', lambda: 'CreateFileW' in app.editor().text_block())
    app.command(1056)
    until('history forward', lambda: 'CloseHandle' in app.editor().text_block())


def test_file_dialog_cancel_preserves_search(app):
    app.cancel_file_dialog()
    assert 'CloseHandle' in app.open_symbol('CloseHandle').text_block()


def test_file_dialog_opens_metadata(app, metadata):
    # A different source path must actually be committed; an ignored dialog
    # cannot pass by reusing the catalog loaded through the command line.
    app.command(1021)
    before = app.source_count()
    added = app.output / 'additional 输入.winmd'
    shutil.copyfile(metadata, added)
    app.open_file(added)
    until('additional source committed', lambda: app.source_count() == before + 1, 30)
    assert 'CreateFileW' in app.open_symbol('CreateFileW').text_block()


def test_source_reload_preserves_open_document_snapshot(app):
    fixtures = Path(__file__).with_name('fixtures')
    source = app.output / 'reload 输入.winmd'
    shutil.copyfile(fixtures / 'before.winmd', source)
    app.open_file(source)
    original = app.open_symbol('ReloadSnapshot')
    original_text = original.text_block()
    assert 'OriginalField' in original_text and 'UpdatedField' not in original_text

    shutil.copyfile(fixtures / 'after.winmd', source)
    app.reload_source(source)
    # Only the replacement defines this type: no status text or stale result
    # can serve as evidence that the new catalog has been committed.
    app.search('ReloadVersionTwo')
    app.open_symbol('ReloadSnapshot')
    until('updated declaration', lambda: 'UpdatedField' in app.editor().text_block())
    updated = app.editor()
    updated_text = updated.text_block()
    assert 'OriginalField' not in updated_text

    original.activate()
    assert app.editor().text_block() == original_text
    updated.activate()
    assert app.editor().text_block() == updated_text


@pytest.mark.real_input
def test_keyboard_search_and_open(app):
    app.keyboard_search('CreateFileW')

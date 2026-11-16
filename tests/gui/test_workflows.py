"""Public behavior smoke coverage; no transient loading-window assertions."""
import shutil
from pathlib import Path

import pytest
from driver import until


def test_search_read_find_and_exit(app):
    editor = app.open_symbol('CreateFileW')
    text = editor.text_block()
    assert 'CreateFileW' in text and 'HANDLE' in text
    app.find('CreateFileW')
    assert editor.selected_text() == 'CreateFileW'
    app.close()


def test_history_returns_to_previous_declaration(app):
    app.open_symbol('CreateFileW')
    app.open_symbol('CloseHandle')
    app.command(1055)
    until('history back', lambda: 'CreateFileW' in app.editor().text_block())
    app.command(1056)
    until('history forward', lambda: 'CloseHandle' in app.editor().text_block())


def test_file_dialog_cancel_preserves_search(app, ui_text):
    app.search('CloseHandle')
    before = app.completed_result_names(ui_text(2025))
    app.cancel_file_dialog()
    assert app.text(app.child(1102)) == 'CloseHandle'
    assert app.completed_result_names(ui_text(2025)) == before


def test_file_dialog_opens_metadata(app):
    added = app.output / 'additional 输入.winmd'
    shutil.copyfile(Path(__file__).with_name('fixtures') / 'before.winmd', added)
    app.open_file(added)
    assert 'OriginalField' in app.open_symbol('ReloadSnapshot').text_block()


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


def test_failed_source_reload_retains_data_and_recovers(app, ui_text):
    fixtures = Path(__file__).with_name('fixtures')
    source = app.output / 'recover 输入.winmd'
    shutil.copyfile(fixtures / 'before.winmd', source)
    app.open_file(source)
    original = app.open_symbol('ReloadSnapshot')
    original_text = original.text_block()
    assert 'OriginalField' in original_text and 'UpdatedField' not in original_text

    source.write_bytes(b'not a metadata file')
    app.reload_source(source)
    # A retained document alone cannot prove that a reload was attempted.
    # Read the diagnostic resource rather than duplicate localized wording.
    failed = ui_text(2102)  # IDS_RELOAD_FAILED
    assert failed
    until('source reload failure reported', lambda: failed in app.source_text(source))
    original.activate()
    assert app.editor().text_block() == original_text
    # Navigate away before searching again, so an untouched active editor
    # cannot stand in for a working search over the retained catalog.
    app.open_symbol('CloseHandle')
    assert app.open_symbol('ReloadSnapshot').text_block() == original_text

    shutil.copyfile(fixtures / 'after.winmd', source)
    app.reload_source(source)
    app.search('ReloadVersionTwo')
    app.open_symbol('ReloadSnapshot')
    until('recovered declaration', lambda: 'UpdatedField' in app.editor().text_block())
    assert 'OriginalField' not in app.editor().text_block()
    assert failed not in app.source_text(source)
    original.activate()
    assert app.editor().text_block() == original_text


@pytest.mark.real_input
def test_keyboard_search_and_open(app):
    app.keyboard_search('CreateFileW')

"""Document identity and lifetime contracts from the public GUI guide."""


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

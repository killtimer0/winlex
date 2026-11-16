"""Closing a source removes search candidates, not open document snapshots."""
from pathlib import Path

import pytest
import win32con
import win32gui

from driver import until


@pytest.mark.parametrize('command', [1050, 1049])  # Open / open in new document
def test_source_panel_open_does_not_use_hidden_search_selection(app, command):
    original = app.open_symbol('CloseHandle')
    document = app.document()
    declaration = original.text_block()
    app.select_result('CreateFileW')
    win32gui.SendMessage(app.frame, win32con.WM_COMMAND, 1021, 0)  # Sources
    win32gui.SendMessage(app.frame, win32con.WM_COMMAND, command, 0)
    assert app.document() == document
    assert original.text_block() == declaration


def test_closed_source_keeps_document_and_architecture_switching(app, metadata, ui_text):
    app.open_file(Path(__file__).with_name('fixtures') / 'before.winmd')
    app.search('ReloadSnapshot')
    original = app.open_symbol('SECURITY_ATTRIBUTES')
    original.select_architecture('x86')
    until('x86 SDK structure', lambda: 'size: 12 bytes, alignment: 4 bytes' in original.text_block())
    x86_text = original.text_block()
    original.select_architecture('x64')
    until('x64 SDK structure', lambda: 'size: 24 bytes, alignment: 8 bytes' in original.text_block())
    x64_text = original.text_block()
    original.select_architecture('x86')
    until('original architecture restored', lambda: original.text_block() == x86_text)

    query = 'name = "SECURITY_ATTRIBUTES" OR name = "ReloadSnapshot"'
    app.search_mode(ui_text(2097))  # IDS_MODE_CONDITION
    app.search(query, expected='ReloadSnapshot')
    completed = ui_text(2025)  # IDS_SEARCH_STATUS
    assert set(app.completed_result_names(completed)) == {
        'Windows.Win32.Security.SECURITY_ATTRIBUTES', 'GuiFixture.ReloadSnapshot'}
    before = app.source_count()
    app.close_source(metadata)
    until('source removed', lambda: app.source_count() == before - 1)

    original.activate()
    assert original.text_block() == x86_text
    original.select_architecture('x64')
    until('closed-source x64 snapshot', lambda: original.text_block() == x64_text)
    original.select_architecture('x86')
    until('closed-source x86 snapshot', lambda: original.text_block() == x86_text)

    # Keep a positive result from another source, and wait for completion before
    # concluding that the removed source's type is absent.
    app.search(query, expected='ReloadSnapshot')
    assert app.completed_result_names(completed) == ['GuiFixture.ReloadSnapshot']

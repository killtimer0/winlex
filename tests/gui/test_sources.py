"""Closing a source removes search candidates, not open document snapshots."""
from pathlib import Path
import shutil

import pytest
import win32con
import win32gui

from driver import until


def test_long_source_list_starts_at_first_row_and_preserves_view_on_refresh(app, ui_text):
    fixture = Path(__file__).with_name('fixtures') / 'search_left.winmd'
    inputs = app.output / 'sources'
    inputs.mkdir()
    paths = [inputs / ('source-{:03d}.winmd'.format(index)) for index in range(100)]
    for path in paths:
        shutil.copyfile(fixture, path)
    app.restart(paths=paths)
    app.search('GuiSearchLeftReady')
    app.completed_result_names(ui_text(2025))
    app.command(1021)  # Sources: first layout follows hidden data initialization.
    listing = app.child(1112)
    until('sources visible', lambda: win32gui.IsWindowVisible(listing))
    assert app.list_item_rect(listing, 0)[1] == 0, 'First display must not skip the first source'

    row = app.list_item_rect(listing, 0)
    win32gui.SendMessage(listing, 0x1014, 0, 30 * (row[3] - row[1]))  # LVM_SCROLL
    top = win32gui.SendMessage(listing, 0x1027, 0, 0)  # LVM_GETTOPINDEX
    assert top > 0, 'Exercise an actually scrolled list'
    anchor = app.list_command(listing, 'GetText', str(top)).split('\r\n', 1)[0]
    offset = app.list_item_rect(listing, top)[1]

    extra = inputs / 'additional.winmd'
    shutil.copyfile(fixture, extra)
    app.open_file(extra)
    until('additional source loaded', lambda: app.source_count() == len(paths) + 1
          and ui_text(2050) in app.source_text(extra))  # IDS_LOADED
    index = app.source_index(Path(anchor))
    assert app.list_item_rect(listing, index)[1] == offset, 'Refresh must preserve the reading anchor'
    app.search('GuiSearchLeftReady')
    app.completed_result_names(ui_text(2025))
    app.command(1021)
    until('sources visible again', lambda: win32gui.IsWindowVisible(listing))
    index = app.source_index(Path(anchor))
    assert app.list_item_rect(listing, index)[1] == offset, 'Refresh and panel switching must preserve the reading anchor'


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

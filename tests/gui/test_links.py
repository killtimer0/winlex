"""Public context-menu navigation retains the originating catalog and architecture."""
from pathlib import Path
import shutil

import pytest
from driver import until


@pytest.mark.real_input
def test_type_jump_and_back_restore_origin(app):
    origin = app.open_symbol('CreateFileW')
    origin.select_architecture('x86')
    until('x86 function rendered', lambda: origin.architecture() == 'x86'
          and 'SECURITY_ATTRIBUTES' in origin.text_block())
    position = app.find('SECURITY_ATTRIBUTES')
    line = origin.first_visible_line()
    origin.context_action(2158)  # Go to declaration
    until('target structure rendered', lambda: 'struct SECURITY_ATTRIBUTES' in app.editor().text_block())
    target = app.editor()
    assert target.architecture() == 'x86'
    assert 'struct SECURITY_ATTRIBUTES' in target.text_block()
    target_window = target.window()
    app.command(1055)
    until('origin selection restored', lambda: app.editor().window() == origin.window()
          and origin.selection_indices() == position and origin.first_visible_line() == line)
    origin.context_action(2158)
    until('existing target activated', lambda: app.editor().window() == target_window)


@pytest.mark.real_input
def test_type_jump_uses_old_snapshot_after_reload_and_close(app):
    fixtures = Path(__file__).with_name('fixtures')
    source = app.output / 'linked 快照.winmd'
    shutil.copyfile(fixtures / 'before.winmd', source)
    app.open_file(source)
    origin = app.open_symbol('ReloadHolder')
    origin.select_architecture('x86')
    until('holder rendered', lambda: 'ReloadSnapshot' in origin.text_block())
    shutil.copyfile(fixtures / 'after.winmd', source)
    app.reload_source(source)
    app.search('ReloadVersionTwo')
    origin.activate()
    app.find('ReloadSnapshot')
    origin.context_action(2158)
    until('original target rendered', lambda: 'OriginalField' in app.editor().text_block())
    target = app.editor()
    assert 'OriginalField' in target.text_block() and 'UpdatedField' not in target.text_block()
    assert target.architecture() == 'x86'
    before = app.source_count()
    app.close_source(source)
    until('linked source removed', lambda: app.source_count() == before - 1)
    origin.activate()
    app.find('ReloadSnapshot')
    origin.context_action(2158)
    until('closed-source target activated', lambda: app.editor().window() == target.window())
    assert 'OriginalField' in app.editor().text_block()

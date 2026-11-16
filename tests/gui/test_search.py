"""Search recovery and source identity through public edits and result rows."""
from pathlib import Path
import re

import win32con
import win32gui

from driver import until


FIXTURES = Path(__file__).with_name('fixtures')


def open_fixture(app, name):
    path = (FIXTURES / ('search_' + name + '.winmd')).resolve()
    before = app.source_count()
    app.open_file(path)
    until('search fixture committed: ' + name, lambda: app.source_count() == before + 1)
    # The source panel can include a still-pending input. All callers are in
    # quick mode here; a definition unique to this file proves catalog commit.
    app.search('GuiSearch' + name.title() + 'Ready')
    return path


def change_query(app, query):
    # Unlike app.search(), this deliberately does not wait for a matching row.
    app.record('change query ' + query)
    app.set_text(app.child(1102), query)


def open_source_result(app, full_name, source, field):
    """Select by the full name and source path exposed by the ListView row."""
    results = app.child(1103)
    matches = []
    for index in range(int(app.list_command(results, 'GetItemCount'))):
        lines = app.list_command(results, 'GetText', str(index)).split('\r\n')
        if lines[0] == full_name and any(Path(line) == source for line in lines[1:]):
            matches.append(index)
    assert len(matches) == 1, (full_name, source, matches)
    app.record('open result {} from {}'.format(full_name, source))
    app.list_command(results, 'SelectClear')
    app.list_command(results, 'Select', str(matches[0]))
    win32gui.SendMessage(app.frame, win32con.WM_COMMAND, 1049, 0)  # Open in new window
    until('source-specific declaration ' + field, lambda: field in app.editor().text_block())
    return app.editor()


def test_invalid_condition_recovers_in_condition_and_quick_modes(app, ui_text):
    open_fixture(app, 'left')
    app.search_mode(ui_text(2097))  # IDS_MODE_CONDITION
    app.search('name = "GuiSearchFinalAmber"', expected='GuiSearchFinalAmber')
    assert app.completed_result_names(ui_text(2025)) == ['GuiSearch.GuiSearchFinalAmber']

    change_query(app, 'not_a_search_field = "GuiSearchFinalAmber"')
    diagnostic = ui_text(2122)  # IDS_QUERY_FIELD
    assert diagnostic
    until('invalid condition diagnosed', lambda: diagnostic in app.text(app.child(1105)))
    assert int(app.list_command(app.child(1103), 'GetItemCount')) == 0

    app.search('name = "GuiSearchFinalCobalt"', expected='GuiSearchFinalCobalt')
    assert app.completed_result_names(ui_text(2025)) == ['GuiSearch.GuiSearchFinalCobalt']
    app.search_mode(ui_text(2090))  # IDS_MODE_COMBINED (quick search)
    app.search('GuiSearchFinalQuartz')
    assert app.completed_result_names(ui_text(2025)) == ['GuiSearch.GuiSearchFinalQuartz']
    assert 'QuartzValue' in app.open_symbol('GuiSearchFinalQuartz').text_block()


def test_rapid_query_changes_leave_only_the_final_results(app, ui_text):
    open_fixture(app, 'left')
    app.search_mode(ui_text(2090))
    # Exercise broad SDK searches as well as edits coalesced by the debounce.
    # Each round has a new final name: a previous completed result cannot satisfy
    # the positive barrier. No timing requirement assumes a worker was cancelled.
    for suffix, field in [('Amber', 'AmberValue'), ('Cobalt', 'CobaltValue'),
                          ('Quartz', 'QuartzValue')]:
        for query in ['Windows', 'Create', '', 'Close', 'GuiSearchDiscard']:
            change_query(app, query)
            edit = app.child(1102)
            win32gui.PostMessage(edit, win32con.WM_KEYDOWN, win32con.VK_RETURN, 0)
            win32gui.PostMessage(edit, win32con.WM_KEYUP, win32con.VK_RETURN, 0)
        name = 'GuiSearchFinal' + suffix
        app.search(name)
        assert app.completed_result_names(ui_text(2025)) == ['GuiSearch.' + name]
        assert field in app.open_symbol(name).text_block()


def test_same_named_results_open_the_selected_source(app, ui_text):
    left = open_fixture(app, 'left')
    right = open_fixture(app, 'right')
    app.search_mode(ui_text(2097))
    full_name = 'GuiSearch.GuiSearchSharedEntry'
    app.search('fullname = "' + full_name + '"', expected='GuiSearchSharedEntry')
    assert app.completed_result_names(ui_text(2025)) == [full_name, full_name]

    for source, present, absent in [(left, 'LeftOnlyValue', 'RightOnlyValue'),
                                    (right, 'RightOnlyValue', 'LeftOnlyValue'),
                                    (left, 'LeftOnlyValue', 'RightOnlyValue')]:
        text = open_source_result(app, full_name, source, present).text_block()
        assert present in text and absent not in text


def test_adding_missing_dependency_resolves_field_layout(app, ui_text):
    open_fixture(app, 'consumer')
    app.search_mode(ui_text(2090))
    original = app.open_symbol('GuiSearchHolder')
    missing = original.text_block()
    # The fixture embeds a two-int32 value type between uint32 fields, not a
    # pointer. Without its definition the declaration survives but layout cannot.
    assert 'GuiSearchPair Payload;' in missing
    assert 'Layout unavailable:' in missing
    assert 'size: 16 bytes, alignment: 4 bytes' not in missing

    dependency = open_fixture(app, 'dependency')
    app.search('GuiSearchPair')
    app.completed_result_names(ui_text(2025))
    # The consumer also contains a TypeRef with this name. Open the definition
    # from the dependency rather than relying on the first result's kind/order.
    pair = open_source_result(app, 'GuiSearchDependency.GuiSearchPair', dependency,
                              'FirstValue').text_block()
    assert 'FirstValue' in pair and 'SecondValue' in pair
    assert 'size: 8 bytes, alignment: 4 bytes' in pair

    resolved = app.open_symbol('GuiSearchHolder', new=True).text_block()
    assert 'Layout unavailable:' not in resolved
    assert 'size: 16 bytes, alignment: 4 bytes' in resolved
    assert 'GuiSearchPair Payload;' in resolved
    assert re.search(r'offset: 4 bytes, size: 8 bytes\s+[^\r\n]*\bPayload;', resolved)
    assert re.search(r'offset: 12 bytes, size: 4 bytes\s+[^\r\n]*\bTailValue;', resolved)
    original.activate()
    assert original.text_block() == missing

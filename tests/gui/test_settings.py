"""Settings commit/cancel and search filtering contracts from docs/gui.md."""
from driver import until

SETTINGS_SEARCH_ENGINE = 1302
FILTER_FUNCTION = 1317
IDS_SEARCH_STATUS = 2025
IDS_SEARCH_GOOGLE = 2390
IDS_SEARCH_BING = 2391
IDS_SEARCH_BAIDU = 2392


def test_applied_engine_survives_cancel_and_ok_persists(app, ui_text):
    google, bing, baidu = (ui_text(identifier) for identifier in
                           (IDS_SEARCH_GOOGLE, IDS_SEARCH_BING, IDS_SEARCH_BAIDU))
    sheet = app.settings()
    assert sheet.value(SETTINGS_SEARCH_ENGINE) == google
    sheet.select(SETTINGS_SEARCH_ENGINE, bing)
    sheet.apply()
    sheet.select(SETTINGS_SEARCH_ENGINE, baidu)
    sheet.cancel()

    sheet = app.settings()
    assert sheet.value(SETTINGS_SEARCH_ENGINE) == bing
    sheet.cancel()
    app.restart()
    sheet = app.settings()
    assert sheet.value(SETTINGS_SEARCH_ENGINE) == bing
    sheet.select(SETTINGS_SEARCH_ENGINE, baidu)
    sheet.ok()

    sheet = app.settings()
    assert sheet.value(SETTINGS_SEARCH_ENGINE) == baidu
    sheet.cancel()
    app.restart()
    sheet = app.settings()
    assert sheet.value(SETTINGS_SEARCH_ENGINE) == baidu
    sheet.cancel()


def test_function_filter_cancel_apply_and_persistence(app, ui_text):
    status = ui_text(IDS_SEARCH_STATUS)
    function = 'Windows.Win32.Storage.FileSystem.Apis.CreateFileW'
    structure = 'Windows.Win32.Storage.FileSystem.CREATEFILE2_EXTENDED_PARAMETERS'
    declaration = app.open_symbol('CreateFileW')
    original_text = declaration.text_block()

    # Quick search is case-insensitive and matches both public Win32 symbols.
    app.search('CreateFile', expected='CREATEFILE2_EXTENDED_PARAMETERS')
    baseline = set(app.completed_result_names(status))
    assert {function, structure} <= baseline

    sheet = app.settings()
    assert sheet.checked(FILTER_FUNCTION)
    sheet.check(FILTER_FUNCTION, False)
    sheet.cancel()
    assert set(app.completed_result_names(status)) == baseline

    sheet = app.settings()
    assert sheet.checked(FILTER_FUNCTION)
    sheet.check(FILTER_FUNCTION, False)
    sheet.apply()
    sheet.cancel()
    declaration.activate()
    assert declaration.text_block() == original_text

    # Apply must refresh the current query, not merely affect the next search.
    def filtered_current_query():
        names = set(app.completed_result_names(status))
        assert structure in names and function not in names
        return True
    until('current search reflects applied categories', filtered_current_query)
    sheet = app.settings()
    assert not sheet.checked(FILTER_FUNCTION)
    sheet.cancel()

    app.restart()
    sheet = app.settings()
    assert not sheet.checked(FILTER_FUNCTION)
    sheet.cancel()
    app.search('CreateFile', expected='CREATEFILE2_EXTENDED_PARAMETERS')
    restored = set(app.completed_result_names(status))
    assert structure in restored
    assert function not in restored

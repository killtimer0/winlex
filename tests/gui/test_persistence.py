"""Normal source restoration is distinct from command-line temporary sessions."""
from pathlib import Path

import pytest
import win32api
import win32con
import win32gui
from driver import until


def test_window_placement_survives_restart(app):
    win32gui.ShowWindow(app.frame, win32con.SW_RESTORE)
    original = win32gui.GetWindowRect(app.frame)
    left, top, right, bottom = win32api.GetMonitorInfo(
        win32api.MonitorFromWindow(app.frame))['Work']
    width, height = min(900, right - left - 80), min(600, bottom - top - 80)
    win32gui.SetWindowPos(app.frame, 0, right - width - 40, bottom - height - 40,
                          width, height, win32con.SWP_NOZORDER)
    saved = win32gui.GetWindowRect(app.frame)
    assert saved != original
    app.open_symbol('CreateFileW')
    app.restart()
    restored = win32gui.GetWindowRect(app.frame)
    assert all(abs(a - b) <= 2 for a, b in zip(saved, restored)), (saved, restored)
    app.open_symbol('CreateFileW')
    app.find('lpFileName')


@pytest.mark.timeout(120)
def test_normal_sources_restore_and_temporary_sessions_do_not_replace_them(app, metadata, ui_text):
    other = Path(__file__).with_name('fixtures') / 'before.winmd'
    app.restart(paths=[], empty=True)
    app.cancel_startup_prompt()
    app.open_file(metadata)
    app.open_symbol('CreateFileW')
    app.restart()
    assert app.source_count() == 1
    assert 'CreateFileW' in app.open_symbol('CreateFileW').text_block()

    app.open_file(other)
    app.search('ReloadSnapshot')
    app.close_source(metadata)
    until('normal source replaced', lambda: app.source_count() == 1)
    app.restart()
    assert 'OriginalField' in app.open_symbol('ReloadSnapshot').text_block()
    assert app.source_count() == 1

    app.restart(paths=[metadata])
    assert 'CreateFileW' in app.open_symbol('CreateFileW').text_block()
    app.open_file(other)
    app.search('ReloadSnapshot')
    app.close_source(other)
    until('temporary source replaced', lambda: app.source_count() == 1)
    sheet = app.settings()
    sheet.select(1302, ui_text(2391))  # Preferences still save in temporary sessions.
    sheet.ok()
    app.restart(paths=[])
    assert app.source_count() == 1
    assert 'OriginalField' in app.open_symbol('ReloadSnapshot').text_block()
    sheet = app.settings()
    assert sheet.value(1302) == ui_text(2391)
    sheet.cancel()

    app.close_source(other)
    until('normal collection emptied', lambda: app.source_count() == 0)
    app.restart(empty=True)
    app.cancel_startup_prompt()

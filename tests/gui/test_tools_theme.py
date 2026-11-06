import win32con
import winreg
import pytest
import uiautomation as auto

from support import native as n
from support.diagnostics import screenshot
from test_theme import THEME_SUPPORTED, dark_background, modern_theme, select_theme, selected_index
from test_tools import body, query, tool


@modern_theme
@pytest.mark.interactive
def test_tool_empty_and_populated_result_follow_theme(launch, profile, settings):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'dark')
    app = launch()
    doc = tool(app)
    result = n.get_item(doc, 1816)
    assert body(doc) == ''
    assert dark_background(result)
    screenshot(app.frame, settings[2] / 'tool-empty-dark.png')
    for populated in (False, True):
        if populated:
            query(app, doc, '5')
        original = body(doc)
        for mode in (1, 2):
            sheet = app.settings()
            page = app.settings_page(sheet, 0)
            select_theme(page, mode)
            app.close_settings(sheet)
            assert dark_background(result) == (mode == 2)
            assert body(doc) == original


@pytest.mark.skipif(THEME_SUPPORTED, reason='requires an OS without modern appearance support')
@pytest.mark.parametrize('preference', ['light', 'dark'])
def test_unsupported_appearance_retains_preference_and_native_tool_behavior(launch, profile, preference):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, preference)
    app = launch()
    results = app.results
    doc = tool(app)
    original = query(app, doc, '5')
    assert '0x00000005' in original
    controls = [n.get_item(doc, control) for control in (1810, 1815, 1816)]
    layout = [n.rectangle(control) for control in controls]
    enabled = n.api(n.user, 'IsWindowEnabled', n.w.BOOL, n.w.HWND)
    system_color = n.api(n.user, 'GetSysColor', n.w.DWORD, n.c.c_int)
    assert n.send(results, 0x1000) == system_color(win32con.COLOR_WINDOW)  # LVM_GETBKCOLOR
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    appearance = n.get_item(page, 1301)
    assert len(app.choices(appearance)) == 1
    assert selected_index(app, appearance) == 0
    assert not enabled(appearance)
    search_page = app.settings_page(sheet, 1)
    checkbox = auto.ControlFromHandle(n.get_item(search_page, 1313))
    if not checkbox.GetTogglePattern().ToggleState:
        checkbox.GetTogglePattern().Toggle()
    app.close_settings(sheet)
    assert profile.preferences() == ('en-US', 15, preference)
    for notification in (win32con.WM_SETTINGCHANGE, win32con.WM_SYSCOLORCHANGE, 0x031a):
        n.send(app.frame, notification)
    assert body(doc) == original and app.active_document() == doc
    assert [n.rectangle(control) for control in controls] == layout
    assert all(n.is_visible(control) and enabled(control) for control in controls)
    assert n.send(results, 0x1000) == system_color(win32con.COLOR_WINDOW)
    assert '0x00000057' in query(app, doc, '87')
    assert profile.preferences() == ('en-US', 15, preference)
    restarted = launch()
    sheet = restarted.settings()
    appearance = n.get_item(restarted.settings_page(sheet, 0), 1301)
    assert len(restarted.choices(appearance)) == 1 and not enabled(appearance)
    restarted.close_settings(sheet)
    assert profile.preferences() == ('en-US', 15, preference)
    assert '0x00000005' in query(restarted, tool(restarted), '5')

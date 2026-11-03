import win32con
import winreg

from support import native as n
from support.diagnostics import screenshot
from test_theme import dark_background
from test_tools import body, query, tool


def test_tool_empty_and_populated_result_follow_theme(launch, profile, settings):
    profile.set('Preferences', winreg.REG_SZ, 'en-US:7:dark')
    app = launch()
    doc = tool(app)
    result = n.get_item(doc, 1816)
    assert body(doc) == ''
    assert dark_background(result)
    screenshot(app.frame, settings[2] / 'tool-empty-dark.bmp')
    for populated in (False, True):
        if populated:
            query(app, doc, '5')
        original = body(doc)
        for mode in (1, 2):
            sheet = app.settings()
            page = app.settings_page(sheet, 0)
            n.send(n.get_item(page, 1301), win32con.CB_SETCURSEL, mode)
            app.close_settings(sheet)
            assert dark_background(result) == (mode == 2)
            assert body(doc) == original

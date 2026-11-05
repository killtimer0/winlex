import struct
import ctypes as c
import sys
import pytest
import win32con
import win32gui
import winreg
from support import native as n
from support.diagnostics import capture, screenshot
from test_about import open_about
from test_search_help import open_help


THEME_SUPPORTED = tuple(sys.getwindowsversion()[:3]) >= (10, 0, 19045)
modern_theme = pytest.mark.skipif(
    not THEME_SUPPORTED, reason='requires Windows 10 build 19045 or later')


def dark_background(hwnd):
    root = n.api(n.user, 'GetAncestor', n.w.HWND, n.w.HWND, n.w.UINT)(hwnd, win32con.GA_ROOT)
    point = win32gui.ClientToScreen(hwnd, (3, 3))
    x, y = win32gui.ScreenToClient(root, point)
    with capture(root, client=True) as (_, header, pixels):
        width, height = struct.unpack_from('<ii', header, 4)
        # Sample an inset blank corner rather than glyphs or the native border.
        offset = ((height - y - 1) * width + x) * 4
        return max(pixels[offset:offset + 3]) < 128


@pytest.mark.parametrize('language,dpi', [
    ('en-US', 'pmv2'), ('en-US', 'pmv1'),
    ('zh-CN', 'system'), ('zh-CN', 'unaware'),
])
@modern_theme
def test_theme_switch_persistence_and_dialogs(launch, profile, settings, language, dpi):
    profile.set('Language', winreg.REG_SZ, language)
    profile.set('Theme', winreg.REG_SZ, 'dark')
    app = launch(dpi=dpi)
    assert dark_background(app.search)
    for opener in (open_about, open_help):
        dialog = opener(app)
        assert dark_background(dialog)
        screenshot(dialog, settings[2] / ('dark-%s-%s-%s.bmp' % (language, dpi, opener.__name__)))
        n.post(n.get_item(dialog, 1), win32con.BM_CLICK, 0, 0)
        n.wait(lambda: not n.is_window(dialog), 'close themed dialog')
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    combo = n.get_item(page, 1301)
    assert n.send(combo, win32con.CB_GETCURSEL) == 2
    assert dark_background(page)
    tab = next(h for h in n.windows(sheet) if n.class_name(h) == 'SysTabControl32')
    assert dark_background(tab)
    screenshot(sheet, settings[2] / ('dark-%s-%s-settings.bmp' % (language, dpi)))
    n.send(combo, win32con.CB_SETCURSEL, 1)
    app.close_settings(sheet, accept=False)
    assert profile.preferences() == (language, 7, 'dark')
    assert dark_background(app.search)
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    n.send(n.get_item(page, 1301), win32con.CB_SETCURSEL, 1)
    app.close_settings(sheet)
    assert profile.preferences() == (language, 7, 'light')
    assert not dark_background(app.search)
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    assert n.send(n.get_item(page, 1301), win32con.CB_GETCURSEL) == 1
    n.send(n.get_item(page, 1301), win32con.CB_SETCURSEL, 2)
    app.close_settings(sheet)
    assert dark_background(app.search)
    assert profile.preferences() == (language, 7, 'dark')
    n.send(app.frame, win32con.WM_SETTINGCHANGE, 0, 0)
    assert dark_background(app.search)
    assert profile.preferences() == (language, 7, 'dark')
    restarted = launch(dpi=dpi)
    assert dark_background(restarted.search)


@modern_theme
def test_theme_change_preserves_document(launch, profile, settings):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'light')
    app = launch([settings[1]])
    app.query('CreateFileW')
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), 'loaded declaration', 45)
    original = app.content(doc)
    for mode in (2, 1, 2):
        sheet = app.settings()
        page = app.settings_page(sheet, 0)
        n.send(n.get_item(page, 1301), win32con.CB_SETCURSEL, mode)
        app.close_settings(sheet)
        assert app.content(doc) == original
        assert app.active_document() == doc
        assert dark_background(app.search) == (mode == 2)
    screenshot(app.frame, settings[2] / 'dark-document.bmp')


@modern_theme
def test_system_theme_uses_windows_preference(launch, profile):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'system')
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                            r'Software\Microsoft\Windows\CurrentVersion\Themes\Personalize') as key:
            expected = winreg.QueryValueEx(key, 'AppsUseLightTheme')[0] == 0
    except FileNotFoundError:
        expected = False
    app = launch()
    assert dark_background(app.search) == expected
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    assert n.send(n.get_item(page, 1301), win32con.CB_GETCURSEL) == 0
    app.close_settings(sheet)
    assert profile.preferences() == ('en-US', 7, 'system')


@modern_theme
def test_modern_titlebar_tracks_explicit_theme(launch, profile):
    try:
        dwm = c.WinDLL('dwmapi')
        get_attribute = n.api(dwm, 'DwmGetWindowAttribute', c.c_long,
                              n.w.HWND, n.w.DWORD, c.c_void_p, n.w.DWORD)
    except (OSError, AttributeError):
        pytest.skip('DWM attributes unavailable')
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'dark')
    app = launch()
    value = c.c_int()
    attribute = next((attribute for attribute in (20, 19)
                      if get_attribute(app.frame, attribute, c.byref(value), 4) == 0), None)
    if attribute is None:
        pytest.skip('dark titlebar attribute unavailable')
    assert value.value == 1
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    n.send(n.get_item(page, 1301), win32con.CB_SETCURSEL, 1)
    app.close_settings(sheet)
    assert get_attribute(app.frame, attribute, c.byref(value), 4) == 0
    assert value.value == 0

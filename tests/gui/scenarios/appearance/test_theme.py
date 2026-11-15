"""Apply and persist appearance without closing dialogs or losing documents."""
import ctypes as c
import sys
import pytest
import win32con
import winreg
import uiautomation as auto
from support import messages, native as n
from support.session import invoke
from support.diagnostics import screenshot
from driver.dialogs import open_about, open_help
from driver.appearance import background_pixels, modern_theme, select_theme, selected_index


@modern_theme
def test_settings_apply_theme_without_closing(launch, profile):
    profile.set('Theme', winreg.REG_SZ, 'light')
    app = launch()
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    apply = auto.ControlFromHandle(n.get_item(sheet, 0x3021))
    light = n.send(app.results, 0x1000)  # LVM_GETBKCOLOR
    for index, theme in ((2, 'dark'), (1, 'light'), (2, 'dark')):
        select_theme(page, index)
        n.wait(lambda: apply.IsEnabled, 'Apply enabled after theme edit')
        invoke(apply)
        n.wait(lambda: profile.get('Theme') == theme and not apply.IsEnabled,
               'theme applied without closing')
        assert n.is_window(sheet)
        background = n.send(app.results, 0x1000)
        assert (background == light) == (theme == 'light')
    select_theme(page, 1)
    app.close_settings(sheet, accept=False)
    assert profile.get('Theme') == 'dark'
    assert n.send(app.results, 0x1000) != light


@pytest.mark.parametrize('language,dpi', [
    ('en-US', 'pmv2'), ('en-US', 'pmv1'),
    ('zh-CN', 'system'), ('zh-CN', 'unaware'),
])
@modern_theme
@pytest.mark.interactive
def test_theme_switch_persistence_and_dialogs(launch, profile, settings, language, dpi):
    profile.set('Language', winreg.REG_SZ, language)
    profile.set('Theme', winreg.REG_SZ, 'dark')
    original = profile.preferences()
    app = launch(dpi=dpi)
    dark = background_pixels(app.search)
    dialogs = {}
    for opener in (open_about, open_help):
        dialog = opener(app)
        dialogs[opener] = background_pixels(dialog)
        screenshot(dialog, settings[2] / ('dark-%s-%s-%s.png' % (language, dpi, opener.__name__)))
        invoke(n.get_item(dialog, 1))
        n.wait(lambda: not n.is_window(dialog), 'close themed dialog')
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    combo = n.get_item(page, 1301)
    assert selected_index(app, combo) == 2
    dark_page = background_pixels(page)
    tab = n.send(sheet, messages.PSM_GETTABCONTROL)
    assert tab, 'settings tab control'
    dark_tab = background_pixels(tab)
    screenshot(sheet, settings[2] / ('dark-%s-%s-settings.png' % (language, dpi)))
    select_theme(page, 1)
    app.close_settings(sheet, accept=False)
    assert profile.preferences() == original
    assert background_pixels(app.search) == dark
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    select_theme(page, 1)
    app.close_settings(sheet)
    assert profile.preferences() == (*original[:2], 'light')
    n.wait(lambda: background_pixels(app.search) != dark, 'owner repaints after theme changes')
    for opener, previous in dialogs.items():
        dialog = opener(app)
        assert background_pixels(dialog) != previous, 'new dialogs follow the changed theme'
        invoke(n.get_item(dialog, 1))
        n.wait(lambda: not n.is_window(dialog), 'close themed dialog')
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    assert selected_index(app, n.get_item(page, 1301)) == 1
    assert background_pixels(page) != dark_page, 'settings inherits the changed theme'
    tab = n.send(sheet, messages.PSM_GETTABCONTROL)
    assert tab, 'settings tab control'
    assert background_pixels(tab) != dark_tab, 'tab surface inherits the changed theme'
    select_theme(page, 2)
    app.close_settings(sheet)
    assert profile.preferences() == original
    n.wait(lambda: background_pixels(app.search) == dark, 'owner restores the original theme')
    n.send(app.frame, win32con.WM_SETTINGCHANGE, 0, 0)
    assert background_pixels(app.search) == dark
    assert profile.preferences() == original
    app.close()
    restarted = launch(dpi=dpi)
    n.wait(lambda: background_pixels(restarted.search) == dark, 'restarted owner paints persisted theme')


@modern_theme
@pytest.mark.interactive
def test_theme_change_preserves_document(launch, profile, settings):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'light')
    app = launch([settings[1]])
    app.query('CreateFileW')
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), 'loaded declaration', 45)
    original = app.content(doc)
    light = background_pixels(app.search)
    dark = None
    for mode in (2, 1, 2):
        sheet = app.settings()
        page = app.settings_page(sheet, 0)
        select_theme(page, mode)
        app.close_settings(sheet)
        assert app.content(doc) == original
        assert app.active_document() == doc
        if mode == 1:
            n.wait(lambda: background_pixels(app.search) == light, 'light appearance returns')
        else:
            n.wait(lambda: background_pixels(app.search) != light, 'theme change is painted')
            if dark is None:
                dark = background_pixels(app.search)
            else:
                assert background_pixels(app.search) == dark
    screenshot(app.frame, settings[2] / 'dark-document.png')


@modern_theme
@pytest.mark.interactive
def test_system_theme_uses_windows_preference(launch, profile):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'system')
    original = profile.preferences()
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                            r'Software\Microsoft\Windows\CurrentVersion\Themes\Personalize') as key:
            expected = winreg.QueryValueEx(key, 'AppsUseLightTheme')[0] == 0
    except FileNotFoundError:
        expected = False
    app = launch()
    system = background_pixels(app.search)
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    assert selected_index(app, n.get_item(page, 1301)) == 0
    app.close_settings(sheet)
    assert profile.preferences() == original
    profile.set('Theme', winreg.REG_SZ, 'dark' if expected else 'light')
    explicit = launch()
    assert background_pixels(explicit.search) == system, 'system preference matches explicit appearance'


@modern_theme
def test_modern_titlebar_tracks_explicit_theme(launch, profile):
    if tuple(sys.getwindowsversion()[:3]) < (10, 0, 18362):
        pytest.skip('Windows 10 1809 uses the legacy titlebar composition API')
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
    select_theme(page, 1)
    app.close_settings(sheet)
    assert get_attribute(app.frame, attribute, c.byref(value), 4) == 0
    assert value.value == 0

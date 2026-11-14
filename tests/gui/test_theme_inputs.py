"""Visible input-control states and real keyboard/mouse behavior."""
import sys
import winreg
import pytest
import win32con
import win32gui
import uiautomation as auto

from support import deadline, messages, native as n
from support.diagnostics import capture, screenshot
from support.input_state import InputState
from test_theme import background_pixels, select_theme
from test_theme_surfaces import background, center, colors, dark_app, foreground, inset, move

pytestmark = pytest.mark.interactive


class ComboInfo(n.c.Structure):
    _fields_ = [('size', n.w.DWORD), ('item', n.w.RECT), ('button', n.w.RECT),
                ('state', n.w.DWORD), ('combo', n.w.HWND), ('edit', n.w.HWND),
                ('list', n.w.HWND)]


def combo_info(hwnd):
    info = ComboInfo()
    info.size = n.c.sizeof(info)
    assert n.api(n.user, 'GetComboBoxInfo', n.w.BOOL, n.w.HWND,
                 n.c.POINTER(ComboInfo))(hwnd, n.c.byref(info))
    return info


def click_control(inputs, hwnd):
    point = win32gui.ClientToScreen(hwnd, center(n.rectangle(hwnd, client=True)))
    # WindowFromPoint deliberately ignores disabled controls.
    assert (not win32gui.IsWindowEnabled(hwnd) or
            win32gui.WindowFromPoint(point) == hwnd), 'input target is obscured'
    inputs.click(*point)


def change_theme(app, mode):
    sheet = app.settings()
    select_theme(app.settings_page(sheet, 0), mode)
    app.close_settings(sheet)


def test_theme_combo_dropdown_pixels_and_keyboard_commit(launch, profile, settings):
    app = dark_app(launch, profile)
    dropdowns = {}
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        # Each new sheet must inherit the current theme, including its popup list.
        for dark, target in ((True, 1), (False, 2), (True, 1)):
            previous = background_pixels(app.search)
            sheet = app.settings()
            combo = n.get_item(app.settings_page(sheet, 0), 1301)
            info = combo_info(combo)
            arrow = (info.button.left, info.button.top, info.button.right, info.button.bottom)
            inputs.click(*win32gui.ClientToScreen(combo, center(arrow)), wait_time=0)
            n.wait(lambda: n.send(combo, win32con.CB_GETDROPPEDSTATE), 'mouse opens theme dropdown')
            popup = n.wait(lambda: info.list if n.is_visible(info.list) else None, 'combo list visible')
            _, _, width, height = n.rectangle(popup, client=True)
            sample = (3, 3, width - 3, height - 3)
            palette = colors(popup, sample)
            fill = palette.most_common(1)[0][0]
            if dark in dropdowns:
                assert fill == dropdowns[dark], 'dropdown appearance returns with the theme'
            if dark not in dropdowns and dropdowns:
                assert fill != next(iter(dropdowns.values())), 'dropdown follows the theme change'
            dropdowns[dark] = fill
            assert any(color != fill for color in palette), 'dropdown option text must be painted'
            screenshot(sheet, settings[2] / ('theme-combo-%s.png' % dark))
            # A native dropdown retains the navigated row on Escape; cancellation
            # must not commit preferences or close the surrounding property sheet.
            original_preferences = profile.preferences()
            inputs.key(win32con.VK_HOME)
            inputs.key(win32con.VK_ESCAPE)
            n.wait(lambda: not n.send(combo, win32con.CB_GETDROPPEDSTATE), 'Escape closes dropdown')
            assert n.send(combo, win32con.CB_GETCURSEL) == 0
            assert profile.preferences() == original_preferences
            assert n.is_window(sheet), 'Escape from dropdown must not close settings'
            inputs.key(win32con.VK_F4)
            n.wait(lambda: n.send(combo, win32con.CB_GETDROPPEDSTATE), 'F4 reopens dropdown')
            inputs.key(win32con.VK_HOME)
            for _ in range(target):
                inputs.key(win32con.VK_DOWN)
            inputs.key(win32con.VK_RETURN)
            n.wait(lambda: not n.send(combo, win32con.CB_GETDROPPEDSTATE), 'Enter commits dropdown')
            assert n.send(combo, win32con.CB_GETCURSEL) == target
            assert n.is_window(sheet), 'Enter from dropdown must not accept settings'
            app.close_settings(sheet)
            assert profile.preferences()[2] == ('dark' if target == 2 else 'light')
            n.wait(lambda: background_pixels(app.search) != previous,
                   'owner repaints the committed theme after settings closes')


def test_edit_focus_disabled_and_theme_roundtrip(launch, profile, settings):
    app = dark_app(launch, profile)
    enable = n.api(n.user, 'EnableWindow', n.w.BOOL, n.w.HWND, n.w.BOOL)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        edit = app.search
        inputs.disable_ime(edit)
        appearances = {}
        for position, mode in enumerate((2, 1, 2)):
            previous = background_pixels(edit)
            if position:
                change_theme(app, mode)
                n.wait(lambda: background_pixels(edit) != previous, 'search edit repaints after theme changes')
            if mode in appearances:
                assert background_pixels(edit) == appearances[mode], 'search edit restores its appearance'
            appearances[mode] = background_pixels(edit)
            app.focus(app.results)
            move(app.status, (8, 3))
            click_control(inputs, edit)
            n.wait(lambda: app.focused() == edit, 'mouse focuses search edit')
            move(app.status, (8, 3))
            app.focus(app.results)
            n.wait(lambda: app.focused() == app.results, 'search edit releases focus')
            before = n.window_text(edit)
            try:
                enable(edit, False)
                assert not win32gui.IsWindowEnabled(edit)
                click_control(inputs, edit)
                assert app.focused() != edit, 'disabled edit must reject mouse focus'
                assert n.window_text(edit) == before
            finally:
                enable(edit, True)
            move(app.status, (8, 3))
            assert win32gui.IsWindowEnabled(edit)
            click_control(inputs, edit)
            n.send(edit, win32con.EM_SETSEL, -1, -1)
            inputs.key(ord('A'))
            n.wait(lambda: n.window_text(edit) == before + 'a', 'reenabled edit accepts real keyboard input')
        assert n.window_text(edit) == 'aaa', 'theme switches must preserve edit contents'


def monitor_with_dpi(expected):
    """Find a real monitor, rather than simulating a WM_DPICHANGED message."""
    shcore = n.c.WinDLL('shcore')
    get_dpi = n.api(shcore, 'GetDpiForMonitor', n.c.c_long,
                    n.w.HANDLE, n.c.c_int, n.c.POINTER(n.w.UINT), n.c.POINTER(n.w.UINT))
    candidates = []
    errors = []
    callback_type = n.c.WINFUNCTYPE(n.w.BOOL, n.w.HANDLE, n.w.HDC,
                                   n.c.POINTER(n.w.RECT), n.w.LPARAM)

    def visit(monitor, _dc, rectangle, _data):
        x, y = n.w.UINT(), n.w.UINT()
        result = get_dpi(monitor, 0, n.c.byref(x), n.c.byref(y))
        if result != 0:
            errors.append(result)
            return False
        area = rectangle.contents
        candidates.append((x.value, y.value, (area.left, area.top, area.right, area.bottom)))
        return True

    callback = callback_type(visit)
    assert n.api(n.user, 'EnumDisplayMonitors', n.w.BOOL,
                 n.w.HDC, n.c.c_void_p, callback_type, n.w.LPARAM)(None, None, callback, 0)
    assert not errors, ('GetDpiForMonitor failed', errors)
    matches = [area for x, y, area in candidates if x == y == expected]
    if not matches:
        pytest.skip('no physical monitor configured for %s DPI: %r' % (expected, candidates))
    return matches[0]


def edit_pixels(hwnd):
    win32gui.RedrawWindow(hwnd, None, None,
                         win32con.RDW_INVALIDATE | win32con.RDW_UPDATENOW | win32con.RDW_FRAME)
    with capture(hwnd) as (bitmap, _header, pixels):
        width, height = bitmap.Width, bitmap.Height
        return [[tuple(pixels[((height - y - 1) * width + x) * 4:
                              ((height - y - 1) * width + x) * 4 + 3])
                 for x in range(width)] for y in range(height)]


@pytest.mark.skipif(sys.getwindowsversion().build < 22000, reason='requires Windows 11')
@pytest.mark.parametrize('dpi', [96, 192])
def test_win11_edit_real_dpi_corners_and_theme_restore(launch, profile, settings, dpi):
    profile.set('Theme', winreg.REG_SZ, 'dark')
    app = launch(dpi='pmv2')
    get_dpi = n.api(n.user, 'GetDpiForWindow', n.w.UINT, n.w.HWND)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        left, top, right, bottom = monitor_with_dpi(dpi)
        width, height = 1000 * dpi // 96, 650 * dpi // 96
        assert width + 128 <= right - left and height + 128 <= bottom - top
        win32gui.SetWindowPos(app.frame, 0, left + 64, top + 64, width, height,
                              win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
        edit = app.search
        n.wait(lambda: get_dpi(app.frame) == dpi and get_dpi(edit) == dpi,
               'frame and Edit adopt actual monitor DPI')
        inputs.disable_ime(edit)
        appearances = {}
        for index, mode in enumerate((2, 1, 2)):
            previous = background_pixels(edit)
            if index:
                change_theme(app, mode)
                n.wait(lambda: background_pixels(edit) != previous, 'Edit repaints after theme changes')
            assert get_dpi(edit) == dpi
            assert win32gui.GetWindowLong(edit, win32con.GWL_EXSTYLE) & win32con.WS_EX_CLIENTEDGE
            if mode in appearances:
                assert background_pixels(edit) == appearances[mode]
            appearances[mode] = background_pixels(edit)
            app.focus(app.results)
            move(app.status, (8, 3))
            idle = edit_pixels(edit)
            before = n.window_text(edit)
            click_control(inputs, edit)
            n.wait(lambda: app.focused() == edit, 'native mouse focus')
            move(app.status, (8, 3))
            n.wait(lambda: edit_pixels(edit) != idle, 'native Edit focus repaint is visible')
            screenshot(edit, settings[2] / ('win11-edit-%s-%s-focused.png' % (dpi, index)))
            n.send(edit, win32con.EM_SETSEL, -1, -1)
            inputs.key(ord('A'))
            n.wait(lambda: n.window_text(edit) == before + 'a', 'native Edit keyboard input')
            app.focus(app.results)
            move(app.status, (8, 3))
            n.wait(lambda: app.focused() == app.results, 'native Edit releases focus')
        assert n.window_text(edit) == 'aaa', 'theme roundtrip must preserve native Edit text'


@deadline.operation('measure settings tab item via UIA', 5)
def tab_rect(tab, item):
    rect = item.BoundingRectangle
    x, y = win32gui.ClientToScreen(tab, (0, 0))
    return rect.left - x, rect.top - y, rect.right - x, rect.bottom - y


@deadline.operation('read settings tab selection via UIA', 5)
def tab_selected(item):
    return item.GetSelectionItemPattern().IsSelected


def test_settings_tabs_hover_keyboard_focus_and_navigation(launch, profile, settings):
    app = dark_app(launch, profile)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        sheet = app.settings()
        tab = n.send(sheet, messages.PSM_GETTABCONTROL)
        items = app.settings_tabs(sheet)
        selected, other = items[0], items[1]

        def strip_background():
            rectangles = [tab_rect(tab, item) for item in items]
            for rect in rectangles:
                palette = colors(tab, inset(rect))
                fill = palette.most_common(1)[0][0]
                assert any(color != fill for color in palette), 'tab label must be painted'
            # Include unused header space, not just the labels.
            _, _, width, _ = n.rectangle(tab, client=True)
            left = max(rect[2] for rect in rectangles) + 4
            top = max(rect[1] for rect in rectangles) + 4
            bottom = min(rect[3] for rect in rectangles) - 4
            return background(tab, (left, top, width - 4, bottom))

        page = app.settings_page(sheet, 0)
        app.focus(n.get_item(page, 1301))
        move(sheet, (8, 8))
        screenshot(sheet, settings[2] / 'dark-settings-tabs-idle.png')
        original_strip = strip_background()
        other_rect = tab_rect(tab, other)
        move(tab, center(other_rect))
        assert tab_selected(selected), 'hover must not switch page'
        assert strip_background() == original_strip
        move(sheet, (8, 8))
        # Tab through the dialog so Windows reveals keyboard focus cues naturally.
        for _ in range(16):
            if app.focused() == tab:
                break
            previous = app.focused()
            inputs.key(win32con.VK_TAB)
            n.wait(lambda: app.focused() != previous, 'settings Tab advances focus')
        assert app.focused() == tab, 'settings tabs must be keyboard reachable'
        inputs.key(win32con.VK_RIGHT)
        n.wait(lambda: tab_selected(other), 'Right selects next settings page')
        assert n.is_visible(n.get_item(sheet, 1310))
        assert strip_background() == original_strip
        inputs.key(win32con.VK_LEFT)
        n.wait(lambda: tab_selected(selected), 'Left restores settings page')
        assert n.is_visible(n.get_item(sheet, 1301))
        screenshot(sheet, settings[2] / 'dark-settings-tabs-focused.png')
        for mode in (1, 2):
            select_theme(page, mode)
            app.close_settings(sheet)
            sheet = app.settings()
            page = app.settings_page(sheet, 0)
            tab = n.send(sheet, messages.PSM_GETTABCONTROL)
            items = app.settings_tabs(sheet)
            move(sheet, (8, 8))
            assert (strip_background() == original_strip) == (mode == 2), 'tab strip follows theme roundtrip'
            screenshot(sheet, settings[2] / ('settings-tabs-theme-%s.png' % mode))
        app.close_settings(sheet, accept=False)

"""Native prompts and pickers preserve modality, default actions and theme."""
import json
import sys
import time

import pytest
import uiautomation as auto
import win32con
import win32gui
import winreg

from support import deadline, native as n
from support.diagnostics import assert_unobscured, screenshot
from support.input_state import GA_ROOT, InputState, foreground, get_ancestor
from support.session import invoke, message_button
from driver.appearance import (
    background_pixels, dark_background, modern_theme, select_theme, settled_background_pixels,
)
from driver.surfaces import dark_color, observed_colors as colors
from driver.native_dialogs import (
    assert_modal, assert_native_text_contrast, dismiss_no, file_picker_colors,
    filename_field, native_dialog, native_task_dialog_primary_fill, picker_entry,
)


@pytest.mark.parametrize('dpi', ['pmv2', 'pmv1'])
def test_sources_prompt_native_version_buttons_and_default(launch, profile, dpi):
    profile.set('AutoLoad', winreg.REG_MULTI_SZ, [])
    before = profile.preferences()
    app = launch(dpi=dpi, startup='metadata-guide')
    prompt = n.wait(lambda: native_dialog(app), 'empty-startup sources prompt')
    assert_modal(app, prompt)
    yes = n.wait(lambda: message_button(prompt, win32con.IDYES), 'native Yes button').NativeWindowHandle
    no = n.wait(lambda: message_button(prompt, win32con.IDNO), 'native No button').NativeWindowHandle
    assert win32gui.IsWindowEnabled(yes) and win32gui.IsWindowEnabled(no)
    # Query default-button semantics instead of pressing Enter and launching a browser.
    assert n.send(yes, win32con.WM_GETDLGCODE) & 0x10, 'Yes must be the default push button'
    assert not n.send(no, win32con.WM_GETDLGCODE) & 0x10
    classes = []
    def visit(hwnd, _):
        classes.append(n.class_name(hwnd))
        return True

    win32gui.EnumChildWindows(prompt, visit, None)
    if sys.getwindowsversion().major >= 6:
        assert 'DirectUIHWND' in classes, 'Vista+ must use the native TaskDialog, not MessageBox'
    else:
        assert 'DirectUIHWND' not in classes, 'XP must use the native MessageBox'
    dismiss_no(app, prompt)
    assert profile.preferences() == before
    assert profile.get('AutoLoad') == []


def assert_task_dialog_client_theme(appearance, dark):
    for name, dominant in zip(('content', 'commands'), appearance[:2]):
        assert (max(dominant) < 128) == dark, (
            'native TaskDialog pane must follow the selected theme', name, dominant)


@deadline.operation('observe TaskDialog client theme')
def task_dialog_appearance(dialog, native_light=False):
    """Observe both client panes for relative theme comparisons."""
    assert_unobscured(dialog)
    surfaces = []
    # capture_pixels crops in the target's coordinate space, whereas UIA bounds
    # are physical pixels. Measure native geometry in that same target space.
    with n.window_dpi_context(dialog):
        _, _, width, height = n.rectangle(dialog, client=True)
        panes = [(3, top, width - 3, bottom)
                 for top, bottom in ((3, height // 2), (height // 2, height - 3))]
        if native_light:
            expected = native_task_dialog_primary_fill(dialog)

            def primary_painted():
                assert_unobscured(dialog)
                primary = colors(dialog, panes[0]).most_common(1)[0][0]
                assert_unobscured(dialog)
                return primary == expected

            # Screen capture can observe the compositor's opening transition.
            # Wait for a native color role, not an unknown stable sample or delay.
            n.wait(primary_painted, 'native light TaskDialog primary fill is displayed', 15)
        for rect in panes:
            palette = colors(dialog, rect)
            dominant = palette.most_common(1)[0][0]
            surfaces.append(dominant)
        origin = win32gui.ClientToScreen(dialog, (0, 0))
        for identifier in (win32con.IDYES, win32con.IDNO):
            button = message_button(dialog, identifier)
            assert button is not None, ('missing native command', identifier)
            hwnd = button.NativeWindowHandle
            assert hwnd, ('native command has no HWND', identifier)
            left, top, right, bottom = n.rectangle(hwnd)
            rect = (left - origin[0] + 5, top - origin[1] + 5,
                    right - origin[0] - 5, bottom - origin[1] - 5)
            dominant = colors(dialog, rect).most_common(1)[0][0]
            surfaces.append(dominant)
    assert_unobscured(dialog)
    return surfaces


def assert_titlebar_theme(dialog, dark):
    if tuple(sys.getwindowsversion()[:3]) < (10, 0, 18362):
        return
    dwm = n.c.WinDLL('dwmapi')
    get_attribute = n.api(dwm, 'DwmGetWindowAttribute', n.c.c_long,
                          n.w.HWND, n.w.DWORD, n.c.c_void_p, n.w.DWORD)
    value = n.c.c_int()
    attribute = next((attribute for attribute in (20, 19)
                      if get_attribute(dialog, attribute, n.c.byref(value), 4) == 0), None)
    assert attribute is not None, 'native dialog dark titlebar attribute unavailable'
    assert bool(value.value) == dark, 'native dialog titlebar must follow the current owner theme'


@modern_theme
@pytest.mark.interactive
@pytest.mark.parametrize('theme', ['light', 'dark'])
@pytest.mark.parametrize('dpi', ['pmv2', 'pmv1'])
def test_sources_prompt_theme_and_escape_preserve_yes_no(launch, profile, settings, theme, dpi):
    profile.set('AutoLoad', winreg.REG_MULTI_SZ, [])
    profile.set('Theme', winreg.REG_SZ, theme)
    before = profile.preferences()
    with InputState() as inputs:
        app = launch(dpi=dpi, startup='metadata-guide')
        prompt = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
        with n.physical_dpi_context():
            left, top, right, bottom = n.titlebar_rectangle(prompt)
            inputs.click((left + right) // 2, (top + bottom) // 2, wait_time=0)
        n.wait(lambda: win32gui.GetForegroundWindow() == prompt, 'sources prompt foreground')
        assert_modal(app, prompt)
        assert_titlebar_theme(prompt, theme == 'dark')
        assert_native_text_contrast(prompt)
        screenshot(prompt, settings[2] / ('sources-prompt-%s-%s.png' % (theme, dpi)))
        inputs.key(win32con.VK_ESCAPE)
        until = time.monotonic() + .3
        while time.monotonic() < until:
            assert n.is_window(prompt), 'Escape must not dismiss a Yes/No-only dialog'
            assert_modal(app, prompt)
            time.sleep(.025)
        dismiss_no(app, prompt)
        assert profile.preferences() == before
        assert profile.get('AutoLoad') == []


@modern_theme
@pytest.mark.interactive
@pytest.mark.parametrize('dpi', ['pmv2', 'pmv1'])
def test_sources_prompt_client_theme_roundtrip(launch, profile, dpi):
    profile.set('AutoLoad', winreg.REG_MULTI_SZ, [])
    appearances = {}
    with InputState() as inputs:
        for theme in ('light', 'dark', 'light'):
            profile.set('Theme', winreg.REG_SZ, theme)
            app = launch(dpi=dpi, startup='metadata-guide')
            prompt = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
            with deadline.operation('activate sources TaskDialog via UIA'):
                auto.ControlFromHandle(prompt).SetActive(waitTime=0)
            with n.physical_dpi_context():
                left, top, right, bottom = n.titlebar_rectangle(prompt)
                point = ((left + right) // 2, (top + bottom) // 2)
                hit = win32gui.WindowFromPoint(point)
                assert get_ancestor(hit, GA_ROOT) == prompt, 'TaskDialog title bar is obscured'
                if win32gui.GetForegroundWindow() != prompt:
                    inputs.click(*point, wait_time=0)
            n.wait(lambda: win32gui.GetForegroundWindow() == prompt, 'TaskDialog foreground', 15)
            appearance = task_dialog_appearance(prompt, native_light=theme == 'light')
            assert_task_dialog_client_theme(appearance, theme == 'dark')
            if theme in appearances:
                assert appearance == appearances[theme], 'client theme roundtrip restores its appearance'
            elif appearances:
                assert appearance != next(iter(appearances.values())), 'client follows the selected theme'
            appearances[theme] = appearance
            dismiss_no(app, prompt)
            app.close()


@modern_theme
@pytest.mark.interactive
@pytest.mark.parametrize('theme', ['light', 'dark'])
@pytest.mark.parametrize('dpi', ['pmv2', 'pmv1'])
def test_file_picker_current_theme_native_navigation_and_cancel(
        launch, profile, settings, tmp_path, theme, dpi):
    profile.set('Theme', winreg.REG_SZ, theme)
    app = launch(dpi=dpi)
    folder = tmp_path / 'native-picker-navigation'
    folder.mkdir()
    sentinel = folder / 'winlex-dialog-navigation.winmd'
    sentinel.write_bytes(b'')
    with InputState() as inputs, foreground(app, inputs):
        appearances = []
        owner = settled_background_pixels(app.search)
        # Reopen after changing the owner theme, guarding against a stale per-process preference.
        for current in (theme, 'dark' if theme == 'light' else 'light'):
            if current != theme:
                sheet = app.settings()
                select_theme(app.settings_page(sheet, 0), 2 if current == 'dark' else 1)
                app.close_settings(sheet)
                n.wait(lambda: background_pixels(app.search) != owner, 'owner paints changed theme')
            before = profile.preferences()
            assert before[2] == current
            owner = settled_background_pixels(app.search)
            status = app.status_text()
            app.command(1020)
            dialog = n.wait(lambda: native_dialog(app), 'native file picker', 30)
            assert_modal(app, dialog)
            n.wait(lambda: win32gui.GetForegroundWindow() == dialog, 'file picker foreground', 15)
            field = n.wait(lambda: filename_field(dialog), 'native filename field', 15)
            value = n.c.create_unicode_buffer(str(folder) + '\\')
            assert n.send(field, win32con.WM_SETTEXT, 0, n.c.addressof(value))
            button = n.wait(lambda: n.get_item(dialog, win32con.IDOK), 'native Open button', 15)
            win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDOK, button)
            entry = n.wait(lambda: picker_entry(dialog, sentinel), 'native folder navigation contents', 30)
            assert_modal(app, dialog)
            assert_titlebar_theme(dialog, current == 'dark')

            def row_text():
                found = assert_native_text_contrast(dialog)
                return found if any(kind == 'row' for kind, _name, _rect in found) else None

            surfaces = n.wait(row_text, 'native picker list-row text bounds')
            artifact = 'file-picker-%s-%s-from-%s' % (current, dpi, theme)
            screenshot(dialog, settings[2] / (artifact + '.png'))
            evidence = {'theme': current, 'dpi': dpi,
                        'text_surfaces': [{'kind': kind, 'name': name,
                                           'rect': list(rect)}
                                          for kind, name, rect in surfaces],
                        'surfaces': file_picker_colors(dialog, field, entry)}
            (settings[2] / (artifact + '-colors.json')).write_text(
                json.dumps(evidence, indent=2), encoding='utf-8')
            invoke(n.get_item(dialog, win32con.IDCANCEL))
            n.wait(lambda: not n.is_window(dialog) and win32gui.IsWindowEnabled(app.frame),
                   'cancel native file picker', 30)
            assert not native_dialog(app)
            assert app.status_text() == status, 'navigation and Cancel must not load a source'
            assert profile.preferences() == before
            n.wait(lambda: background_pixels(app.search) == owner,
                   'owner repaint preserves theme after picker closes')
            n.wait(lambda: dark_background(app.mdi) == (current == 'dark'),
                   'owner main window restores theme after picker closes')
            assert 'contents' in evidence['surfaces'], 'native folder surface must be measurable'
            for name, measured in evidence['surfaces'].items():
                dominant = tuple(measured['colors_bgr'][0][0])
                assert dark_color(dominant) == (current == 'dark'), (
                    'file picker client surface must follow the selected theme',
                    name, current, dominant, measured['colors_bgr'][:3])
            appearance = {name: value['colors_bgr'][0][0] for name, value in evidence['surfaces'].items()}
            if appearances:
                assert appearance != appearances[-1], 'native picker follows the owner theme change'
            appearances.append(appearance)

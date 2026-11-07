"""Live native dialogs must survive real high-contrast entry and exit."""
import json

import pytest
import uiautomation as auto
import win32con
import win32gui
import winreg
from comtypes import COMError

from support import native as n
from support.diagnostics import screenshot
from support.input_state import InputState, foreground
from support.session import invoke, message_button
from test_system_dialogs import (
    assert_modal, assert_readable_native_client, assert_task_dialog_client_theme,
    dismiss_no, file_picker_colors, filename_field, native_dialog,
)
from test_theme import dark_background, modern_theme
from test_theme_compatibility import (
    HCF_AVAILABLE, HCF_HIGHCONTRASTON, high_contrast, system_color, temporary_high_contrast,
)
from test_theme_surfaces import colors


pytestmark = [modern_theme, pytest.mark.interactive]


def contrast_snapshot():
    snapshot = high_contrast()
    if snapshot[0] & HCF_HIGHCONTRASTON:
        pytest.skip('high contrast already enabled; preserve the existing desktop')
    if not snapshot[0] & HCF_AVAILABLE:
        pytest.skip('system high contrast is unavailable')
    return snapshot


def task_surfaces(dialog):
    _, _, width, height = n.rectangle(dialog, client=True)
    rects = {'content': (3, 3, width - 3, height // 2),
             'commands': (3, height // 2, width - 3, height - 3)}
    origin = win32gui.ClientToScreen(dialog, (0, 0))
    text = auto.ControlFromHandle(dialog).TextControl(AutomationId='ContentText')
    if not text.Exists(0, 0):
        raise LookupError('native TaskDialog content text is not available yet')
    bounds = text.BoundingRectangle
    rects['text'] = (bounds.left - origin[0], bounds.top - origin[1],
                     bounds.right - origin[0], bounds.bottom - origin[1])
    for identifier in (win32con.IDYES, win32con.IDNO):
        button = message_button(dialog, identifier)
        if button is None:
            raise LookupError('native TaskDialog command is not available yet')
        bounds = button.BoundingRectangle
        rects['command_%s' % identifier] = (
            bounds.left - origin[0] + 5, bounds.top - origin[1] + 5,
            bounds.right - origin[0] - 5, bounds.bottom - origin[1] - 5)
    return {name: {'rect': rect, 'colors_bgr': colors(dialog, rect).most_common(8)}
            for name, rect in rects.items()}


def assert_native_contrast(surfaces):
    for name, measured in surfaces.items():
        allowed = native_surface_colors(name)
        dominant = tuple(measured['colors_bgr'][0][0])
        assert dominant in allowed, ('native high-contrast surface must use system colors',
                                     name, dominant, allowed)


def task_text_readable(surfaces):
    palette = surfaces['text']['colors_bgr']
    background = palette[0][0]
    brightness = lambda value: sum(a * b for a, b in zip(value, (.0722, .7152, .2126)))
    return any(abs(brightness(color) - brightness(background)) > 80 and count >= 8
               for color, count in palette)


def native_colors(surfaces):
    return all(tuple(value['colors_bgr'][0][0]) in native_surface_colors(name)
               for name, value in surfaces.items())


def native_surface_colors(name):
    allowed = {system_color(win32con.COLOR_WINDOW), system_color(win32con.COLOR_BTNFACE)}
    if name.startswith('command_') and name != 'command_pane':
        allowed.add(system_color(win32con.COLOR_HIGHLIGHT))
    return allowed


def dark_colors(surfaces):
    return all(max(value['colors_bgr'][0][0]) < 128 for value in surfaces.values())


def settled_surfaces(measure, matches, description, evidence=None):
    last = {}

    def ready():
        nonlocal last
        try:
            last = measure()
        except (COMError, LookupError):
            # Native theme changes rebuild UIA elements while preserving the
            # dialog HWND. Reacquire them; never reuse a pre-transition node.
            return None
        return last if matches(last) else None

    try:
        return n.wait(ready, description, 45)
    except AssertionError as error:
        if evidence is not None:
            record(*evidence, last)
        raise AssertionError((description, 'last measured surfaces', last)) from error


def record(dialog, output, phase, surfaces):
    screenshot(dialog, output / (phase + '.png'))
    screenshot(win32gui.GetDesktopWindow(), output / (phase + '-desktop.png'))
    evidence = {'high_contrast': bool(high_contrast()[0] & HCF_HIGHCONTRASTON),
                'window': system_color(win32con.COLOR_WINDOW),
                'button_face': system_color(win32con.COLOR_BTNFACE), 'surfaces': surfaces}
    (output / (phase + '-colors.json')).write_text(json.dumps(evidence, indent=2), encoding='utf-8')


def assert_prompt_contract(app, dialog):
    assert n.is_window(dialog) and native_dialog(app) == dialog
    assert_modal(app, dialog)
    yes = message_button(dialog, win32con.IDYES)
    no = message_button(dialog, win32con.IDNO)
    assert yes is not None and no is not None
    assert n.send(yes.NativeWindowHandle, win32con.WM_GETDLGCODE) & 0x10
    assert not n.send(no.NativeWindowHandle, win32con.WM_GETDLGCODE) & 0x10


@pytest.mark.parametrize('dpi', ['pmv1', 'pmv2'])
def test_task_dialog_stays_open_across_high_contrast(launch, profile, settings, dpi):
    snapshot = contrast_snapshot()
    profile.set('Theme', winreg.REG_SZ, 'dark')
    profile.set('AutoLoad', winreg.REG_MULTI_SZ, [])
    before = profile.preferences()
    with InputState() as inputs:
        app = launch(dpi=dpi, metadata_prompt=True)
        dialog = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
        auto.ControlFromHandle(dialog).TitleBarControl().Click(simulateMove=False)
        assert_prompt_contract(app, dialog)
        assert_task_dialog_client_theme(dialog, True)
        with temporary_high_contrast(snapshot, settings[2] / ('task-live-%s-restore.json' % dpi)):
            assert_prompt_contract(app, dialog)
            surfaces = settled_surfaces(lambda: task_surfaces(dialog),
                                        lambda value: native_colors(value) and task_text_readable(value),
                                        'open TaskDialog repaints in native high contrast',
                                        (dialog, settings[2], 'task-live-%s-hc-failed' % dpi))
            record(dialog, settings[2], 'task-live-%s-hc' % dpi, surfaces)
            assert_native_contrast(surfaces)
            assert_readable_native_client(dialog)
            inputs.key(win32con.VK_ESCAPE)
            assert_prompt_contract(app, dialog)
        assert high_contrast() == snapshot
        assert_prompt_contract(app, dialog)
        surfaces = settled_surfaces(lambda: task_surfaces(dialog),
                                    lambda value: dark_colors(value) and task_text_readable(value),
                                    'same TaskDialog recovers dark text and panels',
                                    (dialog, settings[2], 'task-live-%s-restored-failed' % dpi))
        record(dialog, settings[2], 'task-live-%s-restored' % dpi, surfaces)
        assert_task_dialog_client_theme(dialog, True)
        assert_readable_native_client(dialog)
        dismiss_no(app, dialog)
        n.wait(lambda: dark_background(app.search), 'dark owner after live TaskDialog closes')
        assert profile.preferences() == before


def navigate(dialog, folder, sentinel):
    field = n.wait(lambda: filename_field(dialog), 'native filename input')
    value = n.c.create_unicode_buffer(str(folder) + '\\')
    assert n.send(field, win32con.WM_SETTEXT, 0, n.c.addressof(value))
    button = n.wait(lambda: n.get_item(dialog, win32con.IDOK), 'native Open button')
    win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDOK, button)
    entry = auto.ControlFromHandle(dialog).Control(
        searchDepth=12, Compare=lambda item, depth: item.Name in (sentinel.name, sentinel.stem))
    assert entry.Exists(10), 'native navigation must reveal the expected file'
    return field, entry


def picker_surfaces(dialog, sentinel):
    field = filename_field(dialog)
    entry = auto.ControlFromHandle(dialog).Control(
        searchDepth=12, Compare=lambda item, depth: item.Name in (sentinel.name, sentinel.stem))
    if not field or not entry.Exists(0, 0):
        raise LookupError('native folder contents are not available yet')
    return file_picker_colors(dialog, field, entry)


@pytest.mark.parametrize('dpi', ['pmv1', 'pmv2'])
def test_file_picker_stays_open_across_high_contrast(launch, profile, settings, tmp_path, dpi):
    snapshot = contrast_snapshot()
    profile.set('Theme', winreg.REG_SZ, 'dark')
    before = profile.preferences()
    app = launch(dpi=dpi)
    folders = []
    for index in range(3):
        folder = tmp_path / ('native-navigation-%s' % index)
        folder.mkdir()
        sentinel = folder / ('contrast-navigation-%s.winmd' % index)
        sentinel.write_bytes(b'')
        folders.append((folder, sentinel))
    with InputState() as inputs, foreground(app, inputs):
        status = app.status_text()
        app.command(1020)
        dialog = n.wait(lambda: native_dialog(app), 'native file picker')
        assert_modal(app, dialog)
        field, entry = navigate(dialog, *folders[0])
        with temporary_high_contrast(snapshot, settings[2] / ('picker-live-%s-restore.json' % dpi)):
            assert n.is_window(dialog) and native_dialog(app) == dialog
            assert_modal(app, dialog)
            surfaces = settled_surfaces(lambda: picker_surfaces(dialog, folders[0][1]), native_colors,
                                        'open picker repaints in native high contrast before navigation')
            record(dialog, settings[2], 'picker-live-%s-hc-before-navigation' % dpi, surfaces)
            assert_native_contrast(surfaces)
            field, entry = navigate(dialog, *folders[1])
            assert_modal(app, dialog)
            surfaces = settled_surfaces(lambda: picker_surfaces(dialog, folders[1][1]), native_colors,
                                        'native navigation retains high contrast')
            assert_native_contrast(surfaces)
        assert high_contrast() == snapshot
        assert n.is_window(dialog) and native_dialog(app) == dialog
        assert_modal(app, dialog)
        surfaces = settled_surfaces(lambda: picker_surfaces(dialog, folders[1][1]), dark_colors,
                                    'same picker recovers dark colors before navigation',
                                    (dialog, settings[2], 'picker-live-%s-restored-failed' % dpi))
        record(dialog, settings[2], 'picker-live-%s-restored-before-navigation' % dpi, surfaces)
        for surface, measured in surfaces.items():
            assert measured['dominant_luminance'] < 128, ('dark picker must recover before navigation', surface, measured)
        field, entry = navigate(dialog, *folders[2])
        surfaces = settled_surfaces(lambda: picker_surfaces(dialog, folders[2][1]), dark_colors,
                                    'native navigation retains recovered dark colors')
        for surface, measured in surfaces.items():
            assert measured['dominant_luminance'] < 128, ('native navigation must retain dark colors', surface, measured)
        invoke(n.get_item(dialog, win32con.IDCANCEL))
        n.wait(lambda: not n.is_window(dialog) and win32gui.IsWindowEnabled(app.frame), 'native Cancel')
        assert not native_dialog(app)
        n.wait(lambda: dark_background(app.search), 'dark owner after live picker closes')
        assert app.status_text() == status
        assert profile.preferences() == before

"""Native message/file dialog contracts, including their public theme boundary."""
import json
import sys
import time

import pytest
import uiautomation as auto
import win32con
import win32gui
import win32process
import winreg

from support import native as n
from support.diagnostics import screenshot
from support.input_state import InputState, foreground
from support.session import invoke, message_button
from test_theme import dark_background, modern_theme, select_theme
from test_theme_surfaces import colors


def native_dialog(app):
    found = []

    def visit(hwnd, _):
        if (win32process.GetWindowThreadProcessId(hwnd)[1] == app.process.pid
                and n.is_visible(hwnd) and n.class_name(hwnd) == '#32770'
                and win32gui.GetWindow(hwnd, win32con.GW_OWNER) == app.frame):
            found.append(hwnd)
        return True

    win32gui.EnumWindows(visit, None)
    assert len(found) <= 1, ('ambiguous owned native dialog', found)
    return found[0] if found else None


def assert_modal(app, dialog):
    assert win32gui.GetWindow(dialog, win32con.GW_OWNER) == app.frame
    assert not win32gui.IsWindowEnabled(app.frame), 'native dialog must disable its owner'


def dismiss_no(app, dialog):
    invoke(n.wait(lambda: message_button(dialog, win32con.IDNO), 'native No button'))
    n.wait(lambda: not n.is_window(dialog) and win32gui.IsWindowEnabled(app.frame),
           'No dismisses prompt and restores owner')
    assert not native_dialog(app), 'No must not open another guide dialog'
    assert app.process.poll() is None


@pytest.mark.parametrize('dpi', ['pmv2', 'pmv1'])
def test_sources_prompt_native_version_buttons_and_default(launch, profile, dpi):
    profile.set('AutoLoad', winreg.REG_MULTI_SZ, [])
    before = profile.preferences()
    app = launch(dpi=dpi, metadata_prompt=True)
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


def assert_readable_native_client(dialog):
    _, _, width, height = n.rectangle(dialog, client=True)
    palette = colors(dialog, (3, 3, width - 3, height - 3))
    background = palette.most_common(1)[0][0]
    luminance = lambda color: sum(a * b for a, b in zip(color, (.0722, .7152, .2126)))
    assert any(abs(luminance(color) - luminance(background)) > 80 and count >= 8
               for color, count in palette.items()), 'native client content must remain visible'


def assert_task_dialog_client_theme(dialog, dark):
    """Check both content and command panes, not just the non-client frame."""
    _, _, width, height = n.rectangle(dialog, client=True)
    for top, bottom in ((3, height // 2), (height // 2, height - 3)):
        palette = colors(dialog, (3, top, width - 3, bottom))
        dominant = palette.most_common(1)[0][0]
        assert (max(dominant) < 128) == dark, (
            'TaskDialog client pane must follow the selected theme',
            (top, bottom), dominant, dark)
    for identifier in (win32con.IDYES, win32con.IDNO):
        button = message_button(dialog, identifier)
        assert button is not None, ('missing native command', identifier)
        bounds = button.BoundingRectangle
        origin = win32gui.ClientToScreen(dialog, (0, 0))
        rect = (bounds.left - origin[0] + 5, bounds.top - origin[1] + 5,
                bounds.right - origin[0] - 5, bounds.bottom - origin[1] - 5)
        dominant = colors(dialog, rect).most_common(1)[0][0]
        assert (max(dominant) < 128) == dark, (
            'TaskDialog command background must follow the selected theme',
            identifier, dominant, dark)


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
        app = launch(dpi=dpi, metadata_prompt=True)
        prompt = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
        auto.ControlFromHandle(prompt).TitleBarControl().Click(simulateMove=False)
        n.wait(lambda: win32gui.GetForegroundWindow() == prompt, 'sources prompt foreground')
        assert_modal(app, prompt)
        assert_titlebar_theme(prompt, theme == 'dark')
        assert_task_dialog_client_theme(prompt, theme == 'dark')
        assert_readable_native_client(prompt)
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


def filename_field(dialog):
    fields = []

    def visit(hwnd, _):
        if n.is_visible(hwnd) and n.class_name(hwnd) == 'Edit' and n.get_id(hwnd) in (1148, 1152):
            fields.append(hwnd)
        return True

    win32gui.EnumChildWindows(dialog, visit, None)
    assert len(fields) <= 1, ('ambiguous native filename field', fields)
    return fields[0] if fields else None


def file_picker_colors(dialog, field, entry):
    """Measure native filename and folder surfaces without fixing an exact palette."""
    origin = win32gui.ClientToScreen(dialog, (0, 0))
    _, _, width, height = n.rectangle(dialog, client=True)
    view = entry.GetParentControl()
    for _ in range(10):
        if not view or view.ControlType in (auto.ControlType.ListControl,
                                            auto.ControlType.DataGridControl):
            break
        view = view.GetParentControl()
    surfaces = {'filename': auto.ControlFromHandle(field).BoundingRectangle}
    if view and view.ControlType in (auto.ControlType.ListControl, auto.ControlType.DataGridControl):
        surfaces['contents'] = view.BoundingRectangle
    navigation = auto.ControlFromHandle(dialog).TreeControl(searchDepth=12)
    if navigation.Exists(0, 0):
        surfaces['navigation'] = navigation.BoundingRectangle
    for identifier in (win32con.IDOK, win32con.IDCANCEL):
        button = n.get_item(dialog, identifier)
        assert button, ('native file command unavailable', identifier)
        surfaces['command_%s' % identifier] = auto.ControlFromHandle(button).BoundingRectangle

    def combo_surface(hwnd, _):
        if n.is_visible(hwnd) and n.class_name(hwnd) == 'ComboBox':
            surfaces['combo_%s' % len(surfaces)] = auto.ControlFromHandle(hwnd).BoundingRectangle
        return True

    win32gui.EnumChildWindows(dialog, combo_surface, None)
    result = {}
    for name, bounds in surfaces.items():
        rect = (max(3, bounds.left - origin[0] + 3),
                max(3, bounds.top - origin[1] + 3),
                min(width - 3, bounds.right - origin[0] - 3),
                min(height - 3, bounds.bottom - origin[1] - 3))
        assert rect[0] < rect[2] and rect[1] < rect[3], ('native surface not visible', name, rect)
        palette = colors(dialog, rect)
        dominant = palette.most_common(1)[0][0]
        result[name] = {'rect': rect, 'colors_bgr': palette.most_common(8),
                        'dominant_luminance': sum(a * b for a, b in zip(dominant, (.0722, .7152, .2126)))}
    rect = (3, height - 8, width - 3, height - 3)
    palette = colors(dialog, rect)
    dominant = palette.most_common(1)[0][0]
    result['command_pane'] = {
        'rect': rect, 'colors_bgr': palette.most_common(8),
        'dominant_luminance': sum(a * b for a, b in zip(dominant, (.0722, .7152, .2126)))}
    return result


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
        # Reopen after changing the owner theme, guarding against a stale per-process preference.
        for current in (theme, 'dark' if theme == 'light' else 'light'):
            if current != theme:
                sheet = app.settings()
                select_theme(app.settings_page(sheet, 0), 2 if current == 'dark' else 1)
                app.close_settings(sheet)
            before = profile.preferences()
            assert before[2] == current
            n.wait(lambda: dark_background(app.search) == (current == 'dark'),
                   'owner theme visible before opening picker')
            status = app.status_text()
            app.command(1020)
            dialog = n.wait(lambda: native_dialog(app), 'native file picker')
            assert_modal(app, dialog)
            n.wait(lambda: win32gui.GetForegroundWindow() == dialog, 'file picker foreground')
            field = n.wait(lambda: filename_field(dialog), 'native filename field')
            value = n.c.create_unicode_buffer(str(folder) + '\\')
            assert n.send(field, win32con.WM_SETTEXT, 0, n.c.addressof(value))
            button = n.wait(lambda: n.get_item(dialog, win32con.IDOK), 'native Open button')
            win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDOK, button)
            root = auto.ControlFromHandle(dialog)
            entry = root.Control(searchDepth=12,
                                 Compare=lambda item, depth: item.Name in (sentinel.name, sentinel.stem))
            assert entry.Exists(10), 'native folder navigation must display its contents'
            assert_modal(app, dialog)
            assert_titlebar_theme(dialog, current == 'dark')
            assert_readable_native_client(dialog)
            artifact = 'file-picker-%s-%s-from-%s' % (current, dpi, theme)
            screenshot(dialog, settings[2] / (artifact + '.png'))
            evidence = {'theme': current, 'dpi': dpi,
                        'surfaces': file_picker_colors(dialog, field, entry)}
            (settings[2] / (artifact + '-colors.json')).write_text(
                json.dumps(evidence, indent=2), encoding='utf-8')
            invoke(n.get_item(dialog, win32con.IDCANCEL))
            n.wait(lambda: not n.is_window(dialog) and win32gui.IsWindowEnabled(app.frame),
                   'cancel native file picker')
            assert not native_dialog(app)
            assert app.status_text() == status, 'navigation and Cancel must not load a source'
            assert profile.preferences() == before
            n.wait(lambda: dark_background(app.search) == (current == 'dark'),
                   'owner repaint preserves theme after picker closes')
            assert 'contents' in evidence['surfaces'], 'native folder surface must be measurable'
            for surface, measured in evidence['surfaces'].items():
                assert (measured['dominant_luminance'] < 128) == (current == 'dark'), (
                    'file picker client surface must follow the selected theme',
                    surface, current, measured)

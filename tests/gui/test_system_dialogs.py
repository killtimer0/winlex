"""Native message/file dialog contracts, including their public theme boundary."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import sys
import time
import uuid

import pytest
import uiautomation as auto
import win32con
import win32gui
import win32process
import winreg

from support import deadline, native as n
from support.diagnostics import assert_unobscured, screenshot
from support.input_state import GA_ROOT, InputState, foreground, get_ancestor
from support.session import invoke, message_button
from test_theme import background_pixels, modern_theme, select_theme
from test_theme_surfaces import observed_colors as colors


def native_dialog(app):
    found = []

    def visit(hwnd, _):
        if (win32process.GetWindowThreadProcessId(hwnd)[1] == app.process.pid
                and n.is_visible(hwnd) and n.class_name(hwnd) == '#32770'
                and win32gui.GetWindow(hwnd, win32con.GW_OWNER) == app.frame):
            found.append(hwnd)
        return True

    for hwnd in n.top_level_windows():
        visit(hwnd, None)
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


def assert_native_content_painted(dialog):
    _, _, width, height = n.rectangle(dialog, client=True)
    palette = colors(dialog, (3, 3, width - 3, height - 3))
    background = palette.most_common(1)[0][0]
    assert any(color != background for color in palette), 'native client content must be painted'


@deadline.operation('read current native TaskDialog theme', 5)
def native_task_dialog_primary_fill(dialog):
    """Read the public native TaskDialog primary-panel color as BGR."""
    # UxTheme caches caller-process state. Opening a new HTHEME is insufficient
    # while real theme-change notifications remain queued on this STA thread.
    # Dispatch pending messages only; never synthesize a theme notification or
    # treat an empty queue as completion of the system's theme transition.
    n.dispatch_pending_messages()
    ux = n.c.WinDLL('uxtheme')
    theme = n.api(ux, 'OpenThemeData', n.c.c_void_p, n.w.HWND, n.w.LPCWSTR)(
        dialog, 'TaskDialog')
    assert theme, 'native TaskDialog theme unavailable'
    try:
        color = n.w.DWORD()
        # vssym32.h: TDLG_PRIMARYPANEL = 1; TMT_FILLCOLOR = 3802.
        status = n.api(ux, 'GetThemeColor', n.c.c_long, n.c.c_void_p,
                       n.c.c_int, n.c.c_int, n.c.c_int, n.c.POINTER(n.w.DWORD))(
                           theme, 1, 0, 3802, n.c.byref(color))
        assert status >= 0, ('native TaskDialog primary fill unavailable', status)
        # COLORREF is 0x00BBGGRR; captured pixel tuples are BGR.
        return (color.value >> 16 & 255, color.value >> 8 & 255, color.value & 255)
    finally:
        n.api(ux, 'CloseThemeData', n.c.c_long, n.c.c_void_p)(theme)


@deadline.operation('observe TaskDialog client theme', 5)
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
            n.wait(primary_painted, 'native light TaskDialog primary fill is displayed', 5)
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
        app = launch(dpi=dpi, metadata_prompt=True)
        prompt = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
        with n.physical_dpi_context():
            left, top, right, bottom = n.titlebar_rectangle(prompt)
            inputs.click((left + right) // 2, (top + bottom) // 2)
        n.wait(lambda: win32gui.GetForegroundWindow() == prompt, 'sources prompt foreground')
        assert_modal(app, prompt)
        assert_titlebar_theme(prompt, theme == 'dark')
        assert_native_content_painted(prompt)
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
            app = launch(dpi=dpi, metadata_prompt=True)
            prompt = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
            with deadline.operation('activate sources TaskDialog via UIA', 5):
                auto.ControlFromHandle(prompt).SetActive(waitTime=0)
            with n.physical_dpi_context():
                left, top, right, bottom = n.titlebar_rectangle(prompt)
                point = ((left + right) // 2, (top + bottom) // 2)
                hit = win32gui.WindowFromPoint(point)
                assert get_ancestor(hit, GA_ROOT) == prompt, 'TaskDialog title bar is obscured'
                if win32gui.GetForegroundWindow() != prompt:
                    inputs.click(*point)
            n.wait(lambda: win32gui.GetForegroundWindow() == prompt, 'TaskDialog foreground', 5)
            appearance = task_dialog_appearance(prompt, native_light=theme == 'light')
            if theme in appearances:
                assert appearance == appearances[theme], 'client theme roundtrip restores its appearance'
            elif appearances:
                assert appearance != next(iter(appearances.values())), 'client follows the selected theme'
            appearances[theme] = appearance
            dismiss_no(app, prompt)
            app.close()


def filename_field(dialog):
    fields = []

    def visit(hwnd, _):
        if n.is_visible(hwnd) and n.class_name(hwnd) == 'Edit' and n.get_id(hwnd) in (1148, 1152):
            fields.append(hwnd)
        return True

    win32gui.EnumChildWindows(dialog, visit, None)
    assert len(fields) <= 1, ('ambiguous native filename field', fields)
    return fields[0] if fields else None


@contextmanager
def picker_uia_calls():
    """Observe swallowed REGDB_E_IIDNOTREG on this thread; never handle it."""
    class ExceptionRecord(n.c.Structure):
        _fields_ = [('code', n.w.DWORD), ('flags', n.w.DWORD),
                    ('record', n.c.c_void_p), ('address', n.c.c_void_p)]

    class ExceptionPointers(n.c.Structure):
        _fields_ = [('record', n.c.POINTER(ExceptionRecord)), ('context', n.c.c_void_p)]

    callback_type = n.c.WINFUNCTYPE(n.c.c_long, n.c.POINTER(ExceptionPointers))
    get_thread = n.api(n.kernel, 'GetCurrentThreadId', n.w.DWORD)
    thread = get_thread()
    first = []

    @callback_type
    def observe(pointers):
        record = pointers.contents.record.contents
        if record.code == 0x80040155 and get_thread() == thread and not first:
            first.append(record.address)
        return 0  # EXCEPTION_CONTINUE_SEARCH: preserve handlers/faulthandler.

    add = n.api(n.kernel, 'AddVectoredExceptionHandler', n.c.c_void_p,
                n.w.ULONG, callback_type)
    remove = n.api(n.kernel, 'RemoveVectoredExceptionHandler', n.w.ULONG, n.c.c_void_p)
    token = add(1, observe)
    assert token, 'Cannot observe native picker traversal exceptions'

    def call(operation, *args):
        started = time.monotonic()
        try:
            return operation(*args)
        finally:
            if first:
                evidence = {'code': '0x80040155', 'address': first[0], 'thread': thread,
                            'call_seconds': time.monotonic() - started,
                            'operation': getattr(operation, '__name__', type(operation).__name__)}
                ledger = os.environ.get('WINLEX_TEST_LEDGER')
                if ledger:
                    path = Path(ledger).parent / ('picker-native-exception-' + uuid.uuid4().hex + '.json')
                    path.write_text(json.dumps(evidence, indent=2), encoding='utf-8')
                    evidence['artifact'] = str(path)
                raise AssertionError(('First native exception during picker UIA traversal', evidence))

    try:
        yield call
    finally:
        assert remove(token), 'Cannot remove picker exception observer'


@deadline.operation('locate native folder entry via UIA', 5)
def picker_entry(dialog, sentinel):
    # Exists/FindControl can traverse past native exceptions swallowed below
    # Python. Check each provider call instead of retrying that whole search.
    with picker_uia_calls() as call:
        root = call(auto.ControlFromHandle, dialog)

        def find(parent, depth, in_list=False):
            child = call(parent.GetFirstChildControl)
            while child:
                kind = call(lambda: child.ControlType)
                # Non-client titlebars cannot contain folder entries. In
                # particular, do not ask their provider for children or names.
                if kind != auto.ControlType.TitleBarControl:
                    if (in_list and kind in (auto.ControlType.ListItemControl,
                                             auto.ControlType.DataItemControl)
                            and call(lambda: child.Name) in (sentinel.name, sentinel.stem)):
                        return child
                    if depth < 12:
                        found = find(child, depth + 1, in_list or kind in (
                            auto.ControlType.ListControl, auto.ControlType.DataGridControl))
                        if found:
                            return found
                child = call(child.GetNextSiblingControl)
            return None

        return find(root, 1)


@deadline.operation('measure native file picker surfaces via UIA', 5)
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
        result[name] = {'rect': rect, 'colors_bgr': palette.most_common(8)}
    rect = (3, height - 8, width - 3, height - 3)
    palette = colors(dialog, rect)
    result['command_pane'] = {
        'rect': rect, 'colors_bgr': palette.most_common(8)}
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
        appearances = []
        owner = background_pixels(app.search)
        # Reopen after changing the owner theme, guarding against a stale per-process preference.
        for current in (theme, 'dark' if theme == 'light' else 'light'):
            if current != theme:
                sheet = app.settings()
                select_theme(app.settings_page(sheet, 0), 2 if current == 'dark' else 1)
                app.close_settings(sheet)
                n.wait(lambda: background_pixels(app.search) != owner, 'owner paints changed theme')
            before = profile.preferences()
            assert before[2] == current
            owner = background_pixels(app.search)
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
            entry = n.wait(lambda: picker_entry(dialog, sentinel), 'native folder navigation contents', 10)
            assert_modal(app, dialog)
            assert_titlebar_theme(dialog, current == 'dark')
            assert_native_content_painted(dialog)
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
            n.wait(lambda: background_pixels(app.search) == owner,
                   'owner repaint preserves theme after picker closes')
            assert 'contents' in evidence['surfaces'], 'native folder surface must be measurable'
            appearance = {name: value['colors_bgr'][0][0] for name, value in evidence['surfaces'].items()}
            if appearances:
                assert appearance != appearances[-1], 'native picker follows the owner theme change'
            appearances.append(appearance)

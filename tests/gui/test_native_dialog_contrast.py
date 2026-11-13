"""Live native dialogs must survive real high-contrast entry and exit."""
import json
import time

import pytest
import uiautomation as auto
import win32con
import win32gui
import winreg
from comtypes import COMError

from support import deadline, native as n
from support.diagnostics import screenshot
from support.input_state import GA_ROOT, InputState, foreground, get_ancestor
from support.session import invoke, message_button
from test_system_dialogs import (
    assert_modal, assert_native_content_painted,
    dismiss_no, file_picker_colors, filename_field, native_dialog, picker_entry,
)
from test_theme import modern_theme
from test_theme_compatibility import (
    HCF_AVAILABLE, HCF_HIGHCONTRASTON, high_contrast, restored_high_contrast, system_color,
    temporary_high_contrast,
)
from test_theme_surfaces import observed_colors as colors


pytestmark = [modern_theme, pytest.mark.interactive]


def contrast_snapshot():
    snapshot = high_contrast()
    if snapshot[0] & HCF_HIGHCONTRASTON:
        pytest.skip('high contrast already enabled; preserve the existing desktop')
    if not snapshot[0] & HCF_AVAILABLE:
        pytest.skip('system high contrast is unavailable')
    return snapshot


@deadline.operation('measure live TaskDialog surfaces via UIA', 5)
def task_surfaces(dialog):
    text = auto.ControlFromHandle(dialog).TextControl(AutomationId='ContentText')
    if not text.Exists(0, 0):
        raise LookupError('native TaskDialog content text is not available yet')
    bounds = text.BoundingRectangle
    # UIA bounds are physical pixels even when the target's native geometry
    # is virtualized. capture_pixels expects rectangles in the target space.
    convert = n.api(n.user, 'PhysicalToLogicalPointForPerMonitorDPI', n.w.BOOL,
                    n.w.HWND, n.c.POINTER(n.w.POINT))
    first = n.w.POINT(bounds.left, bounds.top)
    last = n.w.POINT(bounds.right, bounds.bottom)
    assert convert(dialog, n.c.byref(first)), 'Cannot convert TaskDialog text bounds'
    assert convert(dialog, n.c.byref(last)), 'Cannot convert TaskDialog text bounds'
    with n.window_dpi_context(dialog):
        _, _, width, height = n.rectangle(dialog, client=True)
        rects = {'content': (3, 3, width - 3, height // 2),
                 'commands': (3, height // 2, width - 3, height - 3)}
        origin = win32gui.ClientToScreen(dialog, (0, 0))
        rects['text'] = (first.x - origin[0], first.y - origin[1],
                         last.x - origin[0], last.y - origin[1])
        commands = {}
        for identifier in (win32con.IDYES, win32con.IDNO):
            button = message_button(dialog, identifier)
            hwnd = button.NativeWindowHandle if button is not None else None
            if not hwnd:
                raise LookupError('native TaskDialog command is not available yet')
            left, top, right, bottom = n.rectangle(hwnd)
            name = 'command_%s' % identifier
            commands[name] = hwnd
            rects[name] = (
                left - origin[0] + 5, top - origin[1] + 5,
                right - origin[0] - 5, bottom - origin[1] - 5)
    surfaces = {name: {'rect': rect} for name, rect in rects.items()}
    for name, hwnd in commands.items():
        surfaces[name]['hwnd'] = hwnd
    return surfaces


def task_text_painted(surfaces):
    palette = surfaces['text']['colors_bgr']
    background = palette[0][0]
    return any(color != background
               for color, count in palette)


def surface_backgrounds(surfaces):
    return {name: value['colors_bgr'][0][0] for name, value in surfaces.items()}


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


@deadline.operation('verify live TaskDialog accessible commands', 5)
def assert_prompt_contract(app, dialog):
    assert n.is_window(dialog) and native_dialog(app) == dialog
    assert_modal(app, dialog)
    last_error = None

    def commands_ready():
        nonlocal last_error
        try:
            yes = message_button(dialog, win32con.IDYES)
            no = message_button(dialog, win32con.IDNO)
        except COMError as error:
            # Retain the accessibility contract, but do not poll providers
            # throughout the much longer native painting deadline.
            last_error = repr(error)
            return None
        return (yes, no) if yes is not None and no is not None else None

    try:
        yes, no = n.wait(commands_ready, 'live TaskDialog command providers ready', 5)
    except AssertionError as error:
        raise AssertionError(('TaskDialog command providers unavailable', last_error)) from error
    for provider, default in ((yes, True), (no, False)):
        hwnd = provider.NativeWindowHandle
        assert hwnd, 'native TaskDialog command has no HWND'
        assert bool(n.send(hwnd, win32con.WM_GETDLGCODE) & 0x10) == default


@pytest.mark.parametrize('dpi', ['pmv1', 'pmv2'])
def test_task_dialog_stays_open_across_high_contrast(launch, profile, settings, dpi, hc_appearance):
    from test_system_dialogs import native_task_dialog_primary_fill

    snapshot = contrast_snapshot()
    profile.set('Theme', winreg.REG_SZ, 'dark')
    profile.set('AutoLoad', winreg.REG_MULTI_SZ, [])
    before = profile.preferences()
    with InputState() as inputs:
        app = launch(dpi=dpi, metadata_prompt=True)
        dialog = n.wait(lambda: native_dialog(app), 'sources TaskDialog')
        with n.physical_dpi_context():
            left, top, right, bottom = n.titlebar_rectangle(dialog)
            inputs.click((left + right) // 2, (top + bottom) // 2)
        assert_prompt_contract(app, dialog)
        reference = task_surfaces(dialog)
        obscured_since = None
        native_sample = {}
        previous_pending = None

        def native_command_states():
            result = {}
            for name, value in reference.items():
                if 'hwnd' not in value:
                    continue
                # UIA's command AutomationId is not a Win32 control ID.
                # Retain its actual HWND for passive native state observation.
                hwnd = value['hwnd']
                style = win32gui.GetWindowLong(hwnd, win32con.GWL_STYLE)
                result[name] = {'hwnd': hwnd, 'style': style,
                                'default': style & 0xF == win32con.BS_DEFPUSHBUTTON}
            return result

        def native_panel_fills():
            # HC dialog command areas use the system dialog-face role. The
            # secondary theme part need not expose TMT_FILLCOLOR (19045 does
            # not), so availability of that optional property is not required.
            return {'primary': native_task_dialog_primary_fill(dialog),
                    'secondary': system_color(win32con.COLOR_BTNFACE)}

        def native_contrast_painted(surfaces):
            nonlocal previous_pending
            fills = native_panel_fills()
            palette = native_sample['palette_before']
            commands = native_sample['commands_before']
            backgrounds = surface_backgrounds(surfaces)
            expected = {'content': (fills['primary'],), 'text': (fills['primary'],),
                        'commands': (fills['secondary'],)}
            for name, command in commands.items():
                expected[name] = (palette['button_face'],)
                if command['default']:
                    expected[name] += (palette['highlight'],)
            pending = dict(native_sample, native_panel_fills=fills,
                           expected_backgrounds=expected, surfaces=surfaces)
            if pending != previous_pending:
                (settings[2] / ('task-live-%s-hc-pending-colors.json' % dpi)).write_text(
                    json.dumps(pending, indent=2), encoding='utf-8')
                previous_pending = pending
            # SPI can report HC before the native theme and system colors have
            # finished changing. Require the OS theme's panel fills to agree
            # with its HC window/dialog roles, not with a prior screen sample.
            return (native_sample['high_contrast_before']
                    and native_sample['high_contrast_after']
                    and bool(high_contrast()[0] & HCF_HIGHCONTRASTON)
                    and palette == native_sample['palette_after'] == picker_native_palette()
                    and commands == native_sample['commands_after']
                    and all(command['hwnd'] for command in commands.values())
                    and fills == {'primary': palette['window'],
                                  'secondary': palette['button_face']}
                    and all(backgrounds[name] in allowed for name, allowed in expected.items())
                    and task_text_painted(surfaces))

        @deadline.operation('observe native TaskDialog repaint', 5)
        def observe():
            nonlocal obscured_since, native_sample
            palette = picker_native_palette()
            commands = native_command_states()
            active = bool(high_contrast()[0] & HCF_HIGHCONTRASTON)
            visibility = picker_capture_visibility(dialog, reference)
            unobscured = visibility['visible'] and not visibility['occluders']
            surfaces = ({name: dict(value,
                                   colors_bgr=colors(dialog, value['rect']).most_common(8))
                         for name, value in reference.items()} if unobscured else {})
            after = picker_capture_visibility(dialog, reference) if unobscured else visibility
            if not unobscured or not after['visible'] or after['occluders']:
                if obscured_since is None:
                    obscured_since = time.monotonic()
                if time.monotonic() - obscured_since >= 5:
                    (settings[2] / ('task-live-%s-capture-unavailable.json' % dpi)).write_text(
                        json.dumps({'visibility_before': visibility,
                                    'visibility_after': after}, indent=2), encoding='utf-8')
                    raise AssertionError(('TaskDialog capture remains unavailable',
                                          visibility, after))
                raise LookupError('TaskDialog capture is obscured')
            obscured_since = None
            if visibility['regions'] != after['regions']:
                raise LookupError('TaskDialog moved during capture')
            native_sample = {'palette_before': palette, 'palette_after': picker_native_palette(),
                             'commands_before': commands,
                             'commands_after': native_command_states(),
                             'high_contrast_before': active,
                             'high_contrast_after': bool(high_contrast()[0] & HCF_HIGHCONTRASTON)}
            return surfaces

        initial = settled_surfaces(observe, task_text_painted,
                                   'initial TaskDialog content is painted')
        with temporary_high_contrast(snapshot, settings[2] / ('task-live-%s-restore.json' % dpi), hc_appearance):
            assert n.is_window(dialog) and native_dialog(app) == dialog
            assert_modal(app, dialog)
            surfaces = settled_surfaces(observe,
                                        native_contrast_painted,
                                        'open TaskDialog repaints in native high contrast',
                                        (dialog, settings[2], 'task-live-%s-hc-failed' % dpi))
            record(dialog, settings[2], 'task-live-%s-hc' % dpi, surfaces)
            assert_prompt_contract(app, dialog)
            assert_native_content_painted(dialog)
            inputs.key(win32con.VK_ESCAPE)
            assert_prompt_contract(app, dialog)
        assert restored_high_contrast(high_contrast(), snapshot)
        assert n.is_window(dialog) and native_dialog(app) == dialog
        assert_modal(app, dialog)
        surfaces = settled_surfaces(observe,
                                    lambda value: surface_backgrounds(value) == surface_backgrounds(initial)
                                    and task_text_painted(value),
                                    'same TaskDialog recovers dark text and panels',
                                    (dialog, settings[2], 'task-live-%s-restored-failed' % dpi))
        record(dialog, settings[2], 'task-live-%s-restored' % dpi, surfaces)
        assert_prompt_contract(app, dialog)
        assert_native_content_painted(dialog)
        dismiss_no(app, dialog)
        assert n.is_visible(app.search)
        assert profile.preferences() == before


def navigate(dialog, folder, sentinel):
    field = n.wait(lambda: filename_field(dialog), 'native filename input')
    # HWND existence and changed pixels can precede completion of the native
    # theme broadcast. Wait without mutating the input until its thread can
    # process messages; a failed write is not a readiness poll.
    n.wait(lambda: n.send_timeout(field, win32con.WM_NULL, 0, 0, 3, 50, None),
           'native filename responds before navigation', 5)
    value = n.c.create_unicode_buffer(str(folder) + '\\')
    assert n.send(field, win32con.WM_SETTEXT, 0, n.c.addressof(value))
    button = n.wait(lambda: n.get_item(dialog, win32con.IDOK), 'native Open button')
    win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDOK, button)
    n.wait(lambda: picker_entry(dialog, sentinel), 'native navigation reveals expected file', 10)


@deadline.operation('measure live native picker surfaces via UIA', 5)
def picker_surfaces(dialog, sentinel):
    field = filename_field(dialog)
    entry = picker_entry(dialog, sentinel) if field else None
    if not field or not entry:
        raise LookupError('native folder contents are not available yet')
    return file_picker_colors(dialog, field, entry)


@deadline.operation('observe native picker repaint', 5)
def observe_picker_surfaces(dialog, reference):
    # Keep only screen regions, not provider elements. The theme broadcast
    # rebuilds UIA providers; querying them to decide whether repaint finished
    # can itself block the transition we are trying to observe.
    return {name: {'rect': value['rect'],
                   'colors_bgr': colors(dialog, value['rect']).most_common(8)}
            for name, value in reference.items()}


def picker_native_palette():
    return {name: system_color(role) for name, role in (
        ('window', win32con.COLOR_WINDOW), ('button_face', win32con.COLOR_BTNFACE),
        ('highlight', win32con.COLOR_HIGHLIGHT))}


def picker_command_states(dialog, reference):
    result = {}
    for name in reference:
        if name.startswith('command_') and name != 'command_pane':
            hwnd = n.get_item(dialog, int(name[len('command_'):]))
            style = win32gui.GetWindowLong(hwnd, win32con.GWL_STYLE) if hwnd else None
            result[name] = {'hwnd': hwnd, 'style': style,
                            'default': style is not None
                            and style & 0xF == win32con.BS_DEFPUSHBUTTON}
    return result


def picker_native_backgrounds(surfaces, palette, commands):
    # GetSysColor defines WINDOW for input/list/tree interiors and BTNFACE
    # for push buttons and dialog backgrounds. A whole ComboBox rectangle
    # includes both its selection/edit field and its drop-down button, so
    # either of those two native background roles can dominate that sample.
    # https://learn.microsoft.com/windows/win32/winauto/high-contrast-parameter
    for name, background in surface_backgrounds(surfaces).items():
        if name in ('filename', 'contents', 'navigation'):
            allowed = (palette['window'],)
        elif name.startswith('combo_'):
            allowed = (palette['window'], palette['button_face'])
        else:
            assert name == 'command_pane' or name.startswith('command_'), name
            allowed = (palette['button_face'],)
            # HC can fill the default command with the native selection color.
            # Admit that role only with passive BS_DEFPUSHBUTTON state proof,
            # not for every button or for the command pane.
            if commands.get(name, {}).get('default'):
                allowed += (palette['highlight'],)
        if background not in allowed:
            return False
    return True


def picker_capture_visibility(dialog, reference):
    """Check ownership of the displayed regions without activating the dialog."""
    with n.window_dpi_context(dialog):
        origin = win32gui.ClientToScreen(dialog, (0, 0))
        regions = {}
        for name, value in reference.items():
            left, top, right, bottom = value['rect']
            first = n.physical_screen_point(dialog, (origin[0] + left, origin[1] + top))
            last = n.physical_screen_point(dialog, (origin[0] + right, origin[1] + bottom))
            regions[name] = (*first, *last)
    with n.physical_dpi_context():
        identities = {}

        def identity(hwnd):
            if hwnd not in identities:
                pid = n.w.DWORD()
                tid = n.get_pid(hwnd, n.c.byref(pid)) if hwnd else 0
                identities[hwnd] = ({'hwnd': hwnd, 'class': n.class_name(hwnd),
                                     'pid': pid.value, 'tid': tid,
                                     'rect': n.rectangle(hwnd)} if hwnd else {'hwnd': 0})
            return identities[hwnd]

        foreground_window = identity(win32gui.GetForegroundWindow())
        occluders = []
        for name, (left, top, right, bottom) in regions.items():
            # Hit-test each sample's corners and centre, not just the dialog
            # centre. Child HWNDs belong to the dialog; owned popups do not.
            for point in ((left, top), (right - 1, top),
                          (left, bottom - 1), (right - 1, bottom - 1),
                          ((left + right - 1) // 2, (top + bottom - 1) // 2)):
                hit = win32gui.WindowFromPoint(point)
                root = get_ancestor(hit, GA_ROOT) if hit else 0
                if root != dialog:
                    occluders.append({'surface': name, 'point': point,
                                      'window': identity(root), 'hit_hwnd': hit})
        # WindowFromPoint skips disabled windows and hit testing can skip
        # transparent overlays. Also reject rectangle intersections with any
        # opaque visible top-level window above the dialog. Layered windows
        # can have alpha holes, so retain the hit tests for those instead.
        above = win32gui.GetWindow(dialog, win32con.GW_HWNDPREV)
        while above:
            if n.is_visible(above) and not win32gui.IsIconic(above):
                style = win32gui.GetWindowLong(above, win32con.GWL_EXSTYLE)
                if not style & win32con.WS_EX_LAYERED:
                    window = identity(above)
                    left, top, right, bottom = window['rect']
                    for name, (x1, y1, x2, y2) in regions.items():
                        if max(left, x1) < min(right, x2) and max(top, y1) < min(bottom, y2):
                            occluders.append({'surface': name, 'window': window,
                                              'reason': 'opaque window above sample'})
            above = win32gui.GetWindow(above, win32con.GW_HWNDPREV)
        return {'visible': bool(n.is_visible(dialog)) and not win32gui.IsIconic(dialog),
                'regions': regions, 'foreground': foreground_window, 'occluders': occluders}


def settled_picker_surfaces(dialog, reference, description, evidence,
                            native_before=None, restored=None, initial=False, seconds=45):
    """Wait for native painting, without repeatedly querying UIA providers."""
    last = {}
    previous = None
    obscured_since = None
    output, phase = evidence

    def ready():
        nonlocal last, previous, obscured_since
        palette = picker_native_palette()
        commands = picker_command_states(dialog, reference)
        visibility = picker_capture_visibility(dialog, reference)
        unobscured = visibility['visible'] and not visibility['occluders']
        surfaces = observe_picker_surfaces(dialog, reference) if unobscured else {}
        after = picker_capture_visibility(dialog, reference) if unobscured else visibility
        commands_after = picker_command_states(dialog, reference)
        active = bool(high_contrast()[0] & HCF_HIGHCONTRASTON)
        last = {'high_contrast': active, 'native_palette': palette, 'surfaces': surfaces,
                'visibility_before': visibility, 'visibility_after': after,
                'commands_before': commands, 'commands_after': commands_after}
        signature = (active, palette, surface_backgrounds(surfaces), visibility, after,
                     commands, commands_after)
        if signature != previous:
            # Retain the latest partial paint before any later UIA call can
            # block. Overwrite one bounded artifact, not a per-poll trace.
            (output / (phase + '-pending-colors.json')).write_text(
                json.dumps(last, indent=2), encoding='utf-8')
            previous = signature
        if not unobscured or not after['visible'] or after['occluders']:
            if obscured_since is None:
                obscured_since = time.monotonic()
            if time.monotonic() - obscured_since >= 5:
                raise AssertionError(('native picker capture remains unavailable', after))
            return None
        obscured_since = None
        if visibility['regions'] != after['regions'] or commands != commands_after:
            return None
        # Reject a sample taken across a system palette change. The HC flag
        # alone can precede theme application; the native palette must also
        # acknowledge the entry transition before its roles are an oracle.
        if palette != picker_native_palette():
            return None
        if initial:
            complete = not active
        elif restored is not None:
            complete = not active and surface_backgrounds(surfaces) == restored
        else:
            complete = (active and palette != native_before
                        and picker_native_backgrounds(surfaces, palette, commands))
        return surfaces if complete else None

    try:
        return n.wait(ready, description, seconds)
    except AssertionError as error:
        record(dialog, output, phase + '-failed', last.get('surfaces', {}))
        raise AssertionError((description, 'last native paint sample', last)) from error


@pytest.mark.parametrize('dpi', ['pmv1', 'pmv2'])
def test_file_picker_stays_open_across_high_contrast(launch, profile, settings, tmp_path, dpi, hc_appearance):
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
        navigate(dialog, *folders[0])
        reference = picker_surfaces(dialog, folders[0][1])
        assert 'contents' in reference, 'native folder surface must be measurable'
        initial = settled_picker_surfaces(
            dialog, reference, 'initial picker surfaces are unobscured',
            (settings[2], 'picker-live-%s-initial' % dpi), initial=True, seconds=5)
        native_before = picker_native_palette()
        with temporary_high_contrast(snapshot, settings[2] / ('picker-live-%s-restore.json' % dpi), hc_appearance):
            assert n.is_window(dialog) and native_dialog(app) == dialog
            assert_modal(app, dialog)
            surfaces = settled_picker_surfaces(
                dialog, initial, 'open picker repaints in native high contrast before navigation',
                (settings[2], 'picker-live-%s-hc-before-navigation' % dpi),
                native_before=native_before)
            record(dialog, settings[2], 'picker-live-%s-hc-before-navigation' % dpi, surfaces)
            navigate(dialog, *folders[1])
            assert_modal(app, dialog)
            # Reacquire the visible file and valid surface geometry once under
            # the existing five-second UIA deadline; only pixels are polled.
            reference = picker_surfaces(dialog, folders[1][1])
            assert 'contents' in reference, 'native folder surface must be measurable'
            surfaces = settled_picker_surfaces(
                dialog, reference, 'native navigation retains high contrast',
                (settings[2], 'picker-live-%s-hc-after-navigation' % dpi),
                native_before=native_before, seconds=5)
        assert restored_high_contrast(high_contrast(), snapshot)
        assert n.is_window(dialog) and native_dialog(app) == dialog
        assert_modal(app, dialog)
        surfaces = settled_picker_surfaces(
            dialog, surfaces, 'same picker recovers dark colors before navigation',
            (settings[2], 'picker-live-%s-restored-before-navigation' % dpi),
            restored=surface_backgrounds(initial))
        record(dialog, settings[2], 'picker-live-%s-restored-before-navigation' % dpi, surfaces)
        navigate(dialog, *folders[2])
        reference = picker_surfaces(dialog, folders[2][1])
        assert 'contents' in reference, 'native folder surface must be measurable'
        surfaces = settled_picker_surfaces(
            dialog, reference, 'native navigation retains recovered dark colors',
            (settings[2], 'picker-live-%s-restored-after-navigation' % dpi),
            restored=surface_backgrounds(initial), seconds=5)
        invoke(n.get_item(dialog, win32con.IDCANCEL))
        n.wait(lambda: not n.is_window(dialog) and win32gui.IsWindowEnabled(app.frame), 'native Cancel')
        assert not native_dialog(app)
        assert n.is_visible(app.search)
        assert app.status_text() == status
        assert profile.preferences() == before

"""Legacy native fallback and real high-contrast restoration preserve preferences."""
import ctypes as c
from ctypes import wintypes as w
import json
import time

import pytest
import win32con
import win32gui
import winreg

from support import native as n
from support.diagnostics import screenshot
from support.input_state import InputState, foreground
from driver.appearance import THEME_SUPPORTED, modern_theme, settled_colors
from driver.controls import combo_info
from driver.high_contrast import (
    assert_dark_preference, edit_background, set_edit_text, system_color,
    temporary_high_contrast,
)
from driver.high_contrast_watchdog import (
    HCF_AVAILABLE, HCF_HIGHCONTRASTON, ObscuredSample, high_contrast,
    restored_high_contrast,
)


pytestmark = pytest.mark.interactive


@pytest.mark.skipif(THEME_SUPPORTED, reason='requires Windows before dark-mode support')
def test_legacy_dark_preference_keeps_native_edit(launch, profile, settings):
    profile.set('Theme', winreg.REG_SZ, 'dark')
    expected = profile.preferences()
    app = launch()
    native = edit_background(app.search)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        text = 'native fallback text'
        set_edit_text(app.search, text)
        assert_dark_preference(app, profile, expected)
        assert edit_background(app.search) == native, 'opening settings preserves native appearance'
        assert n.window_text(app.search) == text
        screenshot(app.frame, settings[2] / 'legacy-dark-preference-native.png')


@modern_theme
def test_actual_high_contrast_overrides_dark_and_restores(launch, profile, settings, hc_appearance):
    from driver.surfaces import observed_colors

    snapshot = high_contrast()
    if snapshot[0] & HCF_HIGHCONTRASTON:
        pytest.skip('high contrast already enabled; do not alter the existing desktop')
    if not snapshot[0] & HCF_AVAILABLE:
        pytest.skip('system high contrast is unavailable')
    profile.set('Theme', winreg.REG_SZ, 'dark')
    expected = profile.preferences()
    app = launch()
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        text = 'high contrast preserves this text'
        evidence = {'before': {'high_contrast': list(snapshot)}}
        evidence_path = settings[2] / 'high-contrast-controls.json'

        def record(stage, values):
            evidence[stage] = values
            evidence_path.write_text(json.dumps(evidence, indent=2), encoding='utf-8')

        set_edit_text(app.search, text)
        regions = {}

        def remember_region(hwnd, region=None):
            _, _, width, height = n.rectangle(hwnd, client=True)
            regions[hwnd] = region or (2, 2, width - 2, height - 2)

        def sample_points(hwnd, region):
            left, top, right, bottom = region
            assert left < right and top < bottom, ('empty native sample', hwnd, region)
            return [win32gui.ClientToScreen(hwnd, point) for point in
                    ((left, top), (right - 1, top), (left, bottom - 1),
                     (right - 1, bottom - 1),
                     ((left + right) // 2, (top + bottom) // 2))]

        def require_ownership(hwnd, points):
            for point in points:
                actual = win32gui.WindowFromPoint(point)
                # WindowFromPoint ignores disabled windows. A hidden modal
                # sheet leaves its owner disabled, so descend from the exact
                # hit root without skipping disabled children. Still reject
                # other top-level windows and overlapping siblings.
                if actual == app.frame and not win32gui.IsWindowEnabled(app.frame):
                    current = actual
                    while current and current != hwnd:
                        child = win32gui.ChildWindowFromPointEx(
                            current, win32gui.ScreenToClient(current, point),
                            0x0001 | 0x0004)  # CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT
                        if not child or child == current:
                            break
                        current = child
                    actual = current
                if actual != hwnd and not (actual and win32gui.IsChild(hwnd, actual)):
                    raise ObscuredSample(str((
                        'native control sample is obscured', hwnd, n.class_name(hwnd),
                        point, actual, n.class_name(actual) if actual else None)))

        def fill(hwnd):
            if hwnd not in regions:
                remember_region(hwnd)
            region = regions[hwnd]
            points = sample_points(hwnd, region)
            obstruction = []

            def sample():
                try:
                    require_ownership(hwnd, points)
                    value = observed_colors(hwnd, region).most_common(1)[0][0]
                    require_ownership(hwnd, points)
                    return value
                except ObscuredSample as error:
                    obstruction[:] = [str(error)]
                    return None

            try:
                return n.wait(sample, 'unobscured native control capture', 15)
            except AssertionError as error:
                message = '%s; obstruction=%r' % (error, obstruction)
                record('failure', {'hwnd': hwnd, 'region': region, 'error': message})
                raise AssertionError(message) from error

        def remember_edit_region():
            _, _, width, height = n.rectangle(app.search, client=True)
            remember_region(app.search, (width - 10, height - 8, width - 5, height - 4))

        remember_edit_region()
        original_edit = settled_colors(lambda: fill(app.search), 'initial edit background settles')

        def wait_fill(hwnd, expected_fills, description):
            observed = []

            def sample():
                value = fill(hwnd)
                observed[:] = [value]
                expected = expected_fills() if callable(expected_fills) else expected_fills
                return value if value in expected else None

            try:
                return n.wait(sample, description, 15)
            except AssertionError as error:
                expected = expected_fills() if callable(expected_fills) else expected_fills
                raise AssertionError('%s: observed=%r, expected=%r' %
                                     (description, observed, expected) +
                                     '; cause=' + str(error)) from error

        dark_status = settled_colors(lambda: fill(app.status), 'initial status background settles')
        sheet = app.settings()

        try:
            page = app.settings_page(sheet, 0)
            combo = n.get_item(page, 1301)
            tabs = []
            win32gui.EnumChildWindows(sheet, lambda hwnd, _: tabs.append(hwnd)
                                     if n.class_name(hwnd) == 'SysTabControl32' else None, None)
            tab = next(iter(tabs), None)
            assert tab, 'settings exposes a native tab control'
            selected = n.send(tab, 0x130B)  # TCM_GETCURSEL; no remote pointers.
            selection = n.send(combo, win32con.CB_GETCURSEL)
            assert selection == 2
            assert not win32gui.FindWindowEx(tab, 0, 'msctls_updown32', None)
            # Property sheets use multiline tabs. Exercise the engine's supported
            # single-line overflow configuration on this already-themed HWND.
            style = win32gui.GetWindowLong(tab, win32con.GWL_STYLE)
            win32gui.SetWindowLong(tab, win32con.GWL_STYLE, style & ~0x0200)  # TCS_MULTILINE
            bounds = win32gui.GetWindowRect(tab)
            origin = win32gui.ScreenToClient(win32gui.GetParent(tab), bounds[:2])
            dpi = n.window_dpi(tab)
            win32gui.MoveWindow(tab, origin[0], origin[1], 80 * dpi // 96,
                                bounds[3] - bounds[1], True)
            overflow = n.wait(lambda: win32gui.FindWindowEx(tab, 0, 'msctls_updown32', None),
                              'native overflow created after initial theme application')
            n.wait(lambda: n.is_visible(overflow), 'late overflow visible')

            def remember_control_regions():
                current_info = combo_info(combo)
                item = current_info.item
                remember_region(combo, (item.right - 12, item.top + 3,
                                        item.right - 4, item.bottom - 3))
                overflow_left = win32gui.ScreenToClient(tab, n.rectangle(overflow)[:2])[0]
                # Exclude the property page, header text and overflow child.
                remember_region(tab, (4, 4, overflow_left - 4, 8))
                remember_region(overflow)
                return current_info

            info = remember_control_regions()

            def idle_pointer():
                left, top, _, _ = n.rectangle(sheet)
                assert n.api(n.user, 'SetCursorPos', w.BOOL, c.c_int, c.c_int)(left + 40, top + 10)

            idle_pointer()

            initial = []

            def settled_dark_controls():
                current = {name: fill(hwnd) for name, hwnd in
                           [('combo', combo), ('tab', tab), ('overflow', overflow)]}
                if not initial or current != initial[0]:
                    initial[:] = [current, time.monotonic()]
                # Do not make a restoration oracle from an animation frame.
                return current if time.monotonic() - initial[1] >= .1 else None

            before_controls = n.wait(settled_dark_controls, 'initial dark control animations settle', 15)
            record('dark_controls', before_controls)
            with temporary_high_contrast(snapshot, settings[2] / 'high-contrast-restore.json', hc_appearance):
                assert high_contrast()[0] & HCF_HIGHCONTRASTON
                idle_pointer()
                info = remember_control_regions()
                observed = {}

                def observe(name, sample):
                    try:
                        observed[name] = {'color': sample()}
                    except AssertionError as error:
                        observed[name] = {'error': str(error)}
                        raise
                    finally:
                        record('high_contrast_observations', dict(
                            observed, high_contrast=list(high_contrast()),
                            system_window=system_color(win32con.COLOR_WINDOW),
                            system_face=system_color(win32con.COLOR_BTNFACE)))

                for name, hwnd in [('combo', combo), ('tab', tab), ('overflow', overflow)]:
                    # HC uses system roles, not the application's dark palette.
                    # Check each surface independently: one recolored control
                    # must not hide stale dark rendering in another control.
                    role = win32con.COLOR_WINDOW if name == 'combo' else win32con.COLOR_BTNFACE
                    observe(name, lambda hwnd=hwnd, role=role: wait_fill(
                        hwnd, lambda: {system_color(role)}, name + ' uses its high-contrast system role'))
                # Recheck every exposed control before delivering an owned gesture.
                for hwnd in (combo, tab, overflow):
                    require_ownership(hwnd, sample_points(hwnd, regions[hwnd]))
                assert high_contrast()[0] & HCF_HIGHCONTRASTON

                def require_sheet_input():
                    active = win32gui.GetForegroundWindow()
                    assert active in (sheet, info.list) or (
                        active and (win32gui.IsChild(sheet, active) or
                                    win32gui.IsChild(info.list, active))), (
                                        'settings no longer owns foreground input', active)

                n.wait(lambda: win32gui.GetForegroundWindow() == sheet,
                       'settings foreground returns after native high contrast transition', 15)
                require_sheet_input()
                button = info.button
                button_points = sample_points(combo, (button.left, button.top,
                                                     button.right, button.bottom))
                require_ownership(combo, button_points)
                point = n.physical_screen_point(combo, button_points[-1])
                with n.physical_dpi_context():
                    require_sheet_input()
                    require_ownership(combo, [point])
                    inputs.click(*point)
                n.wait(lambda: n.is_visible(info.list), 'high-contrast combo popup visible', 15)
                remember_region(info.list)
                observe('combo_list', lambda: wait_fill(
                    info.list, lambda: {system_color(win32con.COLOR_WINDOW)},
                    'combo list uses the high-contrast window background'))
                # On failed observation, unwind immediately instead of injecting
                # cleanup input into a potentially covered or inactive desktop.
                require_sheet_input()
                require_ownership(info.list, sample_points(info.list, regions[info.list]))
                assert n.is_visible(info.list), 'combo popup remains open before Escape'
                inputs.key(win32con.VK_ESCAPE)
                n.wait(lambda: not n.is_visible(info.list), 'Escape closes high-contrast combo popup', 15)
                assert n.send(combo, win32con.CB_GETCURSEL) == selection
                assert n.send(tab, 0x130B) == selected
                assert profile.preferences() == expected
                # Hide the owned sheet only while sampling the main window so
                # displayed pixels cannot come from an overlapping dialog.
                win32gui.ShowWindow(sheet, win32con.SW_HIDE)
                try:
                    remember_edit_region()
                    remember_region(app.status)
                    observe('edit', lambda: wait_fill(
                        app.search, lambda: {system_color(win32con.COLOR_WINDOW)},
                        'edit uses the high-contrast window background'))
                    observe('status', lambda: wait_fill(
                        app.status, lambda: {system_color(win32con.COLOR_BTNFACE)},
                        'status uses the high-contrast face background'))
                    assert n.window_text(app.search) == text
                    screenshot(app.frame, settings[2] / 'actual-high-contrast.png')
                finally:
                    win32gui.ShowWindow(sheet, win32con.SW_SHOWNOACTIVATE)
                record('high_contrast_controls', dict(observed,
                        system_window=system_color(win32con.COLOR_WINDOW),
                        system_face=system_color(win32con.COLOR_BTNFACE)))
                screenshot(sheet, settings[2] / 'high-contrast-native-controls.png')
            restored = {}
            idle_pointer()
            remember_control_regions()
            for name, hwnd in [('combo', combo), ('tab', tab), ('overflow', overflow)]:
                restored[name] = wait_fill(hwnd, {before_controls[name]},
                                           name + ' returns to its original dark background')
            assert n.send(combo, win32con.CB_GETCURSEL) == selection
            assert n.send(tab, 0x130B) == selected
            assert profile.preferences() == expected
            record('restored_controls', dict(restored, high_contrast=list(high_contrast())))
            screenshot(sheet, settings[2] / 'high-contrast-controls-restored-dark.png')
        except AssertionError as error:
            if 'failure' not in evidence:
                record('failure', {'error': str(error)})
            raise
        else:
            # Failure teardown belongs to the launch fixture's native Session
            # cleanup, not another UIA operation in the broken dialog state.
            app.close_settings(sheet, accept=False)
        assert restored_high_contrast(high_contrast(), snapshot)
        remember_edit_region()
        remember_region(app.status)
        wait_fill(app.search, {original_edit},
                  'dark palette returns after real high contrast is disabled')
        wait_fill(app.status, {dark_status}, 'status returns to its original dark background')
        assert n.window_text(app.search) == text
        assert_dark_preference(app, profile, expected)
        record('verified', {'high_contrast': list(high_contrast()),
                            'edit': fill(app.search),
                            'status': fill(app.status),
                            'preferences': list(profile.preferences())})
        screenshot(app.frame, settings[2] / 'high-contrast-restored-dark.png')

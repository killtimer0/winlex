"""Saved placement recovery with externally guarded display changes after exit."""
import json
from pathlib import Path

import pytest
import win32api
import win32con
import win32gui

from driver import auto, until
from test_dpi import capture, click, contained, mixed_monitors, move
from test_input_appearance import focused


@pytest.fixture
def display_environment(request):
    directory = request.config.getoption('--display-change-directory')
    if not directory:
        pytest.skip('Requires an independent display-change and restoration guardian')
    directory = Path(directory)
    ready = json.loads((directory / 'ready.json').read_text(encoding='utf-8'))
    assert ready['dpi'] == 192

    def change(command, response):
        (directory / command).touch()
        def completed():
            error = directory / 'guardian-error.json'
            if error.exists():
                pytest.fail(error.read_text(encoding='utf-8'))
            path = directory / response
            if path.exists():
                result = json.loads(path.read_text(encoding='utf-8'))
                assert result['verified'], result
                return result
            return None
        return until('guarded display change ' + command, completed)

    try:
        yield ready, lambda: change('relocate.requested', 'relocated.json')
    finally:
        change('position-reset.requested', 'position-reset.json')


def intersects(a, b):
    return max(a[0], b[0]) < min(a[2], b[2]) and max(a[1], b[1]) < min(a[3], b[3])


def visible_frame(app):
    bounds = win32gui.GetWindowRect(app.frame)
    # Window borders/shadows may extend beyond the work area. The complete
    # client area and caption's central drag point must remain reachable.
    client = win32gui.GetClientRect(app.frame)
    left, top = win32gui.ClientToScreen(app.frame, (client[0], client[1]))
    right, bottom = win32gui.ClientToScreen(app.frame, (client[2], client[3]))
    works = [win32api.GetMonitorInfo(h)['Work'] for h, _, _ in win32api.EnumDisplayMonitors()]
    assert any(w[0] <= left < right <= w[2] and w[1] <= top < bottom <= w[3] for w in works)
    caption = ((bounds[0] + bounds[2]) // 2, (bounds[1] + top) // 2)
    assert any(w[0] <= caption[0] < w[2] and w[1] <= caption[1] < w[3] for w in works)
    return bounds


@pytest.mark.real_input
@pytest.mark.timeout(90)
def test_saved_192_dpi_window_recovers_when_its_old_monitor_region_disappears(
        app, mixed_monitors, display_environment):
    get_dpi, monitors = mixed_monitors
    ready, relocate = display_environment
    source = next(row for row in monitors if row['device'] == ready['device'])
    assert source['dpi'] == 192
    move(app, app.frame, source, get_dpi)
    left, top, right, bottom = source['work']
    # Save a genuinely user-positioned, non-default window entirely on the
    # secondary display. Nothing writes the private placement configuration.
    win32gui.ShowWindow(app.frame, win32con.SW_RESTORE)
    width, height = min(1200, right - left - 96), min(850, bottom - top - 96)
    win32gui.SetWindowPos(app.frame, 0, right - width - 48, bottom - height - 48,
                          width, height, win32con.SWP_NOZORDER)
    saved = visible_frame(app)
    app.open_symbol('CreateFileW')
    capture(app, 'saved-192-dpi')

    # First prove that this position was really persisted; a default primary
    # window must not masquerade as successful recovery after the next restart.
    app.restart()
    unchanged = visible_frame(app)
    assert get_dpi(app.frame) == 192
    assert all(abs(a - b) <= 2 for a, b in zip(saved, unchanged)), (saved, unchanged)
    app.open_symbol('CreateFileW')

    # The callback runs only after fixture-managed process exit and handle
    # closure, before the next contained process is launched with the same config.
    app.restart(before_launch=relocate)
    monitors_after = [win32api.GetMonitorInfo(h) for h, _, _ in win32api.EnumDisplayMonitors()]
    assert all(not intersects(saved, info['Monitor']) for info in monitors_after), (
        'Saved region must actually have disappeared', saved, monitors_after)
    restored = visible_frame(app)
    assert restored != saved
    document = app.open_symbol('CreateFileW')
    app.find('lpFileName')
    assert auto.AU3_WinActivateByHandle(app.frame)
    assert auto.AU3_ControlFocusByHandle(app.frame, app.child(1220, document.window()))
    until('recovered find is focusable', lambda: focused(app) == app.child(1220, document.window()))
    contained(app.child(1220, document.window()), document.window())
    sheet = app.settings()
    contained(app.child(win32con.IDCANCEL, sheet.handle), sheet.handle)
    assert auto.AU3_WinActivateByHandle(sheet.handle)
    until('recovered settings foreground', lambda: win32gui.GetForegroundWindow() == sheet.handle)
    click(app.child(win32con.IDCANCEL, sheet.handle))
    until('recovered dialog cancel works', lambda: not win32gui.IsWindow(sheet.handle))
    capture(app, 'recovered-after-display-change')
    (app.output / 'placement-recovery.json').write_text(json.dumps(dict(
        saved=saved, unchanged_restart=unchanged, recovered=restored,
        recovered_dpi=get_dpi(app.frame), monitors_after=monitors_after)), encoding='utf-8')

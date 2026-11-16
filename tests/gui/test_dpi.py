"""Actual mixed-monitor DPI transitions; never synthesize WM_DPICHANGED."""
import ctypes as c
from ctypes import wintypes as w
import json
import struct

import pytest
import win32api
import win32con
import win32gui
import win32process

from driver import auto, until
from test_generics import form, open_definition, submit
from test_input_appearance import focused


def font_height(hwnd):
    font = win32gui.SendMessage(hwnd, win32con.WM_GETFONT, 0, 0)
    buffer = c.create_string_buffer(92)  # LOGFONTW
    get_object = c.windll.gdi32.GetObjectW
    get_object.argtypes, get_object.restype = [w.HANDLE, c.c_int, c.c_void_p], c.c_int
    assert get_object(font, len(buffer), buffer) == len(buffer)
    return abs(struct.unpack_from('<i', buffer)[0])


def text_zoom(app, hwnd):
    # RichEdit uses character formatting and zoom, not WM_GETFONT. EM_GETZOOM
    # is above WM_USER, so its output pointers must belong to the target process.
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ,
                                   False, app.pid)
    address = None
    try:
        address = win32process.VirtualAllocEx(process, 0, 8, win32con.MEM_COMMIT, win32con.PAGE_READWRITE)
        assert win32gui.SendMessage(hwnd, win32con.WM_USER + 224, address, address + 4)
        values = (c.c_int * 2)()
        read = c.windll.kernel32.ReadProcessMemory
        read.argtypes = [w.HANDLE, c.c_void_p, c.c_void_p, c.c_size_t, c.POINTER(c.c_size_t)]
        read.restype = w.BOOL
        copied = c.c_size_t()
        assert read(int(process), address, values, c.sizeof(values), c.byref(copied))
        assert copied.value == c.sizeof(values)
        numerator, denominator = values
        assert (numerator == denominator == 0) or (numerator > 0 and denominator > 0)
        return numerator / denominator if denominator else 1
    finally:
        if address is not None:
            win32process.VirtualFreeEx(process, address, 0, win32con.MEM_RELEASE)
        process.Close()


@pytest.fixture
def mixed_monitors(app, request):
    user = c.windll.user32
    if not hasattr(user, 'GetDpiForWindow') or not hasattr(user, 'SetThreadDpiAwarenessContext'):
        pytest.skip('Mixed-monitor transitions require Windows 10 DPI APIs')
    get_dpi = user.GetDpiForWindow
    get_dpi.argtypes, get_dpi.restype = [w.HWND], w.UINT
    set_context = user.SetThreadDpiAwarenessContext
    set_context.argtypes, set_context.restype = [w.HANDLE], w.HANDLE
    previous = set_context(c.c_void_p(-4))  # Controller coordinates are physical pixels.
    assert previous, 'Cannot make the controller thread per-monitor aware'
    rows = []
    try:
        get_context = user.GetWindowDpiAwarenessContext
        get_context.argtypes, get_context.restype = [w.HWND], w.HANDLE
        equal = user.AreDpiAwarenessContextsEqual
        equal.argtypes, equal.restype = [w.HANDLE, w.HANDLE], w.BOOL
        mode = request.config.getoption('--dpi-mode') or 'auto'
        expected = -3 if mode == 'pmv1' else -4
        if mode not in ('auto', 'pmv1', 'pmv2'):
            pytest.skip('Requested DPI mode does not support per-monitor transitions')
        assert equal(get_context(app.frame), c.c_void_p(expected)), 'Unexpected application DPI awareness'
        for monitor, _, _ in win32api.EnumDisplayMonitors():
            info = win32api.GetMonitorInfo(monitor)
            left, top, _, _ = info['Work']
            win32gui.SetWindowPos(app.frame, 0, left + 24, top + 24, 0, 0,
                                  win32con.SWP_NOSIZE | win32con.SWP_NOZORDER)
            until('frame on monitor', lambda: win32api.MonitorFromWindow(app.frame) == monitor)
            rows.append(dict(handle=int(monitor), device=info['Device'],
                             work=info['Work'], dpi=get_dpi(app.frame)))
        distinct = {row['dpi']: row for row in rows}
        if len(distinct) < 2:
            pytest.skip('Requires two active monitors with different effective DPI')
        # Start with a logical window size that fits every destination. DPI
        # scaling itself must not be confused with dragging an oversized frame
        # onto a physically smaller desktop.
        left, top, right, bottom = win32gui.GetWindowRect(app.frame)
        current_dpi = get_dpi(app.frame)
        width = min(right - left, min((row['work'][2] - row['work'][0] - 64)
                    * current_dpi // row['dpi'] for row in rows))
        height = min(bottom - top, min((row['work'][3] - row['work'][1] - 64)
                     * current_dpi // row['dpi'] for row in rows))
        win32gui.SetWindowPos(app.frame, 0, left, top, width, height, win32con.SWP_NOZORDER)
        yield get_dpi, [distinct[max(distinct)], distinct[min(distinct)], distinct[max(distinct)]]
    finally:
        (app.output / 'dpi-environment.json').write_text(json.dumps(dict(
            mode=request.config.getoption('--dpi-mode') or 'auto', monitors=rows)), encoding='utf-8')
        assert set_context(previous), 'Cannot restore controller thread awareness'


def move(app, hwnd, monitor, get_dpi):
    left, top, _, _ = monitor['work']
    app.record('move window to {} at {} DPI'.format(monitor['device'], monitor['dpi']))
    win32gui.SetWindowPos(hwnd, 0, left + 24, top + 24, 0, 0,
                          win32con.SWP_NOSIZE | win32con.SWP_NOZORDER)
    until('effective window DPI {}'.format(monitor['dpi']), lambda: get_dpi(hwnd) == monitor['dpi'])


def scaled(control, initial_font, initial_dpi, dpi):
    # Font pixels are observable UI output. Allow integral font rounding, but
    # a stale source-DPI font cannot satisfy a 96/144 transition.
    until('font scaled to window DPI', lambda: abs(
        font_height(control) - initial_font * dpi / initial_dpi) <= 1)


def contained(control, parent):
    left, top, right, bottom = win32gui.GetWindowRect(control)
    bounds = win32gui.GetClientRect(parent)
    pleft, ptop = win32gui.ClientToScreen(parent, (bounds[0], bounds[1]))
    pright, pbottom = win32gui.ClientToScreen(parent, (bounds[2], bounds[3]))
    assert right > left and bottom > top
    assert pleft <= left < right <= pright and ptop <= top < bottom <= pbottom


def capture(app, name, window=None):
    destination = app.output / name
    previous, unchanged = None, 0
    def settled():
        nonlocal previous, unchanged
        image = destination / 'screen.bmp'
        error = destination / 'screen-error.txt'
        image.unlink(missing_ok=True)
        error.unlink(missing_ok=True)
        app.evidence(destination, window=window)
        assert image.is_file() and not error.exists(), 'Fresh DPI screenshot missing or failed'
        pixels = image.read_bytes()
        unchanged = unchanged + 1 if pixels == previous else 0
        previous = pixels
        return unchanged >= 2
    # Observe actual composition settling, rather than capturing a move/menu
    # animation or imposing a fixed delay on every DPI transition.
    until('DPI screenshot settled', settled)


def click(hwnd):
    left, top, right, bottom = win32gui.GetWindowRect(hwnd)
    assert auto.AU3_MouseClick('left', (left + right) // 2, (top + bottom) // 2, 1, 0)


@pytest.mark.real_input
def test_mixed_dpi_reading_and_link_round_trip(app, mixed_monitors):
    get_dpi, monitors = mixed_monitors
    move(app, app.frame, monitors[0], get_dpi)
    document = app.open_symbol('IStream')
    app.find('Clone')
    edit = app.child(1220, document.window())
    assert auto.AU3_WinActivateByHandle(app.frame)
    assert auto.AU3_ControlFocusByHandle(app.frame, edit)
    until('reading focus ready', lambda: focused(app) == edit)
    baseline_font, baseline_dpi = font_height(edit), get_dpi(app.frame)
    baseline_zoom = text_zoom(app, document.handle)
    text = document.text_block()
    for index, monitor in enumerate(monitors):
        document.scroll_lines(-100000)
        document.scroll_lines(10)
        until('reading viewport below the top', lambda: document.first_visible_line() > 0)
        before = document.selection_indices(), document.first_visible_line(), document.architecture()
        move(app, app.frame, monitor, get_dpi)
        scaled(edit, baseline_font, baseline_dpi, monitor['dpi'])
        until('declaration scaled to window DPI', lambda: abs(
            text_zoom(app, document.handle) - baseline_zoom * monitor['dpi'] / baseline_dpi) < .01)
        assert focused(app) == edit, 'DPI transition changed the focused reading control'
        assert get_dpi(document.handle) == monitor['dpi']
        assert document.text_block() == text
        assert (document.selection_indices(), document.first_visible_line(), document.architecture()) == before
        contained(edit, document.window())
        contained(app.child(1061, document.window()), document.window())
        app.find('ISequentialStream')
        document.context_action(2158)  # A real popup hit at the new physical DPI.
        until('base interface opened', lambda: 'interface ISequentialStream :' in app.editor().text_block())
        assert get_dpi(app.editor().handle) == monitor['dpi']
        app.command(1055)
        until('origin restored', lambda: app.editor().window() == document.window())
        app.find('Clone')
        capture(app, 'reading-{}-{}'.format(index, monitor['dpi']))


@pytest.mark.real_input
@pytest.mark.parametrize('kind', ('settings', 'generic'))
def test_mixed_dpi_dialog_retains_pending_input_and_hit_targets(app, mixed_monitors, ui_text, kind):
    get_dpi, monitors = mixed_monitors
    move(app, app.frame, monitors[0], get_dpi)
    if kind == 'settings':
        sheet = app.settings()
        sheet.select(1300, ui_text(2084))
        dialog, edit, expected = sheet.handle, sheet.control(1300), ui_text(2084)
        diagnostic = None
    else:
        open_definition(app)
        dialog = form(app)
        submit(app, dialog, 'int32<')
        diagnostic = until('invalid type argument reported', lambda: app.text(app.child(1232, dialog)))
        edit, expected = app.child(1500, dialog), 'int32<'
    baseline_font, baseline_dpi = font_height(edit), get_dpi(dialog)
    for index, monitor in enumerate(monitors):
        move(app, dialog, monitor, get_dpi)
        scaled(edit, baseline_font, baseline_dpi, monitor['dpi'])
        assert app.text(edit) == expected
        contained(edit, dialog)
        contained(app.child(win32con.IDCANCEL, dialog), dialog)
        if diagnostic:
            assert app.text(app.child(1232, dialog)) == diagnostic
        if kind == 'settings':
            assert auto.AU3_WinActivateByHandle(dialog)
            until('settings foreground', lambda: win32gui.GetForegroundWindow() == dialog)
            click(edit)
            popup = until('language dropdown', lambda: next((h for h in app.windows('ComboLBox')
                          if win32gui.IsWindowVisible(h)), None))
            left, top, right, bottom = win32gui.GetWindowRect(popup)
            mleft, mtop, mright, mbottom = monitor['work']
            assert mleft <= left < right <= mright and mtop <= top < bottom <= mbottom
            auto.AU3_Send('{ESC}', 0)
            until('dropdown dismissed', lambda: not win32gui.IsWindowVisible(popup))
            assert app.text(edit) == expected
        capture(app, '{}-{}-{}'.format(kind, index, monitor['dpi']), window=dialog)
    assert auto.AU3_WinActivateByHandle(dialog)
    until('dialog foreground', lambda: win32gui.GetForegroundWindow() == dialog)
    click(app.child(win32con.IDCANCEL, dialog))
    until('cancel target hit after DPI round trip', lambda: not win32gui.IsWindow(dialog))
    if kind == 'settings':
        sheet = app.settings()
        assert sheet.value(1300) == ui_text(2083)
        sheet.cancel()
    else:
        assert 'IBox<T>' in app.editor().text_block()

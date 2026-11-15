"""Physical monitor capability and user-visible destination layout readiness."""
import ctypes as c
import json
import time
import pytest
import win32con
import uiautomation as auto
from support import deadline, native as n


def monitor_dpis(monitors):
    """Reject unavailable transitions without launching or loading a document.

    Probe window DPI in a temporary per-monitor thread context: monitor DPI
    queries from an unaware caller can otherwise report a uniform virtual DPI.
    This is only a capability check; the product must still move and scale.
    """
    if len(monitors) < 2:
        pytest.skip('no differing monitor DPI: fewer than two monitors')
    if not all(hasattr(n.user, name) for name in (
            'GetDpiForWindow', 'SetThreadDpiAwarenessContext',
            'GetWindowDpiAwarenessContext', 'GetAwarenessFromDpiAwarenessContext')):
        pytest.skip('native monitor DPI reference/awareness APIs unavailable')
    create = n.api(n.user, 'CreateWindowExW', n.w.HWND,
                   n.w.DWORD, n.w.LPCWSTR, n.w.LPCWSTR, n.w.DWORD,
                   c.c_int, c.c_int, c.c_int, c.c_int,
                   n.w.HWND, n.w.HMENU, n.w.HINSTANCE, c.c_void_p)
    destroy = n.api(n.user, 'DestroyWindow', n.w.BOOL, n.w.HWND)
    values = []
    with n.physical_dpi_context():
        for left, top, _right, _bottom in monitors:
            hwnd = create(0, 'STATIC', '', win32con.WS_POPUP,
                          left + 1, top + 1, 1, 1, None, None, None, None)
            assert hwnd, 'Cannot create native monitor DPI probe'
            try:
                value = n.window_dpi(hwnd)
                assert value, 'Cannot read native monitor DPI'
                values.append(value)
            finally:
                assert destroy(hwnd), 'Cannot destroy native monitor DPI probe'
    if len(set(values)) < 2:
        pytest.skip('no differing monitor DPI: native per-monitor window DPI %r' % values)
    return dict(zip(monitors, values))


def move_frame(frame, monitor, expected_dpi, records, artifact, layout_ready=lambda dpi: True):
    """Observe destination state, not exact UIA placement or a queue fence."""
    get_dpi = n.window_dpi
    from_point = n.api(n.user, 'MonitorFromPoint', n.w.HANDLE, n.w.POINT, n.w.DWORD)
    from_window = n.api(n.user, 'MonitorFromWindow', n.w.HANDLE, n.w.HWND, n.w.DWORD)
    context = n.api(n.user, 'GetWindowDpiAwarenessContext', c.c_void_p, n.w.HWND)
    awareness = n.api(n.user, 'GetAwarenessFromDpiAwarenessContext', c.c_int, c.c_void_p)
    row = {'requested': [monitor[0] + 20, monitor[1] + 20],
           'bounds': monitor, 'expected_dpi': expected_dpi}
    records.append(row)
    started = time.monotonic()

    def sample():
        rect = n.w.RECT()
        assert n.get_rect(frame, c.byref(rect)), 'GetWindowRect failed: %s' % c.get_last_error()
        hosting, dpi = from_window(frame, 0), get_dpi(frame)
        assert hosting and dpi, 'Cannot read frame monitor/DPI'
        return {'rect': [rect.left, rect.top, rect.right, rect.bottom],
                'monitor': hosting, 'dpi': dpi}

    def located(state):
        left, top, right, bottom = state['rect']
        return (state['monitor'] == row['target'] and left < right and top < bottom
                and monitor[0] <= left < monitor[2] and monitor[1] <= top < monitor[3]
                and state['dpi'] == expected_dpi)

    try:
        with deadline.operation('move frame and adopt destination layout'), n.physical_dpi_context():
            until = started + 5
            row['target'] = from_point(n.w.POINT(*row['requested']), 0)
            assert row['target'], 'Requested point has no native monitor'
            value = context(frame)
            assert value, 'Cannot read frame DPI awareness context'
            row['awareness'] = awareness(value)
            assert row['awareness'] in (0, 1, 2), 'Invalid frame DPI awareness'
            row['before'] = sample()
            row['actual'] = row['before']
            row['already_located_before'] = located(row['before'])
            artifact.write_text(json.dumps(records, indent=2), encoding='utf-8')
            moved = auto.ControlFromHandle(frame).GetTransformPattern().Move(*row['requested'], waitTime=0)
            row['actual'] = sample()
            assert moved, 'UIA Move returned false'

            def ready():
                row['actual'] = sample()
                row['layout_ready'] = located(row['actual']) and bool(layout_ready(expected_dpi))
                return row['layout_ready']

            remaining = until - time.monotonic()
            assert remaining > 0, 'Move exhausted shared five-second budget'
            n.wait(ready, 'frame destination monitor/origin/DPI/layout', remaining)
        return expected_dpi
    except Exception as error:
        row['error'] = repr(error)
        raise
    finally:
        row['elapsed_seconds'] = time.monotonic() - started
        artifact.write_text(json.dumps(records, indent=2), encoding='utf-8')

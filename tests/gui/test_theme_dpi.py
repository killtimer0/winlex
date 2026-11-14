"""Rendered RichEdit text scales without rewriting its stored formatting."""
import ctypes as c
import json
import time
import pytest
import win32con
import winreg
import uiautomation as auto

from support import deadline, native as n
from test_theme import modern_theme


TOKENS = ('CreateFileW', 'const', 'SECURITY_ATTRIBUTES')


def token_ranges(editor):
    document = n.rich_edit_document(editor)
    text = n.rich_edit_text(editor)
    for token in TOKENS:
        start = len(text[:text.index(token)].encode('utf-16-le')) // 2
        yield token, document.Range(start, start + len(token))


@deadline.operation('read declaration token font styles via TOM')
def font_styles(editor):
    return {token: {'name': span.Font.Name, 'points': span.Font.Size,
                    'bold': span.Font.Bold, 'italic': span.Font.Italic,
                    'underline': span.Font.Underline}
            for token, span in token_ranges(editor)}


@deadline.operation('measure rendered declaration tokens via TOM')
def rendered_metrics(editor):
    """Measure physical text advances and line boxes, including RichEdit zoom.

    UIA FontSize and TOM Font.Size describe stored points, not rendered zoom.
    TOM GetPoint returns the laid-out text coordinates without moving selection.
    """
    metrics = {}
    with n.window_dpi_context(editor):
        for token, span in token_ranges(editor):
            # tomStart | tomClientCoord | tomAllowOffClient, with tomBottom
            # for the second point. AllowOffClient also covers short desktops.
            left, top = span.GetPoint(32 | 256 | 512)
            _, bottom = span.GetPoint(32 | 256 | 512 | 8)
            last = n.rich_edit_document(editor).Range(span.End - 1, span.End)
            right, last_top = last.GetPoint(32 | 256 | 512)
            assert last_top == top, ('measurement token wrapped', token)
            assert right > left and bottom > top, ('empty rendered text box', token)
            metrics[token] = {'advance': right - left, 'height': bottom - top,
                              'glyphs': len(token) - 1}
    return metrics


def assert_scaled(actual, baseline, ratio, *, readiness=False):
    for token, original in baseline.items():
        current = actual[token]
        # GDI hints glyph advances independently. Vertical font metrics round
        # ascent/descent and internal leading separately at each size, too.
        tolerances = {'advance': original['glyphs'] * (1 + ratio) / 2 + 2,
                      'height': 2 * (1 + ratio)}
        for metric, tolerance in tolerances.items():
            expected = original[metric] * ratio
            if readiness and abs(current[metric] - expected) > tolerance:
                return False
            assert abs(current[metric] - expected) <= tolerance, (
                'rendered font did not scale once with effective monitor DPI',
                token, metric, current[metric], expected, tolerance, ratio)
    return True


@deadline.operation('read declaration token colors via UIA')
def text_colors(editor):
    # UIA range queries do not disturb the user's selection, unlike querying
    # EM_GETCHARFORMAT with a temporarily selected token.
    document = auto.ControlFromHandle(editor).GetTextPattern().DocumentRange
    colors = {}
    for token in TOKENS:
        span = document.FindText(token, False, False)
        assert span is not None, 'missing declaration token: ' + token
        color = span.GetAttributeValue(auto.TextAttributeId.ForegroundColorAttribute)
        assert isinstance(color, int), ('missing or mixed foreground color', token, color)
        colors[token] = color
    return colors


def simulated_frame_dpi_update(app, dpi):
    """Send a simulated notification, not a real change of monitor DPI.

    The real frame handler reads the monitor's effective DPI itself. Keeping the
    suggested bounds unchanged exercises its font/layout update path even on a
    single-DPI worker, without claiming that Windows changed the effective DPI.
    """
    with n.window_dpi_context(app.frame):
        bounds = n.w.RECT(*n.rectangle(app.frame))
    # WM_DPICHANGED is a system message: Windows marshals its RECT from the
    # sender's address space, unlike RichEdit's private EM_* messages.
    n.send(app.frame, 0x02E0, dpi | (dpi << 16), c.addressof(bounds))


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
    get_dpi = n.api(n.user, 'GetDpiForWindow', n.w.UINT, n.w.HWND)
    values = []
    with n.physical_dpi_context():
        for left, top, _right, _bottom in monitors:
            hwnd = create(0, 'STATIC', '', win32con.WS_POPUP,
                          left + 1, top + 1, 1, 1, None, None, None, None)
            assert hwnd, 'Cannot create native monitor DPI probe'
            try:
                value = get_dpi(hwnd)
                assert value, 'Cannot read native monitor DPI'
                values.append(value)
            finally:
                assert destroy(hwnd), 'Cannot destroy native monitor DPI probe'
    if len(set(values)) < 2:
        pytest.skip('no differing monitor DPI: native per-monitor window DPI %r' % values)
    return dict(zip(monitors, values))


def move_frame(frame, monitor, expected_dpi, records, artifact, layout_ready=lambda dpi: True):
    """Observe destination state, not exact UIA placement or a queue fence."""
    get_dpi = n.api(n.user, 'GetDpiForWindow', n.w.UINT, n.w.HWND)
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


@modern_theme
@pytest.mark.parametrize('dpi', ['pmv1', 'pmv2'])
@pytest.mark.parametrize('theme', ['light', 'dark'])
@pytest.mark.parametrize('transition', ['monitor', 'monitor-secondary', 'simulated'])
def test_rich_edit_colors_and_selection_after_frame_dpi_update(
        launch, profile, settings, dpi, theme, transition):
    with n.physical_dpi_context():
        monitors = n.monitors()
    if transition != 'simulated':
        references = monitor_dpis(monitors)
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, theme)
    app = launch([settings[1]], dpi=dpi)
    get_dpi = n.api(n.user, 'GetDpiForWindow', n.w.UINT, n.w.HWND)
    moves = []
    artifact = settings[2] / ('rich-edit-moves-%s-%s-%s.json' % (theme, dpi, transition))
    if transition != 'simulated':
        context = n.api(n.user, 'GetWindowDpiAwarenessContext', c.c_void_p, n.w.HWND)(app.frame)
        assert context, 'Cannot read frame DPI awareness context'
        assert n.api(n.user, 'GetAwarenessFromDpiAwarenessContext', c.c_int, c.c_void_p)(context) == 2

    if transition == 'monitor-secondary':
        # Open the RichEdit only after moving: initialization on a monitor whose
        # DPI differs from the system DPI must not bake that DPI into its font.
        move_frame(app.frame, monitors[1], references[monitors[1]], moves, artifact)
    elif transition == 'monitor':
        move_frame(app.frame, monitors[0], references[monitors[0]], moves, artifact)
    app.query('Windows.Win32.Storage.FileSystem.CreateFileW')
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), 'loaded declaration', 45)
    editor = n.get_item(doc, 1203)
    original = n.rich_edit_text(editor)
    colors = text_colors(editor)
    # Establish that the fixture actually contains syntax and hyperlink colors;
    # preserving an already monochrome document must not satisfy the regression.
    assert len(set(colors.values())) == 3, ('expected distinct text, keyword and link colors', colors)
    assert colors['const'] != 0 and colors['SECURITY_ATTRIBUTES'] != 0
    styles = font_styles(editor)
    start = len(original[:original.index('CreateFileW')].encode('utf-16-le')) // 2
    selected = (start + 2, start + 9)
    n.send(editor, win32con.EM_SETSEL, *selected)

    @deadline.operation('read declaration selection via TOM')
    def selection():
        current = n.rich_edit_document(editor).Selection
        return current.Start, current.End

    assert selection() == selected
    effective_dpi = get_dpi(app.frame)
    assert effective_dpi > 0
    initial_metrics = rendered_metrics(editor)

    def assert_preserved():
        assert selection() == selected, 'frame DPI update changed the user selection'
        assert n.rich_edit_text(editor) == original
        assert app.active_document() == doc
        assert text_colors(editor) == colors, 'frame DPI update changed syntax or hyperlink colors'
        assert font_styles(editor) == styles, 'frame DPI update rewrote stored font styles'
        assert selection() == selected, 'formatting observation must not change the user selection'

    observations = [{'effective_dpi': effective_dpi, 'metrics': initial_metrics,
                     'styles': styles}]
    metrics_by_dpi = {effective_dpi: initial_metrics}

    def observe(expected=None, **details):
        observed = get_dpi(app.frame) if expected is None else expected
        if expected is None:
            n.wait(lambda: get_dpi(editor) == observed, 'RichEdit adopts frame monitor DPI')
        metrics = rendered_metrics(editor)
        observations.append(dict(details, effective_dpi=observed, metrics=metrics,
                                 styles=font_styles(editor)))
        assert_preserved()
        assert_scaled(metrics, initial_metrics, observed / effective_dpi)
        if observed in metrics_by_dpi:
            # Identical effective DPI must restore identical layout, even after
            # repeated moves; this also detects accumulating/double zoom.
            assert metrics == metrics_by_dpi[observed], 'rendered metrics drifted after a DPI round trip'
        else:
            metrics_by_dpi[observed] = metrics

    try:
        if transition == 'simulated':
            for target in (144 if effective_dpi != 144 else 192, effective_dpi):
                simulated_frame_dpi_update(app, target)
                assert get_dpi(app.frame) == effective_dpi, 'simulated notification changed the actual monitor DPI'
                observe(simulated_notification_dpi=target)
        else:
            # Repeat complete round trips to distinguish scaling from zoom
            # accumulated on each notification. Restore the starting monitor.
            start_monitor = monitors[1] if transition == 'monitor-secondary' else monitors[0]

            def layout_ready(expected):
                child_dpi = get_dpi(editor)
                assert child_dpi, 'Cannot read RichEdit DPI'
                if child_dpi != expected:
                    return False
                metrics = rendered_metrics(editor)
                return (assert_scaled(metrics, initial_metrics, expected / effective_dpi, readiness=True)
                        and (expected not in metrics_by_dpi or metrics == metrics_by_dpi[expected]))

            for monitor in (monitors + monitors[::-1] + [start_monitor]) * 2:
                expected = move_frame(app.frame, monitor, references[monitor], moves, artifact, layout_ready)
                observe(expected, monitor=monitor)
            assert len({item['effective_dpi'] for item in observations}) >= 2, observations
    finally:
        # Keep measured DPI even when a formatting assertion fails on the old binary.
        (settings[2] / ('rich-edit-dpi-%s-%s-%s.json' % (theme, dpi, transition))).write_text(
            json.dumps(observations, indent=2), encoding='utf-8')

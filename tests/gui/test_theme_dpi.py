"""Rendered RichEdit text scales without rewriting its stored formatting."""
import ctypes as c
import json
import pytest
import win32con
import winreg
import uiautomation as auto

from support import native as n
from test_theme import modern_theme


TOKENS = ('CreateFileW', 'const', 'SECURITY_ATTRIBUTES')


def token_ranges(editor):
    document = n.rich_edit_document(editor)
    text = n.rich_edit_text(editor)
    for token in TOKENS:
        start = len(text[:text.index(token)].encode('utf-16-le')) // 2
        yield token, document.Range(start, start + len(token))


def font_styles(editor):
    return {token: {'name': span.Font.Name, 'points': span.Font.Size,
                    'bold': span.Font.Bold, 'italic': span.Font.Italic,
                    'underline': span.Font.Underline}
            for token, span in token_ranges(editor)}


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


def assert_scaled(actual, baseline, ratio):
    for token, original in baseline.items():
        current = actual[token]
        # GDI hints glyph advances independently. Vertical font metrics round
        # ascent/descent and internal leading separately at each size, too.
        tolerances = {'advance': original['glyphs'] * (1 + ratio) / 2 + 2,
                      'height': 2 * (1 + ratio)}
        for metric, tolerance in tolerances.items():
            expected = original[metric] * ratio
            assert abs(current[metric] - expected) <= tolerance, (
                'rendered font did not scale once with effective monitor DPI',
                token, metric, current[metric], expected, tolerance, ratio)


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


@modern_theme
@pytest.mark.parametrize('dpi', ['pmv1', 'pmv2'])
@pytest.mark.parametrize('theme', ['light', 'dark'])
@pytest.mark.parametrize('transition', ['monitor', 'monitor-secondary', 'simulated'])
def test_rich_edit_colors_and_selection_after_frame_dpi_update(
        launch, profile, settings, dpi, theme, transition):
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, theme)
    app = launch([settings[1]], dpi=dpi)
    get_dpi = n.api(n.user, 'GetDpiForWindow', n.w.UINT, n.w.HWND)
    monitors = n.monitors()
    transform = auto.ControlFromHandle(app.frame).GetTransformPattern()

    def move(monitor):
        with n.window_dpi_context(app.frame):
            assert transform.Move(monitor[0] + 20, monitor[1] + 20)
        return n.wait(lambda: get_dpi(app.frame), 'frame monitor DPI')

    if transition == 'monitor-secondary':
        if len(monitors) < 2:
            pytest.skip('no secondary monitor for document initialization')
        # Open the RichEdit only after moving: initialization on a monitor whose
        # DPI differs from the system DPI must not bake that DPI into its font.
        move(monitors[1])
    elif transition == 'monitor':
        move(monitors[0])
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

    def observe(**details):
        observed = get_dpi(app.frame)
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
            for monitor in (monitors + monitors[::-1] + [start_monitor]) * 2:
                move(monitor)
                observe(monitor=monitor)
            if len({item['effective_dpi'] for item in observations}) < 2:
                pytest.skip('formatting checked; no differing effective monitor DPI observed: %r' % observations)
    finally:
        # Keep measured DPI even when a formatting assertion fails on the old binary.
        (settings[2] / ('rich-edit-dpi-%s-%s-%s.json' % (theme, dpi, transition))).write_text(
            json.dumps(observations, indent=2), encoding='utf-8')

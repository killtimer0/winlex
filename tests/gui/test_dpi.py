import ctypes as c
from ctypes import wintypes as w
import pytest
import win32con
import uiautomation as auto
from support import native as n
from test_theme_dpi import monitor_dpis, move_frame
from test_tools import visual_state


@pytest.mark.parametrize("maximized", [True, False])
def test_cross_monitor_layout(launch, settings, maximized):
    if not hasattr(n.user, "GetDpiForWindow"):
        pytest.skip("GetDpiForWindow unavailable; no actual-DPI assertion")
    get_dpi = n.api(n.user, "GetDpiForWindow", w.UINT, w.HWND)
    with n.physical_dpi_context():
        displays = n.monitors()
    references = monitor_dpis(displays)
    app = launch([settings[1]])
    context = n.api(n.user, 'GetWindowDpiAwarenessContext', c.c_void_p, w.HWND)(app.frame)
    assert context, 'Cannot read frame DPI awareness context'
    awareness = n.api(n.user, 'GetAwarenessFromDpiAwarenessContext', c.c_int, c.c_void_p)(context)
    assert awareness in (0, 1, 2), 'Invalid frame DPI awareness'
    initial_dpi = get_dpi(app.frame)
    assert initial_dpi > 0
    if awareness == 0:
        assert initial_dpi == 96
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "document before DPI change", 45)
    if not maximized:
        visual_state(doc, auto.WindowVisualState.Normal)
    assert bool(n.is_zoomed(doc)) == maximized

    def layout_ready(value):
        child_dpis = (get_dpi(app.results), get_dpi(doc))
        assert all(child_dpis), 'Cannot read results/document DPI'
        if child_dpis != (value, value):
            return False
        tabs = n.get_item(app.frame, 1104)
        if n.rectangle(app.results)[2] > n.rectangle(tabs)[0]:
            return False
        # Independently virtualized non-aware rectangles may round a shared edge
        # one physical pixel in opposite directions.
        if n.rectangle(app.mdi)[1] < n.rectangle(tabs)[3] - 1:
            return False
        if bool(n.is_zoomed(doc)) != maximized:
            return False
        with n.window_dpi_context(app.frame):
            if maximized:
                # A maximized child's client must fill the MDI client even after
                # changing DPI; compare both origins in the same coordinate space.
                origin = w.POINT()
                n.api(n.user, "MapWindowPoints", c.c_int, w.HWND, w.HWND,
                      c.POINTER(w.POINT), w.UINT)(doc, app.mdi, c.byref(origin), 1)
                if (origin.x, origin.y) != (0, 0):
                    return False
                if n.rectangle(doc, client=True) != n.rectangle(app.mdi, client=True):
                    return False
            button = n.get_item(doc, 1047)
            editor = n.get_item(doc, 1203)
            b, e = n.rectangle(button), n.rectangle(editor)
            if not (e[0] <= b[0] < b[2] <= e[2] and b[3] <= e[1]):
                return False
            assert n.send(button, win32con.WM_GETFONT) != 0
            height, face = app.declaration_font(doc)
            assert height > 0 and face == "Courier New"
            return True

    values, moves = [], []
    for monitor in displays:
        expected = references[monitor] if awareness == 2 else initial_dpi
        values.append(move_frame(app.frame, monitor, expected, moves,
                                 settings[2] / ('layout-moves-%s.json' % maximized), layout_ready))
    (settings[2] / "observed-dpi.txt").write_text(repr(values), encoding="utf-8")
    if len(set(values)) < 2:
        assert awareness != 2, ('per-monitor frame did not adopt differing reference DPI', values)
        pytest.skip("geometry checked; no differing effective DPI observed")

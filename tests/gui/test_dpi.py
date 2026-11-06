import ctypes as c
from ctypes import wintypes as w
import pytest
import win32con
import uiautomation as auto
from support import native as n
from test_tools import visual_state


@pytest.mark.parametrize("maximized", [True, False])
def test_cross_monitor_layout(launch, settings, maximized):
    if not hasattr(n.user, "GetDpiForWindow"):
        pytest.skip("GetDpiForWindow unavailable; no actual-DPI assertion")
    get_dpi = n.api(n.user, "GetDpiForWindow", w.UINT, w.HWND)
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "document before DPI change", 45)
    if not maximized:
        visual_state(doc, auto.WindowVisualState.Normal)
    assert bool(n.is_zoomed(doc)) == maximized
    displays = n.monitors()
    values = []
    for left, top, right, bottom in displays:
        assert auto.ControlFromHandle(app.frame).GetTransformPattern().Move(left + 20, top + 20)
        value = n.wait(lambda: get_dpi(app.frame), "window DPI")
        values.append(value)
        tabs = n.get_item(app.frame, 1104)
        assert n.rectangle(app.results)[2] <= n.rectangle(tabs)[0]
        # Independently virtualized non-aware rectangles may round a shared edge
        # one physical pixel in opposite directions.
        assert n.rectangle(app.mdi)[1] >= n.rectangle(tabs)[3] - 1
        assert get_dpi(app.results) == value
        assert get_dpi(doc) == value
        with n.window_dpi_context(app.frame):
            if maximized:
                # A maximized child's client must fill the MDI client even after
                # changing DPI; compare both origins in the same coordinate space.
                origin = w.POINT()
                n.api(n.user, "MapWindowPoints", c.c_int, w.HWND, w.HWND,
                      c.POINTER(w.POINT), w.UINT)(doc, app.mdi, c.byref(origin), 1)
                assert (origin.x, origin.y) == (0, 0)
                assert n.rectangle(doc, client=True) == n.rectangle(app.mdi, client=True)
            button = n.get_item(doc, 1047)
            editor = n.get_item(doc, 1203)
            b, e = n.rectangle(button), n.rectangle(editor)
            assert e[0] <= b[0] < b[2] <= e[2] and b[3] <= e[1]
            assert n.send(button, win32con.WM_GETFONT) != 0
            height, face = app.declaration_font(doc)
            assert height > 0 and face == "Courier New"
    (settings[2] / "observed-dpi.txt").write_text(repr(values), encoding="utf-8")
    if len(set(values)) < 2:
        pytest.skip("geometry checked; no differing effective DPI observed")

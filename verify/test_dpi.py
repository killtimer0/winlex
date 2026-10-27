import ctypes as c
from ctypes import wintypes as w
import pytest
from support import native as n


@pytest.mark.parametrize("maximized", [True, False])
def test_cross_monitor_layout(launch, settings, maximized):
    if not hasattr(n.user, "GetDpiForWindow"):
        pytest.skip("GetDpiForWindow unavailable; no actual-DPI assertion")
    get_dpi = n.api(n.user, "GetDpiForWindow", w.UINT, w.HWND)
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: '"parameters"' in app.content(doc), "document before DPI change", 45)
    if not maximized:
        n.send(doc, 0x112, 0xF120)  # native SC_RESTORE
    assert bool(n.is_zoomed(doc)) == maximized
    displays = n.monitors()
    values = []
    for left, top, right, bottom in displays:
        assert n.set_pos(app.frame, None, left + 20, top + 20, 0, 0, 0x15)
        n.send(app.frame, 0)
        value = get_dpi(app.frame)
        values.append(value)
        tabs = n.get_item(app.frame, 1104)
        assert n.rectangle(app.results)[2] <= n.rectangle(tabs)[0]
        # Independently virtualized non-aware rectangles may round a shared edge
        # one physical pixel in opposite directions.
        assert n.rectangle(app.mdi)[1] >= n.rectangle(tabs)[3] - 1
        assert get_dpi(app.results) == value
        assert get_dpi(doc) == value
        with n.window_dpi_context(app.frame):
            button = n.get_item(doc, 1047)
            editor = n.get_item(doc, 1203)
            b, e = n.rectangle(button), n.rectangle(editor)
            assert e[0] <= b[0] < b[2] <= e[2] and b[3] <= e[1]
            assert n.send(button, 0x31) == n.send(editor, 0x31) != 0
    (settings[2] / "observed-dpi.txt").write_text(repr(values), encoding="utf-8")
    if len(set(values)) < 2:
        pytest.skip("geometry checked; no differing effective DPI observed")

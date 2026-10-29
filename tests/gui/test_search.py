import time
import win32con
from support import messages
from support import native as n


def test_stream_and_panel_state(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("file")
    assert app.count() > 100
    rect0, rect1 = app.item_rect(0), app.item_rect(1)
    assert rect0[3] - rect0[1] >= 28
    assert rect0[3] - rect0[1] == rect1[3] - rect1[1]
    header = n.send(app.results, messages.LVM_GETHEADER)
    header_rect = n.rectangle(header)
    assert not n.is_visible(header) or header_rect[1] == header_rect[3]
    style = n.api(n.user, "GetWindowLongW", n.c.c_long, n.w.HWND, n.c.c_int)(app.results, win32con.GWL_STYLE)
    assert not style & win32con.WS_HSCROLL, "horizontal scrollbar visible"
    first, last = app.cell(0), app.cell(app.count() - 1)
    app.select(3)
    app.command(1021)
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    app.command(1021)
    assert not n.is_visible(app.sources)
    app.command(1040)
    assert n.is_visible(app.results)
    assert (app.cell(0), app.cell(app.count() - 1)) == (first, last)
    assert n.send(app.results, messages.LVM_GETNEXTITEM, -1, messages.LVNI_SELECTED) == 3
    app.query("CreateFileW")
    assert app.count() >= 1
    assert all("CreateFileW" in app.cell(i) for i in range(app.count()))
    n.set_text(app.search, "file")
    n.set_text(app.search, "no_such_winlex_symbol_123456")
    n.wait(lambda: "0" in app.status_text(), "empty replacement")
    assert app.count() == 0


def test_ime_defers_search(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    n.set_text(app.search, "CreateFileW")
    time.sleep(.35)
    assert app.count() == 0
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.count() > 0, "search after IME commit", 45)

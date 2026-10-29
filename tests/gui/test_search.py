import time
import json
import subprocess
import win32con
from support import messages
from support import native as n


def test_search_modes_match_cli_order(launch, settings):
    cli = settings[0].with_name("winlex-fuzzy.exe")
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, flag in [
        (0, "CrtFlW", None), (1, "CreateFileW", "--name"),
        (2, "0x80000000", "--value"), (2, "1.5", "--value"),
        (3, "0000000c00000000c000000000000046", "--guid"),
        (0, "0x80000000", None),
    ]:
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)
        app.query(query)
        command = [str(cli), str(settings[1]), "search", query, "--limit", "1000", "--json"]
        if flag:
            command.append(flag)
        result = json.loads(subprocess.check_output(command, encoding="utf-8"))["result"]
        expected = result["matches"]
        assert len(expected) < 1000
        # The GUI's default categories exclude ordinary fields and type references.
        expected = [hit for hit in expected if hit["kind"] not in {"field", "type_reference", "property", "event"}]
        assert [app.cell(row).splitlines()[0] for row in range(app.count())] == [hit["full_name"] for hit in expected]


def test_search_modes_labels_mapping_errors_and_ime(launch, settings):
    app = launch([settings[1]])
    app.query("CrtFlW")
    mode = n.get_item(app.frame, 1114)
    chinese = app.toolbar_text(1040) == "搜索"
    assert app.choices(mode) == (["综合", "符号名", "数字", "GUID"] if chinese
                                 else ["Combined", "Name", "Number", "GUID"])
    assert n.send(mode, win32con.CB_GETCURSEL) == 0
    assert app.named_rows("CreateFileW")

    def choose(index):
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)

    choose(1)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "exact search")
    assert app.count() == 0
    app.query("CreateFileW")
    assert app.count() > 0
    n.send(mode, win32con.CB_SETCURSEL, 3)
    app.command(1114, code=win32con.CBN_SELENDCANCEL)
    assert n.send(mode, win32con.CB_GETCURSEL) == 1
    choose(2)
    n.wait(lambda: ("数字无效" if chinese else "Invalid number") in app.status_text(), "number error")
    assert app.count() == 0
    app.query("0x80000000")
    expected = [app.cell(i) for i in range(app.count())]
    assert expected
    app.query("2147483648")
    assert [app.cell(i) for i in range(app.count())] == expected
    choose(3)
    app.query("0000000c00000000c000000000000046")
    expected = [app.cell(i) for i in range(app.count())]
    assert expected
    app.query("0,0,0,12,0,0,0,0,192,0,0,0,0,0,0,70")
    assert [app.cell(i) for i in range(app.count())] == expected
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    n.set_text(app.search, "CreateFileW")
    choose(1)
    time.sleep(.35)
    assert app.count() == 0
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.count() > 0, "mode search after IME")
    app.focus(app.search)
    n.key_message(app.search, win32con.VK_TAB)
    n.wait(lambda: app.focused() == mode, "Tab reaches mode selector")
    n.key_message(mode, win32con.VK_TAB)
    n.wait(lambda: app.focused() == app.results, "Tab reaches results")


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
    assert app.cell(0).splitlines()[0].rsplit(".", 1)[-1] == "CreateFileW"
    app.query("CrtFlW")
    assert any(app.cell(i).splitlines()[0].rsplit(".", 1)[-1] == "CreateFileW" for i in range(app.count()))
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


def test_fuzzy_incremental_merge_preserves_list_state(launch, settings):
    app = launch([settings[1]] * 2)
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    # The modal loop leaves the bounded worker queue pending until the sheet closes.
    sheet = app.settings()
    n.set_text(app.search, "e")
    time.sleep(.25)
    n.send(app.search, win32con.WM_TIMER, 1, 0)
    app.close_settings(sheet, False)
    n.wait(lambda: app.count() > 20, "first ranked batch", 45)
    sheet = app.settings()
    initial = app.count()
    assert initial > 20
    assert not app.status_text().startswith(("Results ", "共 ")), "search ended before state capture"
    original_selection = initial // 2
    app.select(original_selection)
    n.send(app.results, 0x1043, 0, original_selection)  # LVM_SETSELECTIONMARK
    selected = app.cell(original_selection)
    n.send(app.results, 0x1013, min(800, initial - 1), 0)  # LVM_ENSUREVISIBLE
    top = n.send(app.results, 0x1027)  # LVM_GETTOPINDEX
    assert top > 0
    anchor = app.cell(top)
    offset = app.item_rect(top)[1]
    started = time.monotonic()
    app.close_settings(sheet, False)
    latencies = []
    def complete():
        before = time.monotonic()
        done = app.status_text().startswith(("Results ", "共 "))
        latencies.append(time.monotonic() - before)
        return done
    n.wait(complete, "ranked search completed", 60)
    assert app.count() > initial
    selected_row = app.selected_row()
    assert app.cell(selected_row) == selected
    assert n.send(app.results, messages.LVM_GETNEXTITEM, -1, 1) == selected_row  # LVNI_FOCUSED
    assert n.send(app.results, 0x1042) == selected_row  # LVM_GETSELECTIONMARK
    top = n.send(app.results, 0x1027)
    assert app.cell(top) == anchor
    assert app.item_rect(top)[1] == offset
    assert not app.documents()
    (settings[2] / "fuzzy-incremental.json").write_text(json.dumps({
        "initial": initial, "total": app.count(), "seconds_after_resume": time.monotonic() - started,
        "original_selection": original_selection, "selected_row": selected_row, "top_row": top,
        "max_status_latency": max(latencies),
    }, indent=2), encoding="utf-8")


def test_fuzzy_query_selects_short_or_qualified_names(launch, settings):
    app = launch([settings[1]])
    app.query("WindowsWin32")
    assert app.count() == 0
    app.query("CrtFlW")
    assert app.named_rows("CreateFileW")
    short = app.count()
    app.query("Storage.FileSystem.CrtFlW")
    assert app.named_rows("CreateFileW")
    assert all("Storage.FileSystem" in app.cell(i).splitlines()[0]
               for i in app.named_rows("CreateFileW"))
    assert app.count() < short

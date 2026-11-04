import time
import json
import subprocess
import win32con
import win32gui
from support.diagnostics import capture, screenshot
from support import messages
from support import native as n


def test_result_rows_show_context_and_match_emphasis(launch, settings):
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, prefix, name_match in [
        (0, "CrtFlW", "Apis · ", True),
        (1, 'name contains "access" and value = 5', "= 5 · ", False),
        (1, 'guid = "0000000c-0000-0000-c000-000000000046"', "{0000000c-0000-0000-c000-000000000046} · ", False),
    ]:
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)
        app.query(query)
        assert app.count() > 0
        row = app.named_rows("CreateFileW")[0] if name_match else 0
        n.send(app.results, 0x1013, row, 0)  # LVM_ENSUREVISIBLE
        lines = app.cell(row).splitlines()
        detail = lines[2]
        assert detail.startswith(prefix), detail
        with n.window_dpi_context(app.frame):
            left, top, right, bottom = app.item_rect(row)
            with capture(app.results, client=True) as (dc, _, _):
                accent = win32gui.GetSysColor(win32con.COLOR_HOTLIGHT)
                assert any(win32gui.GetPixel(dc, x, y) == accent
                           for x in range(left, right)
                           for y in range(top, bottom)), (query, "missing match emphasis")
            screenshot(app.frame, settings[2] / ("result-row-%d.bmp" % index))
            screenshot(app.results, settings[2] / ("result-list-%d.bmp" % index))


def test_search_modes_match_cli_order(launch, settings):
    cli = settings[0].with_name("winlex.exe")
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, flag in [
        (0, "CrtFlW", None), (1, 'name = "CreateFileW"', "--where"),
        (1, 'name contains "access" and value = 5', "--where"),
        (1, 'value contains "Microsoft"', "--where"),
        (1, "value = 1.5", "--where"),
        (1, 'guid = "0000000c-0000-0000-c000-000000000046"', "--where"),
        (0, "0x80000000", None),
    ]:
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)
        app.query(query)
        command = [str(cli), str(settings[1]), "search"]
        if flag:
            command.append(flag)
        command += [query, "--limit", "1000", "--json"]
        result = json.loads(subprocess.check_output(command, encoding="utf-8"))["result"]
        expected = result["matches"]
        assert len(expected) < 1000
        # The GUI's default categories exclude ordinary fields and type references.
        expected = [hit for hit in expected if hit["kind"] not in {"field", "type_reference", "property", "event"}]
        assert [app.cell(row).splitlines()[0] for row in range(app.count())] == [hit["full_name"] for hit in expected]


def test_search_mode_labels_errors_and_navigation(launch, settings):
    app = launch([settings[1]])
    app.query("CrtFlW")
    mode = n.get_item(app.frame, 1114)
    chinese = app.toolbar_text(1040) == "搜索"
    assert app.choices(mode) == (["快速搜索", "条件搜索"] if chinese
                                 else ["Quick", "Conditions"])
    assert n.send(mode, win32con.CB_GETCURSEL) == 0
    assert app.named_rows("CreateFileW")

    def choose(index):
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)

    choose(1)
    n.wait(lambda: ("条件" if chinese else "condition") in app.status_text(), "condition error")
    assert app.count() == 0
    app.query('name = "CreateFileW"')
    assert app.count() > 0
    n.send(mode, win32con.CB_SETCURSEL, 0)
    app.command(1114, code=win32con.CBN_SELENDCANCEL)
    assert n.send(mode, win32con.CB_GETCURSEL) == 1
    n.set_text(app.search, "value =")
    n.wait(lambda: ("尚未输入完整" if chinese else "Incomplete") in app.status_text(), "incomplete condition")
    assert app.count() == 0
    n.set_text(app.search, "value < nan")
    n.wait(lambda: ("条件无效" if chinese else "Invalid condition") in app.status_text(), "NaN bound error")
    assert app.count() == 0
    app.focus(app.search)
    n.key_message(app.search, win32con.VK_TAB)
    n.wait(lambda: app.focused() == mode, "Tab reaches mode selector")
    n.key_message(mode, win32con.VK_TAB)
    n.wait(lambda: app.focused() == app.results, "Tab reaches results")


def test_panel_switch_preserves_result_selection_and_scroll(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("file")
    assert app.count() > 100
    first, last = app.cell(0), app.cell(app.count() - 1)
    app.select(3)
    n.send(app.results, 0x1013, app.count() - 1, 0)  # LVM_ENSUREVISIBLE
    top = n.send(app.results, 0x1027)  # LVM_GETTOPINDEX
    assert top > 0
    anchor = app.cell(top)
    offset = app.item_rect(top)[1]
    app.command(1021)
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    app.command(1021)
    assert not n.is_visible(app.sources)
    app.command(1040)
    assert n.is_visible(app.results)
    assert (app.cell(0), app.cell(app.count() - 1)) == (first, last)
    assert n.send(app.results, messages.LVM_GETNEXTITEM, -1, messages.LVNI_SELECTED) == 3
    top = n.send(app.results, 0x1027)
    assert app.cell(top) == anchor
    assert app.item_rect(top)[1] == offset


def test_panel_refresh_and_programmatic_mode_selection_do_not_submit_search(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    app.select(0)
    mode = n.get_item(app.frame, 1114)
    before = (app.count(), app.cell(0), app.status_text(), profile.preferences())
    # Changing the control alone is not the user's CBN_SELENDOK commitment.
    n.send(mode, win32con.CB_SETCURSEL, 1)
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    app.command(1021)
    app.command(1040)
    assert (app.count(), app.cell(0), app.status_text(), profile.preferences()) == before
    assert n.send(app.results, messages.LVM_GETNEXTITEM, -1, messages.LVNI_SELECTED) == 0
    app.command(1114, code=win32con.CBN_SELENDCANCEL)
    assert n.send(mode, win32con.CB_GETCURSEL) == 0
    assert (app.count(), app.cell(0), app.status_text(), profile.preferences()) == before


def test_search_after_scrolling_keeps_new_results_in_view(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("file")
    assert app.count() > 100
    for query in ("CreateFileW", "file"):
        n.send(app.results, 0x1013, app.count() - 1, 0)  # LVM_ENSUREVISIBLE
        assert n.send(app.results, 0x1027) > 0  # LVM_GETTOPINDEX
        app.query(query)
        assert app.count() > 0
        assert n.send(app.results, 0x1027) == 0
        _, top, _, bottom = app.item_rect(0)
        _, client_top, _, client_bottom = n.rectangle(app.results, client=True)
        assert 0 <= top < bottom <= client_bottom - client_top


def test_ime_defers_search(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    n.set_text(app.search, "CreateFileW")
    app.focus(app.search)
    n.key_message(app.search, win32con.VK_RETURN)
    time.sleep(.35)
    assert app.count() == 0
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.count() > 0, "search after IME commit", 45)

    mode = n.get_item(app.frame, 1114)
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    n.set_text(app.search, 'name = "CloseHandle"')
    n.send(mode, win32con.CB_SETCURSEL, 1)
    app.command(1114, code=win32con.CBN_SELENDOK)
    time.sleep(.35)
    assert app.count() == 0
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.named_rows("CloseHandle"), "mode search after IME commit", 45)


def test_search_syntax_help_preserves_query(launch, settings):
    app = launch([settings[1]])
    app.query("CrtFlW")
    count = app.count()
    n.post(app.frame, win32con.WM_COMMAND, 1053, 0)
    dialog = n.wait(lambda: next((h for h in n.windows()
                    if n.class_name(h) == "#32770" and n.is_visible(h)
                    and n.window_text(h) in ("Search syntax", "搜索语法")), None), "search syntax help")
    labels = "\n".join(n.window_text(h) for h in n.windows(dialog))
    assert 'name contains "access" and value = 5' in labels
    assert "value = nan or value = inf" in labels
    button = n.wait(lambda: next((h for h in n.windows(dialog)
                    if n.class_name(h) == "Button" and n.is_visible(h)), None), "help button")
    n.post(button, win32con.BM_CLICK, 0, 0)
    n.wait(lambda: not n.is_window(dialog), "help closed")
    assert n.window_text(app.search) == "CrtFlW"
    assert app.count() == count

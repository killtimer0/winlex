import time
import json
import subprocess
import pytest
import win32con
import win32gui
import uiautomation as auto
from support.diagnostics import capture, screenshot
from support import native as n
from support.session import invoke, select_combo
from test_search_help import open_help


@pytest.mark.interactive
def test_result_rows_show_context_and_match_emphasis(launch, settings):
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, prefix, name_match in [
        (0, "CrtFlW", "Apis · ", True),
        (1, 'name contains "access" and value = 5', "= 5 · ", False),
        (1, 'guid = "0000000c-0000-0000-c000-000000000046"', "{0000000c-0000-0000-c000-000000000046} · ", False),
    ]:
        select_combo(mode, app.choices(mode)[index])
        app.query(query)
        assert app.count() > 0
        row = app.named_rows("CreateFileW")[0] if name_match else 0
        app.rows()[row].GetScrollItemPattern().ScrollIntoView()
        lines = app.cell(row).splitlines()
        detail = lines[2]
        assert detail.startswith(prefix), detail
        with n.window_dpi_context(app.frame):
            left, top, right, bottom = app.item_rect(row)
            with capture(app.results, client=True) as (bitmap, _, _):
                accent = win32gui.GetSysColor(win32con.COLOR_HOTLIGHT)
                rgb = ((accent & 255) << 16) | (accent & 0xff00) | ((accent >> 16) & 255)
                assert any(bitmap.GetPixelColor(x, y) & 0xffffff == rgb
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
    ]:
        select_combo(mode, app.choices(mode)[index])
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
        assert [row.Name.splitlines()[0] for row in app.rows()] == [hit["full_name"] for hit in expected]


def test_search_mode_errors_and_navigation(launch, settings):
    app = launch([settings[1]])
    app.query("CrtFlW")
    mode = n.get_item(app.frame, 1114)
    chinese = app.toolbar_text(1040) == "搜索"
    assert auto.ControlFromHandle(mode).GetValuePattern().Value == app.choices(mode)[0]
    assert app.named_rows("CreateFileW")

    def choose(index):
        select_combo(mode, app.choices(mode)[index])

    choose(1)
    n.wait(lambda: ("条件" if chinese else "condition") in app.status_text(), "condition error")
    assert app.count() == 0
    app.query('name = "CreateFileW"')
    assert app.count() > 0
    n.send(mode, win32con.CB_SETCURSEL, 0)
    n.send(app.frame, win32con.WM_COMMAND, (win32con.CBN_SELENDCANCEL << 16) | 1114, mode)
    assert auto.ControlFromHandle(mode).GetValuePattern().Value == app.choices(mode)[1]
    auto.ControlFromHandle(app.search).GetValuePattern().SetValue('value =')
    n.wait(lambda: ("尚未输入完整" if chinese else "Incomplete") in app.status_text(), "incomplete condition")
    assert app.count() == 0
    auto.ControlFromHandle(app.search).GetValuePattern().SetValue('value < nan')
    n.wait(lambda: ("条件无效" if chinese else "Invalid condition") in app.status_text(), "NaN bound error")
    assert app.count() == 0
    app.focus(app.search)
    win32gui.PostMessage(app.search, win32con.WM_KEYDOWN, win32con.VK_TAB, 1)
    win32gui.PostMessage(app.search, win32con.WM_KEYUP, win32con.VK_TAB, -1073741823)
    n.wait(lambda: app.focused() == mode, "Tab reaches mode selector")
    win32gui.PostMessage(mode, win32con.WM_KEYDOWN, win32con.VK_TAB, 1)
    win32gui.PostMessage(mode, win32con.WM_KEYUP, win32con.VK_TAB, -1073741823)
    n.wait(lambda: app.focused() == app.results, "Tab reaches results")


def test_panel_switch_preserves_result_selection_and_scroll(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("file")
    assert app.count() > 100
    first, last = app.cell(0), app.cell(app.count() - 1)
    app.select(3)
    app.rows()[-1].GetScrollItemPattern().ScrollIntoView()
    top = next(i for i, item in enumerate(app.rows()) if not item.IsOffscreen)
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
    assert app.selected_row() == 3
    top = next(i for i, item in enumerate(app.rows()) if not item.IsOffscreen)
    assert app.cell(top) == anchor
    assert app.item_rect(top)[1] == offset


def test_panel_refresh_and_programmatic_mode_selection_do_not_submit_search(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    app.select(0)
    mode = n.get_item(app.frame, 1114)
    before = (app.count(), app.cell(0), app.status_text(), profile.preferences())
    # Selecting an item through the pattern is not a completed dropdown gesture.
    combo = auto.ControlFromHandle(mode)
    combo.GetExpandCollapsePattern().Expand()
    choices = [item for item, _ in auto.WalkControl(combo)
               if item.ControlType == auto.ControlType.ListItemControl]
    choices[1].GetSelectionItemPattern().Select()
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    app.command(1021)
    app.command(1040)
    assert (app.count(), app.cell(0), app.status_text(), profile.preferences()) == before
    assert app.selected_row() == 0
    combo.GetExpandCollapsePattern().Collapse()
    n.send(app.frame, win32con.WM_COMMAND, (win32con.CBN_SELENDCANCEL << 16) | 1114, mode)
    assert auto.ControlFromHandle(mode).GetValuePattern().Value == app.choices(mode)[0]
    assert (app.count(), app.cell(0), app.status_text(), profile.preferences()) == before


def test_search_after_scrolling_keeps_new_results_in_view(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("file")
    assert app.count() > 100
    for query in ("CreateFileW", "file"):
        app.rows()[-1].GetScrollItemPattern().ScrollIntoView()
        assert app.rows()[0].IsOffscreen
        app.query(query)
        assert app.count() > 0
        assert not app.rows()[0].IsOffscreen
        _, top, _, bottom = app.item_rect(0)
        _, client_top, _, client_bottom = n.rectangle(app.results, client=True)
        assert 0 <= top < bottom <= client_bottom - client_top


def test_ime_defers_search(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    auto.ControlFromHandle(app.search).GetValuePattern().SetValue('CreateFileW')
    app.focus(app.search)
    win32gui.PostMessage(app.search, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(app.search, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    time.sleep(.35)
    assert app.count() == 0
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.count() > 0, "search after IME commit", 45)

    mode = n.get_item(app.frame, 1114)
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    auto.ControlFromHandle(app.search).GetValuePattern().SetValue('name = "CloseHandle"')
    select_combo(mode, app.choices(mode)[1])
    time.sleep(.35)
    assert app.count() == 0
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.named_rows("CloseHandle"), "mode search after IME commit", 45)


def test_search_syntax_help_preserves_query(launch, settings):
    app = launch([settings[1]])
    app.query("CrtFlW")
    count = app.count()
    dialog = open_help(app)
    invoke(n.get_item(dialog, win32con.IDOK))
    n.wait(lambda: not n.is_window(dialog), "help closed")
    assert n.window_text(app.search) == "CrtFlW"
    assert app.count() == count

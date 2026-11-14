import time
import json
import subprocess
import pytest
import win32con
import win32gui
import winreg
import uiautomation as auto
from support.diagnostics import capture, screenshot
from support import deadline, messages, native as n
from support.session import invoke, select_combo
from test_search_help import open_help


@pytest.mark.interactive
def test_result_rows_show_context_and_match_emphasis(launch, profile, settings):
    # Keep the matched GUID visible rather than testing the folded {…} glyphs.
    profile.set('SidebarWidth', winreg.REG_DWORD, 480)
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, prefix, name_match in [
        (0, "CrtFlW", "Apis · ", True),
        (1, 'name contains "access" and value = 5', "= 5 · ", False),
        (1, 'guid = "0000000c-0000-0000-c000-000000000046"', "{0000000c-0000-0000-c000-000000000046} · ", False),
    ]:
        select_combo(mode, app.choices(mode)[index])
        app.query(query)
        row = app.named_rows("CreateFileW")[0] if name_match else 0
        with deadline.operation('scroll result row into view'):
            app.row(row).GetScrollItemPattern().ScrollIntoView()
        lines = app.cell(row).splitlines()
        detail = lines[2]
        assert detail.startswith(prefix), detail
        cell = app.cell(row)
        name = lines[0].rsplit('.', 1)[-1]

        def painted_row(row):
            with deadline.operation('scroll compared result row into view'):
                app.row(row).GetScrollItemPattern().ScrollIntoView()
            with n.window_dpi_context(app.frame):
                # Equal logical heights at different Y positions can span
                # different physical pixels under fractional DPI scaling.
                # Compare at the same viewport position, not with a tolerance.
                n.send(app.results, messages.LVM_SCROLL, 0, app.item_rect(row)[1])
                n.wait(lambda: app.item_rect(row)[1] == 0, 'compared row aligns with viewport top')
                left, top, right, bottom = app.item_rect(row)
                dc = win32gui.GetDC(app.results)
                font = n.send(app.results, win32con.WM_GETFONT)
                old = win32gui.SelectObject(dc, font)
                try:
                    text_width, _ = win32gui.GetTextExtentPoint32(dc, name if name_match else prefix)
                finally:
                    win32gui.SelectObject(dc, old)
                    win32gui.ReleaseDC(app.results, dc)
                # Exclude the trailing namespace and scrollbar: the number of
                # results can change the list's available width, not its text.
                right = min(right, left + text_width + bottom - top)
                middle = (top + bottom) // 2
                rect = (left, top, right, middle) if name_match else (left, middle, right, bottom)
                with capture(app.results, client=True, rect=rect) as (_, header, pixels):
                    return header, pixels

        matched = painted_row(row)
        screenshot(app.results, settings[2] / ("result-list-%d.png" % index))
        select_combo(mode, app.choices(mode)[1])
        # Double negation selects only this name without positive match spans;
        # a whole namespace would require thousands of unrelated UIA reads.
        baseline = 'not (not (name = ' + json.dumps(name) + '))'
        app.query(baseline)
        row = next(row for row in app.named_rows(name) if app.cell(row) == cell)
        unmatched = painted_row(row)
        assert matched[0] == unmatched[0], "matching must not change row geometry"
        assert matched[1] != unmatched[1], (query, "matching must change the same text's rendering")


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
        with deadline.operation('query CLI search results'):
            result = json.loads(subprocess.check_output(command, encoding="utf-8", timeout=45))["result"]
        expected = result["matches"]
        assert len(expected) < 1000
        # The GUI's default categories exclude ordinary fields and type references.
        expected = [hit for hit in expected if hit["kind"] not in {"field", "type_reference", "property", "event"}]
        with deadline.operation('read complete result row names'):
            names = [row.Name.splitlines()[0] for row in app.rows()]
        assert names == [hit["full_name"] for hit in expected]


def test_search_mode_errors_and_navigation(launch, settings):
    app = launch([settings[1]])
    app.query("CrtFlW")
    mode = n.get_item(app.frame, 1114)
    chinese = app.toolbar_text(1040) == "搜索"
    with deadline.operation('read quick search mode'):
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
    with deadline.operation('read condition search mode'):
        assert auto.ControlFromHandle(mode).GetValuePattern().Value == app.choices(mode)[1]
    with deadline.operation('set incomplete search condition'):
        auto.ControlFromHandle(app.search).GetValuePattern().SetValue('value =', waitTime=0)
    n.wait(lambda: ("尚未输入完整" if chinese else "Incomplete") in app.status_text(), "incomplete condition")
    assert app.count() == 0
    with deadline.operation('set invalid search condition'):
        auto.ControlFromHandle(app.search).GetValuePattern().SetValue('value < nan', waitTime=0)
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
    first, last = app.cell(0), app.cell(app.count() - 1)
    app.select(3)
    with deadline.operation('scroll last result row into view'):
        app.row(-1).GetScrollItemPattern().ScrollIntoView()
    top = app.top_row()
    assert top > 0
    anchor = app.cell(top)
    offset = app.item_rect(top)[1]
    app.command(1021)
    assert n.is_visible(app.sources) and not n.is_visible(app.results)
    app.command(1021)
    assert not n.is_visible(app.sources)
    app.command(1040)
    assert (app.cell(0), app.cell(app.count() - 1)) == (first, last)
    assert app.selected_row() == 3
    top = app.top_row()
    assert app.cell(top) == anchor
    assert app.item_rect(top)[1] == offset


def test_panel_refresh_and_programmatic_mode_selection_do_not_submit_search(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    app.select(0)
    mode = n.get_item(app.frame, 1114)
    before = (app.count(), app.cell(0), app.status_text(), profile.preferences())
    with deadline.operation('expand search mode selector'):
        combo = auto.ControlFromHandle(mode)
        original_mode = combo.GetValuePattern().Value
        combo.GetExpandCollapsePattern().Expand()
    assert n.send(mode, win32con.CB_SETCURSEL, 1) == 1
    with deadline.operation('read programmatically selected search mode'):
        assert combo.GetValuePattern().Value != original_mode
    n.send(app.frame, win32con.WM_SETTINGCHANGE)
    app.command(1021)
    app.command(1040)
    assert (app.count(), app.cell(0), app.status_text(), profile.preferences()) == before
    assert app.selected_row() == 0
    with deadline.operation('collapse search mode selector'):
        combo.GetExpandCollapsePattern().Collapse()
    n.send(app.frame, win32con.WM_COMMAND, (win32con.CBN_SELENDCANCEL << 16) | 1114, mode)
    with deadline.operation('read restored search mode'):
        assert auto.ControlFromHandle(mode).GetValuePattern().Value == app.choices(mode)[0]
    assert (app.count(), app.cell(0), app.status_text(), profile.preferences()) == before
    assert app.selected_row() == 0


def test_search_after_scrolling_keeps_new_results_in_view(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    app.query("file")
    for query in ("CreateFileW", "file"):
        with deadline.operation('scroll results and read first row visibility'):
            assert app.row(-1).GetScrollItemPattern().ScrollIntoView(waitTime=0)
            n.wait(lambda: app.row(0).IsOffscreen, 'first result scrolled out of view', 15)
        app.query(query)
        with deadline.operation('read new result row visibility'):
            assert not app.row(0).IsOffscreen
        _, top, _, bottom = app.item_rect(0)
        _, client_top, _, client_bottom = n.rectangle(app.results, client=True)
        assert 0 <= top < bottom <= client_bottom - client_top


def test_ime_defers_search(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
    idle_status = app.status_text()

    def observe_composition():
        # panel.rs::debounce schedules normal searches after 200 ms. Observe
        # through that scheduling opportunity, not just one empty result count.
        until = time.monotonic() + 0.2

        def still_idle():
            assert app.status_text() == idle_status, "search started during IME composition"
            assert app.count() == 0, "results appeared during IME composition"
            return time.monotonic() >= until

        # Waiting already changes status (catalog.rs::search_status), so fail
        # immediately even if the search has not produced any matching rows.
        n.wait(still_idle, "search remains idle during IME composition", 15)

    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    with deadline.operation('set search text during IME composition'):
        value = auto.ControlFromHandle(app.search).GetValuePattern()
        assert value.SetValue('CreateFileW', waitTime=0)
        n.wait(lambda: value.Value == 'CreateFileW', 'IME search text updated', 15)
    app.focus(app.search)
    n.send(app.search, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    n.send(app.search, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    observe_composition()
    n.send(app.search, win32con.WM_IME_ENDCOMPOSITION)
    n.wait(lambda: app.count() > 0, "search after IME commit", 45)

    mode = n.get_item(app.frame, 1114)
    n.send(app.search, win32con.WM_IME_STARTCOMPOSITION)
    with deadline.operation('set condition during IME composition'):
        value = auto.ControlFromHandle(app.search).GetValuePattern()
        assert value.SetValue('name = "CloseHandle"', waitTime=0)
        n.wait(lambda: value.Value == 'name = "CloseHandle"', 'IME condition text updated', 15)
    select_combo(mode, app.choices(mode)[1])
    observe_composition()
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

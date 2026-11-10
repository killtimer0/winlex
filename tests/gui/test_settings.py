from pathlib import Path
import json
import time
import pytest
import win32con
import win32gui
import winreg
import uiautomation as auto
from support import native as n
from support.session import invoke, select_combo
from support.diagnostics import capture, screenshot, save_pixels
from support.languages import LANGUAGES


def mask(app, sheet, value):
    page = app.settings_page(sheet, 1)
    for index in range(8):
        checkbox = auto.ControlFromHandle(n.get_item(page, 1310 + index))
        if checkbox.GetTogglePattern().ToggleState != (value >> index) & 1:
            checkbox.GetTogglePattern().Toggle()
    return page


def checked(app, sheet):
    page = app.settings_page(sheet, 1)
    return sum(auto.ControlFromHandle(n.get_item(page, 1310 + index)).GetTogglePattern().ToggleState << index
               for index in range(8))


def test_settings_apply_tracks_saved_values(launch, profile, ui_language):
    app = launch()
    original = profile.preferences()
    sheet = app.settings()
    apply = auto.ControlFromHandle(n.get_item(sheet, 0x3021))
    assert not apply.IsEnabled
    mask(app, sheet, 0)
    n.wait(lambda: apply.IsEnabled, "Apply enabled after edit")
    mask(app, sheet, original[1])
    n.wait(lambda: not apply.IsEnabled, "Apply disabled after reverting edit")
    general = app.settings_page(sheet, 0)
    alternative = next(option for option in LANGUAGES if option != ui_language)
    select_combo(n.get_item(general, 1300), alternative.label)
    n.wait(lambda: apply.IsEnabled, "Apply enabled after language edit")
    select_combo(n.get_item(general, 1300), ui_language.label)
    n.wait(lambda: not apply.IsEnabled, "Apply disabled after reverting language")
    select_combo(n.get_item(general, 1300), alternative.label)
    mask(app, sheet, 1)
    invoke(apply)
    n.wait(lambda: profile.preferences() == (alternative.name, 1, original[2]) and not apply.IsEnabled,
           "Apply saves without closing")
    assert n.is_window(sheet)
    mask(app, sheet, 0)
    invoke(apply)
    n.wait(lambda: profile.get("SearchCategories") == 0 and not apply.IsEnabled,
           "second Apply updates saved baseline")
    mask(app, sheet, 7)
    n.wait(lambda: apply.IsEnabled, "new edit after Apply")
    mask(app, sheet, 0)
    n.wait(lambda: not apply.IsEnabled, "revert to most recently applied value")
    mask(app, sheet, 255)
    app.close_settings(sheet, accept=False)
    assert profile.preferences() == (alternative.name, 0, original[2])
    sheet = app.settings()
    general = app.settings_page(sheet, 0)
    assert auto.ControlFromHandle(n.get_item(general, 1300)).GetValuePattern().Value == alternative.label
    assert checked(app, sheet) == 0
    app.close_settings(sheet, accept=False)


def test_settings_cancel_and_page_state(launch, profile, ui_language):
    original = profile.preferences()
    app = launch()
    sheet = app.settings()
    enabled = n.api(n.user, "IsWindowEnabled", n.w.BOOL, n.w.HWND)
    assert not enabled(app.frame)
    assert checked(app, sheet) == 135
    mask(app, sheet, 255)
    general = app.settings_page(sheet, 0)
    control = n.get_item(general, 1300)
    alternative = next(option for option in LANGUAGES if option != ui_language)
    select_combo(control, alternative.label)
    assert checked(app, sheet) == 255
    app.close_settings(sheet, accept=False)
    assert enabled(app.frame)
    assert profile.preferences() == original
    sheet = app.settings()
    assert checked(app, sheet) == 135
    app.close_settings(sheet)


def test_online_search_engine_apply_cancel_and_restart(launch, profile, settings, ui_language):
    from test_metadata_guide import resource_text

    app = launch()
    sheet = app.settings()
    page = app.settings_page(sheet, 1)
    combo = n.get_item(page, 1302)
    apply = auto.ControlFromHandle(n.get_item(sheet, 0x3021))
    label = lambda resource: resource_text(settings[0], resource, ui_language.langid)
    assert n.window_text(combo) == label(2390)
    assert profile.get('SearchEngine') is None and not apply.IsEnabled
    select_combo(combo, label(2391))
    n.wait(lambda: apply.IsEnabled, 'search engine edit enables Apply')
    select_combo(combo, label(2390))
    n.wait(lambda: not apply.IsEnabled, 'reverted engine disables Apply')
    select_combo(combo, label(2391))
    invoke(apply)
    n.wait(lambda: profile.get('SearchEngine') == 'bing' and not apply.IsEnabled,
           'Apply saves search engine')
    select_combo(combo, label(2392))
    app.close_settings(sheet, accept=False)
    assert profile.get('SearchEngine') == 'bing'
    app.close()
    app = launch()
    sheet = app.settings()
    combo = n.get_item(app.settings_page(sheet, 1), 1302)
    assert n.window_text(combo) == label(2391)
    select_combo(combo, label(2392))
    app.close_settings(sheet)
    assert profile.get('SearchEngine') == 'baidu'


def test_menu_scopes(launch, settings):
    app = launch([settings[1]])
    menu = n.api(n.user, "GetMenu", n.w.HMENU, n.w.HWND)(app.frame)
    submenu = n.api(n.user, "GetSubMenu", n.w.HMENU, n.w.HMENU, n.c.c_int)
    navigation = submenu(menu, 1)
    window = submenu(menu, 2)
    documents = []
    for name in ("CreateFileW", "CloseHandle"):
        app.query(name)
        doc = app.open_row(app.named_rows(name)[0])
        n.wait(lambda: name in app.content(doc), "window menu document", 45)
        documents.append(doc)
    get_id = n.api(n.user, "GetDlgCtrlID", n.c.c_int, n.w.HWND)
    get_state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)
    for doc in documents:
        command = get_id(doc)
        assert get_state(window, command, win32con.MF_BYCOMMAND) != 0xffffffff
        assert get_state(navigation, command, win32con.MF_BYCOMMAND) == 0xffffffff
        app.command(command)
        n.wait(lambda: app.active_document() == doc, "Window menu activates its document")
        assert get_state(window, command, win32con.MF_BYCOMMAND) & win32con.MF_CHECKED
    app.command(1015)
    n.wait(lambda: not n.is_window(doc), "Window menu closes active document")
    assert n.is_window(documents[0])


@pytest.mark.parametrize("action", ["escape", "close", "enter"])
def test_settings_keyboard_and_close(launch, profile, action):
    original = profile.preferences()
    app = launch()
    sheet = app.settings()
    page = mask(app, sheet, 0)
    first, second = n.get_item(page, 1310), n.get_item(page, 1317)
    app.focus(first)
    n.wait(lambda: app.focused() == first, "settings initial keyboard focus")
    win32gui.PostMessage(first, win32con.WM_KEYDOWN, win32con.VK_TAB, 1)
    win32gui.PostMessage(first, win32con.WM_KEYUP, win32con.VK_TAB, -1073741823)
    n.wait(lambda: app.focused() == second, "settings Tab order")
    if action == "close":
        auto.ControlFromHandle(sheet).GetWindowPattern().Close()
    else:
        key = win32con.VK_RETURN if action == 'enter' else win32con.VK_ESCAPE
        win32gui.PostMessage(second, win32con.WM_KEYDOWN, key, 1)
        if n.is_window(second):
            win32gui.PostMessage(second, win32con.WM_KEYUP, key, -1073741823)
    n.wait(lambda: not n.is_window(sheet), "settings keyboard close")
    assert profile.preferences() == ((original[0], 0, original[2]) if action == "enter" else original)


@pytest.mark.parametrize("action", ["ok", "apply"])
def test_settings_refresh_preserves_document(launch, profile, settings, action):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "original declaration", 45)
    body = app.content(doc)
    auto.ControlFromHandle(app.search).GetValuePattern().SetValue('file')
    sheet = app.settings()
    mask(app, sheet, 1)
    if action == "apply":
        invoke(n.get_item(sheet, 0x3021))
    else:
        app.close_settings(sheet)
        app.focus(app.search)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "refiltered search", 45)
    assert app.count() > 0
    assert all(row.Name.splitlines()[1].split(" · ")[0] in
               {"class", "struct", "interface", "enum", "delegate", "attribute", "module"}
               for row in app.rows())
    assert app.active_document() == doc
    assert app.content(doc) == body
    assert profile.get("SearchCategories") == 1
    assert profile.get("AutoLoad") is None
    if action == "apply":
        assert n.is_window(sheet)
        app.close_settings(sheet, accept=False)


@pytest.mark.parametrize("value", [0, 1, 2, 4, 8, 16, 32, 64, 128, 255])
def test_search_category_visibility(launch, profile, settings, value):
    language = profile.get("Language")
    fixture = Path(settings[1]).with_name("SearchEntries.dll")
    if not fixture.exists():
        pytest.skip("SearchEntries.dll fixture not deployed")
    app = launch([fixture])
    app.query("e")
    sheet = app.settings()
    mask(app, sheet, value)
    app.close_settings(sheet)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "filtered search completed")
    kinds = {row.Name.splitlines()[1].split(" · ")[0] for row in app.rows()}
    expected = set()
    for bit, names in [(1, {"class", "module"}), (2, {"method"}), (4, {"constant"}),
                       (8, {"field"}), (16, {"property"}), (32, {"event"}),
                       (64, {"type_reference"}), (128, {"function"})]:
        if value & bit:
            expected.update(names)
    assert kinds == expected
    assert profile.preferences() == (language, value, "system")
    if not value:
        assert app.count() == 0
    if value & 2:
        rows = [row.Name for row in app.rows()]
        assert sum(text.splitlines()[0].rsplit(".", 1)[-1] == "EntryMethod" for text in rows) == 2
    sheet = app.settings()
    assert checked(app, sheet) == value
    app.close_settings(sheet, accept=False)
    if value in (0, 255):
        app.close()
        restored = launch([fixture])
        sheet = restored.settings()
        assert checked(restored, sheet) == value
        restored.close_settings(sheet, accept=False)


@pytest.mark.parametrize("button", [1, 0x3021], ids=["ok", "apply"])
def test_settings_save_failure_preserves_state(launch, profile, button):
    original = profile.preferences()
    profile.set("Version", winreg.REG_DWORD, 99)
    app = launch()
    sheet = app.settings()
    mask(app, sheet, 0)
    invoke(n.get_item(sheet, button))
    error_control = auto.WindowControl(searchDepth=3, ClassName='#32770',
                                       Compare=lambda control, depth: control.ProcessId == app.process.pid
                                       and control.NativeWindowHandle != sheet)
    assert error_control.Exists(5)
    error = error_control.NativeWindowHandle
    invoke(error_control.ButtonControl())
    n.wait(lambda: not n.is_window(error), "save error dismissed")
    assert n.is_window(sheet)
    assert profile.get("Version") == 99 and profile.preferences() == original
    assert checked(app, sheet) == 0
    assert auto.ControlFromHandle(n.get_item(sheet, 0x3021)).IsEnabled
    app.close_settings(sheet, accept=False)
    sheet = app.settings()
    assert checked(app, sheet) == 135
    app.close_settings(sheet, accept=False)


@pytest.mark.parametrize("mode", ["pmv1", "pmv2", "system", "unaware"])
def test_settings_dpi_pages(launch, settings, mode):
    app = launch(dpi=mode)
    evidence = []
    output = settings[2] / ("settings-dpi-" + mode + ".json")
    get_dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND) if hasattr(n.user, "GetDpiForWindow") else lambda _h: None
    for monitor_index, monitor in enumerate(n.monitors()):
        with n.window_dpi_context(app.frame):
            auto.ControlFromHandle(app.frame).GetPattern(auto.PatternId.TransformPattern).Move(
                monitor[0] + 30, monitor[1] + 30)
        sheet = app.settings()
        for index in (0, 1, 0, 1):
            page = app.settings_page(sheet, index)
            with n.window_dpi_context(sheet):
                outer = n.rectangle(page)
                controls = []
                for element in auto.ControlFromHandle(page).GetChildren():
                    control = element.NativeWindowHandle
                    rect = element.BoundingRectangle
                    controls.append({
                        "id": n.get_id(control),
                        "class": n.class_name(control),
                        "rect": (rect.left, rect.top, rect.right, rect.bottom),
                        "text": element.Name,
                        "font": n.send(control, win32con.WM_GETFONT),
                    })
                evidence.append({
                    "mode": mode,
                    "monitor": monitor,
                    "dpi": get_dpi(sheet),
                    "sheet": n.rectangle(sheet),
                    "page": outer,
                    "controls": controls,
                })
                output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
                for control in controls:
                    rect = control["rect"]
                    assert outer[0] <= rect[0] < rect[2] <= outer[2] + 1, control
                    assert outer[1] <= rect[1] < rect[3] <= outer[3] + 1, control
        with n.window_dpi_context(sheet):
            initial = n.rectangle(sheet)
            returns = []
            for destination in (list(n.monitors()) + [monitor]) * 3:
                auto.ControlFromHandle(sheet).GetPattern(auto.PatternId.TransformPattern).Move(
                    destination[0] + 40, destination[1] + 40)
                app.settings_page(sheet, 1)
                evidence.append({"move": destination, "dpi": get_dpi(sheet), "rect": n.rectangle(sheet)})
                if destination == monitor:
                    r = n.rectangle(sheet)
                    returns.append((r[2]-r[0], r[3]-r[1]))
            final = n.rectangle(sheet)
            evidence.append({"initial": initial, "final": final})
            output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
            # Native property sheets may round their size on the first DPI round trip.
            assert all(abs(width-returns[-1][0]) <= 1 and abs(height-returns[-1][1]) <= 1
                       for width, height in returns[1:])
        for index, name in ((0, "general"), (1, "search")):
            app.settings_page(sheet, index)
            n.api(n.user, "RedrawWindow", n.w.BOOL, n.w.HWND, n.c.c_void_p, n.c.c_void_p, n.w.UINT)(sheet, None, None, 0x185)
            screenshot(sheet, settings[2] / ("settings-" + mode + "-" + str(monitor_index) + "-" + name + ".png"))
        app.close_settings(sheet, accept=False)
    output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")


@pytest.mark.interactive
def test_settings_pmv1_initial_paint(launch, settings):
    if not hasattr(n.user, "GetDpiForWindow"):
        pytest.skip("per-monitor DPI unavailable")
    app = launch(dpi="pmv1")
    get_dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)
    get_system_dpi = n.api(n.user, "GetDpiForSystem", n.w.UINT)
    with n.window_dpi_context(app.frame):
        system_dpi = get_system_dpi()
        frame = auto.ControlFromHandle(app.frame)
        assert frame.SetTopmost(True)
        for monitor in n.monitors():
            frame.GetPattern(auto.PatternId.TransformPattern).Move(monitor[0] + 30, monitor[1] + 30)
            if get_dpi(app.frame) != system_dpi:
                break
        else:
            pytest.skip("no monitor with a different DPI")
    # Do not select pages or render a screenshot before observing the first paint.
    auto.ControlFromHandle(app.frame).SendKeys('{Alt}fs')
    sheet_control = auto.WindowControl(searchDepth=3, RegexName='^(Settings|设置)$',
                                       Compare=lambda item, depth: item.ProcessId == app.process.pid)
    assert sheet_control.Exists(5)
    sheet = sheet_control.NativeWindowHandle
    assert get_dpi(sheet) != system_dpi
    page = sheet_control.ComboBoxControl(AutomationId='1300').GetParentControl()
    label = page.TextControl().NativeWindowHandle

    def pixels():
        with n.window_dpi_context(label), capture(label, client=True) as (_, header, raw):
            return header, raw

    previous, since = None, time.monotonic()

    def stable():
        nonlocal previous, since
        current = pixels()
        if current != previous:
            previous, since = current, time.monotonic()
        return current if time.monotonic() - since >= .3 else None

    before = n.wait(stable, "stable initial text paint")
    assert len(set(before[1][::4])) > 1, "visible text pixels were not captured"
    win32gui.RedrawWindow(sheet, None, None,
                         win32con.RDW_INVALIDATE | win32con.RDW_ERASE |
                         win32con.RDW_ALLCHILDREN | win32con.RDW_UPDATENOW)
    after = pixels()
    for name, (header, raw) in (("initial", before), ("repainted", after)):
        save_pixels(settings[2] / ("settings-pmv1-" + name + ".png"), header, raw)
    assert before == after, "initial text changes after an otherwise redundant repaint"
    app.close_settings(sheet, accept=False)

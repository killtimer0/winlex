from pathlib import Path
import json
import pytest
import win32con
import winreg
from support import native as n
from support.diagnostics import screenshot


def mask(app, sheet, value):
    page = app.settings_page(sheet, 1)
    for index in range(7):
        n.send(n.get_item(page, 1310 + index), win32con.BM_SETCHECK, (value >> index) & 1)
    return page


def checked(app, sheet):
    page = app.settings_page(sheet, 1)
    return sum(n.send(n.get_item(page, 1310 + index), win32con.BM_GETCHECK) << index
               for index in range(7))


def test_settings_cancel_and_page_state(launch, profile):
    original = profile.get("Preferences")
    app = launch()
    sheet = app.settings()
    enabled = n.api(n.user, "IsWindowEnabled", n.w.BOOL, n.w.HWND)
    assert not enabled(app.frame)
    chinese = app.toolbar_text(1040) == "搜索"
    assert n.window_text(n.get_item(sheet, 1)) == ("确定" if chinese else "OK")
    assert n.window_text(n.get_item(sheet, 2)) == ("取消" if chinese else "Cancel")
    assert not n.is_visible(n.get_item(sheet, 0x3021))
    assert checked(app, sheet) == 7
    page = app.settings_page(sheet, 1)
    labels = (["类型(&T)", "函数/方法(&M)", "常量(&C)", "字段(&F)", "属性(&P)", "事件(&E)", "类型引用(&R)"]
              if chinese else ["&Types", "Functions/&Methods", "&Constants", "&Fields", "&Properties", "&Events", "Type &references"])
    assert [n.window_text(n.get_item(page, 1310+i)) for i in range(7)] == labels
    mask(app, sheet, 127)
    general = app.settings_page(sheet, 0)
    n.send(n.get_item(general, 1300), win32con.CB_SETCURSEL, 2)
    assert checked(app, sheet) == 127
    app.close_settings(sheet, accept=False)
    assert enabled(app.frame)
    assert profile.get("Preferences") == original
    sheet = app.settings()
    assert checked(app, sheet) == 7
    app.close_settings(sheet)


def test_menu_scopes(launch, settings):
    app = launch([settings[1]])
    menu = n.api(n.user, "GetMenu", n.w.HMENU, n.w.HWND)(app.frame)
    count = n.api(n.user, "GetMenuItemCount", n.c.c_int, n.w.HMENU)
    submenu = n.api(n.user, "GetSubMenu", n.w.HMENU, n.w.HMENU, n.c.c_int)
    item_id = n.api(n.user, "GetMenuItemID", n.w.UINT, n.w.HMENU, n.c.c_int)
    assert count(menu) == 4
    navigation = submenu(menu, 1)
    window = submenu(menu, 2)
    file_ids = [item_id(submenu(menu, 0), i) for i in range(count(submenu(menu, 0)))]
    window_ids = [item_id(window, i) for i in range(count(window))]
    assert 1020 in file_ids and 1051 in file_ids and 1004 in file_ids
    assert not {1049, 1050, 1003}.intersection(file_ids)
    assert [item_id(navigation, i) for i in range(count(navigation))] == [1060, 1061, 1062, 0, 1055, 1056]
    assert 1003 in window_ids
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


@pytest.mark.parametrize("action", ["escape", "close", "enter"])
def test_settings_keyboard_and_close(launch, profile, action):
    original = profile.get("Preferences")
    app = launch()
    sheet = app.settings()
    page = mask(app, sheet, 0)
    first, second = n.get_item(page, 1310), n.get_item(page, 1311)
    app.focus(first)
    n.key_message(first, win32con.VK_TAB)
    n.wait(lambda: app.focused() == second, "settings Tab order")
    if action == "close":
        n.post(sheet, win32con.WM_CLOSE, 0, 0)
    else:
        n.key_message(second, win32con.VK_RETURN if action == "enter" else win32con.VK_ESCAPE)
    n.wait(lambda: not n.is_window(sheet), "settings keyboard close")
    assert profile.get("Preferences") == (original.split(":")[0] + ":0" if action == "enter" else original)


def test_settings_refresh_preserves_document(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "original declaration", 45)
    body = app.content(doc)
    n.set_text(app.search, "file")
    sheet = app.settings()
    mask(app, sheet, 1)
    app.close_settings(sheet)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "refiltered search", 45)
    assert app.count() > 0
    assert all(app.cell(row).splitlines()[1].split(" · ")[0] in
               {"class", "struct", "interface", "enum", "delegate", "attribute", "module"}
               for row in range(app.count()))
    assert app.content(doc) == body
    assert profile.get("AutoLoad") is None


@pytest.mark.parametrize("value", [0, 1, 2, 4, 8, 16, 32, 64, 127])
def test_search_category_visibility(launch, profile, settings, value):
    language = profile.get("Preferences").split(":")[0]
    fixture = Path(settings[1]).with_name("SearchEntries.dll")
    if not fixture.exists():
        pytest.skip("SearchEntries.dll fixture not deployed")
    app = launch([fixture])
    app.query("e")
    sheet = app.settings()
    mask(app, sheet, value)
    app.close_settings(sheet)
    n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "filtered search completed")
    kinds = {app.cell(row).splitlines()[1].split(" · ")[0] for row in range(app.count())}
    expected = set()
    for bit, names in [(1, {"class", "module"}), (2, {"method"}), (4, {"constant"}),
                       (8, {"field"}), (16, {"property"}), (32, {"event"}), (64, {"type_reference"})]:
        if value & bit:
            expected.update(names)
    assert kinds == expected
    assert profile.get("Preferences") == language + ":" + str(value)
    if not value:
        assert app.count() == 0
    if value & 2:
        rows = [app.cell(row) for row in range(app.count())]
        assert sum(text.splitlines()[0].rsplit(".", 1)[-1] == "EntryMethod" for text in rows) == 2
    sheet = app.settings()
    assert checked(app, sheet) == value
    app.close_settings(sheet, accept=False)
    if value in (0, 127):
        app.close()
        restored = launch([fixture])
        sheet = restored.settings()
        assert checked(restored, sheet) == value
        restored.close_settings(sheet, accept=False)


def test_settings_save_failure_preserves_state(launch, profile):
    original = profile.get("Preferences")
    profile.set("Version", winreg.REG_DWORD, 99)
    app = launch()
    sheet = app.settings()
    mask(app, sheet, 0)
    assert n.post(sheet, win32con.WM_COMMAND, 1, 0)
    owner = n.api(n.user, "GetWindow", n.w.HWND, n.w.HWND, n.w.UINT)
    error = n.wait(lambda: next((h for h in n.windows() if n.class_name(h) == "#32770"
                  and owner(h, win32con.GW_OWNER) == sheet and n.is_visible(h)
                  and (n.get_item(h, 1) or n.get_item(h, 2))), None), "save error")
    button = n.wait(lambda: n.get_item(error, 1) or n.get_item(error, 2), "error confirmation button")
    n.post(button, win32con.BM_CLICK, 0, 0)
    n.wait(lambda: not n.is_window(error), "save error dismissed")
    assert n.is_window(sheet)
    assert profile.get("Version") == 99 and profile.get("Preferences") == original
    app.close_settings(sheet, accept=False)
    sheet = app.settings()
    assert checked(app, sheet) == 7
    app.close_settings(sheet, accept=False)


@pytest.mark.parametrize("mode", ["pmv1", "pmv2", "system", "unaware"])
def test_settings_dpi_pages(launch, settings, mode):
    app = launch(dpi=mode)
    evidence = []
    output = settings[2] / ("settings-dpi-" + mode + ".json")
    get_dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND) if hasattr(n.user, "GetDpiForWindow") else lambda _h: None
    for monitor_index, monitor in enumerate(n.monitors()):
        with n.window_dpi_context(app.frame):
            n.set_pos(app.frame, None, monitor[0] + 30, monitor[1] + 30, 0, 0,
                      win32con.SWP_NOSIZE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
        sheet = app.settings()
        for index in (0, 1, 0, 1):
            page = app.settings_page(sheet, index)
            with n.window_dpi_context(sheet):
                outer = n.rectangle(page)
                controls = []
                for control in n.windows(page):
                    rect = n.rectangle(control)
                    controls.append({"id": n.get_id(control), "class": n.class_name(control), "rect": rect, "text": n.window_text(control), "font": n.send(control, win32con.WM_GETFONT)})
                evidence.append({"mode": mode, "monitor": monitor, "dpi": get_dpi(sheet), "sheet": n.rectangle(sheet), "page": outer, "controls": controls})
                output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")
                for control in controls:
                    rect = control["rect"]
                    assert outer[0] <= rect[0] < rect[2] <= outer[2] + 1, control
                    assert outer[1] <= rect[1] < rect[3] <= outer[3] + 1, control
        with n.window_dpi_context(sheet):
            initial = n.rectangle(sheet)
            returns = []
            for destination in (list(n.monitors()) + [monitor]) * 3:
                n.set_pos(sheet, None, destination[0] + 40, destination[1] + 40, 0, 0,
                          win32con.SWP_NOSIZE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
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
            screenshot(sheet, settings[2] / ("settings-" + mode + "-" + str(monitor_index) + "-" + name + ".bmp"))
        app.close_settings(sheet, accept=False)
    output.write_text(json.dumps(evidence, ensure_ascii=False, indent=2), encoding="utf-8")

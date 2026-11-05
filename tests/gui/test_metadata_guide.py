import struct
import win32api
import win32con
import win32gui
import winreg

from support import native as n


def resource_text(exe, identifier, language):
    module = win32api.LoadLibraryEx(str(exe), 0, win32con.LOAD_LIBRARY_AS_DATAFILE)
    try:
        data = win32api.LoadResource(module, win32con.RT_STRING, identifier // 16 + 1, language)
        offset = 0
        for _ in range(identifier % 16):
            length, = struct.unpack_from("<H", data, offset)
            offset += 2 + length * 2
        length, = struct.unpack_from("<H", data, offset)
        return data[offset + 2:offset + 2 + length * 2].decode("utf-16le")
    finally:
        win32api.FreeLibrary(module)


def owned_dialogs(app):
    owner = n.api(n.user, "GetWindow", n.w.HWND, n.w.HWND, n.w.UINT)
    return [hwnd for hwnd in n.windows()
            if n.class_name(hwnd) == "#32770" and n.is_visible(hwnd)
            and owner(hwnd, win32con.GW_OWNER) == app.frame]


def assert_no_dialog(app):
    n.send(app.frame, win32con.WM_NULL)
    assert win32gui.IsWindowEnabled(app.frame)
    assert not owned_dialogs(app)


def test_empty_startup_prompt_dismissal_and_runtime_removal(launch, profile, settings, ui_language):
    app = launch(metadata_prompt=True)
    prompt = n.wait(lambda: next(iter(owned_dialogs(app)), None), "metadata guide prompt")
    try:
        assert not win32gui.IsWindowEnabled(app.frame)
    finally:
        n.post(prompt, win32con.WM_COMMAND, win32con.IDNO, 0)
        n.wait(lambda: not n.is_window(prompt), "metadata prompt dismissed")

    assert_no_dialog(app)
    empty = resource_text(settings[0], 2345, ui_language.langid)
    n.wait(lambda: app.status_text() == empty, "empty metadata guidance")
    menu = win32gui.GetMenu(app.frame)
    help_menu = win32gui.GetSubMenu(menu, win32gui.GetMenuItemCount(menu) - 1)
    assert 1023 in [win32gui.GetMenuItemID(help_menu, index)
                    for index in range(win32gui.GetMenuItemCount(help_menu))]
    app.pick([settings[1]])
    n.wait(lambda: app.count(app.sources) == 1, "source loaded", 45)
    app.command(1021)
    assert app.status_text() != empty
    app.select(0, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0 and profile.get("AutoLoad") == [],
           "source removed", 45)
    n.wait(lambda: app.status_text() == empty, "guidance after removing last source")
    assert_no_dialog(app)
    app.command(1040)
    assert app.status_text() == empty
    assert_no_dialog(app)


def test_failed_configured_source_does_not_prompt(launch, profile, settings):
    missing = str(settings[2] / "missing-guide-restore.winmd")
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [missing])
    app = launch()
    app.command(1021)
    n.wait(lambda: "failed" in app.status_text() or "失败" in app.status_text(),
           "configured source load failure", 45)
    assert app.count(app.sources) == 0
    assert profile.get("AutoLoad") == [missing]
    assert_no_dialog(app)


def test_command_line_source_does_not_prompt(launch, settings):
    app = launch([settings[1]])
    n.wait(lambda: app.count(app.sources) == 1, "command-line source loaded", 45)
    assert_no_dialog(app)

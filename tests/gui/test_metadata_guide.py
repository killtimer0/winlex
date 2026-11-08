import struct
import win32api
import win32con
import win32gui
import uiautomation as auto

from support import native as n
from support.session import invoke, message_button


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
    dialog = auto.WindowControl(searchDepth=3, ClassName='#32770',
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    return [dialog.NativeWindowHandle] if dialog.Exists(0, 0) else []


def assert_no_dialog(app):
    assert win32gui.IsWindowEnabled(app.frame)
    assert not owned_dialogs(app)


def test_empty_startup_prompt_dismissal_and_runtime_removal(launch, profile, settings, ui_language):
    app = launch(metadata_prompt=True)
    prompt = n.wait(lambda: next(iter(owned_dialogs(app)), None), "metadata guide prompt")
    try:
        assert not win32gui.IsWindowEnabled(app.frame)
    finally:
        invoke(n.wait(lambda: message_button(prompt, win32con.IDNO), 'metadata prompt No button'))
        n.wait(lambda: not n.is_window(prompt), "metadata prompt dismissed")

    assert_no_dialog(app)
    empty = resource_text(settings[0], 2345, ui_language.langid)
    n.wait(lambda: app.status_text() == empty, "empty metadata guidance")
    menu = win32gui.GetMenu(app.frame)
    help_menu = win32gui.GetSubMenu(menu, win32gui.GetMenuItemCount(menu) - 1)
    assert 1023 in [win32gui.GetMenuItemID(help_menu, index)
                    for index in range(win32gui.GetMenuItemCount(help_menu))]
    app.pick([settings[1]])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "source loaded", 45)
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

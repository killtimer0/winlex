import win32con
import win32gui
from support import native as n
from test_tools import tool
from test_about import open_about


def topmost(hwnd):
    return bool(win32gui.GetWindowLong(hwnd, win32con.GWL_EXSTYLE) & win32con.WS_EX_TOPMOST)


def test_topmost_preserves_window_state_and_is_instance_local(launch):
    app = launch()

    def check(expected):
        assert topmost(app.frame) == expected
        menu = win32gui.GetMenu(app.frame)
        n.send(app.frame, win32con.WM_INITMENUPOPUP, win32gui.GetSubMenu(menu, 2), 2)
        state = win32gui.GetMenuState(menu, 1010, win32con.MF_BYCOMMAND)
        assert state != -1
        assert bool(state & win32con.MF_CHECKED) == expected

    check(False)
    before = win32gui.GetWindowPlacement(app.frame)
    bounds = win32gui.GetWindowRect(app.frame)
    focused = app.focused()
    app.command(1010)
    check(True)
    assert win32gui.GetWindowPlacement(app.frame) == before
    assert win32gui.GetWindowRect(app.frame) == bounds
    assert app.focused() == focused

    first = tool(app)
    second = tool(app, new=True)
    n.send(app.mdi, win32con.WM_MDIACTIVATE, first)
    check(True)
    n.send(app.mdi, win32con.WM_MDIACTIVATE, second)
    for command in (win32con.SC_MINIMIZE, win32con.SC_RESTORE, win32con.SC_MAXIMIZE):
        n.send(app.frame, win32con.WM_SYSCOMMAND, command)
        check(True)
    maximized = win32gui.GetWindowPlacement(app.frame)
    app.command(1010)
    check(False)
    assert win32gui.GetWindowPlacement(app.frame) == maximized
    app.command(1010)
    check(True)
    n.send(app.frame, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)

    sheet = app.settings()
    assert topmost(sheet)
    assert not win32gui.IsWindowEnabled(app.frame)
    app.close_settings(sheet, accept=False)
    assert win32gui.IsWindowEnabled(app.frame)
    check(True)

    dialog = open_about(app)
    assert topmost(dialog)
    assert not win32gui.IsWindowEnabled(app.frame)
    assert n.post(dialog, win32con.WM_COMMAND, win32con.IDOK, 0)
    n.wait(lambda: not n.is_window(dialog), "about closed while topmost")
    assert win32gui.IsWindowEnabled(app.frame)

    other = launch()
    assert not topmost(other.frame)
    other.close()
    app.close()
    restarted = launch()
    assert not topmost(restarted.frame)

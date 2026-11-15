"""Observe history command availability through the native Navigate menu."""
import win32con
from support import native as n


HISTORY_BACK = 1055
HISTORY_FORWARD = 1056


def enabled(app, command):
    _, _, menu = history_menu(app)
    state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)(
        menu, command, win32con.MF_BYCOMMAND)
    assert state != 0xffffffff, "history command exists in native menu"
    return not state & (win32con.MF_DISABLED | win32con.MF_GRAYED)


def history_menu(app):
    menu = n.api(n.user, "GetMenu", n.w.HMENU, n.w.HWND)(app.frame)
    count = n.api(n.user, "GetMenuItemCount", n.c.c_int, n.w.HMENU)
    submenu = n.api(n.user, "GetSubMenu", n.w.HMENU, n.w.HMENU, n.c.c_int)
    item = n.api(n.user, "GetMenuItemID", n.w.UINT, n.w.HMENU, n.c.c_int)
    for index in range(count(menu)):
        child = submenu(menu, index)
        if child and HISTORY_BACK in [item(child, i) for i in range(count(child))]:
            return menu, index, child
    raise AssertionError("native Navigate menu contains history commands")


def state(app, back, forward):
    n.wait(lambda: (enabled(app, HISTORY_BACK), enabled(app, HISTORY_FORWARD)) == (back, forward),
           "history menu availability")

"""Content-independent tab input and accessibility exposed by the running application."""
import ctypes as c
from ctypes import wintypes as w
import win32con
import win32gui
import uiautomation as auto
from support import native as n
from support.session import ThreadInfo


def center(page):
    rect = page.BoundingRectangle
    return rect.left + min(20, (rect.right - rect.left) // 2), (rect.top + rect.bottom) // 2


def test_tabs_msaa_activation_close_and_reorder(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    first = app.open_row()
    app.query("CloseHandle")
    second = app.open_row()
    tabs = n.get_item(app.frame, 1104)
    root = auto.ControlFromHandle(tabs)
    pages = app._tabs()
    assert root.GetLegacyIAccessiblePattern().Role == 0x3c
    assert len(pages) == 2 and all(page.GetLegacyIAccessiblePattern().Role == 0x25 for page in pages)
    names = [page.Name for page in pages]
    assert root.GetLegacyIAccessiblePattern().Select(1, waitTime=0)
    assert pages[0].GetLegacyIAccessiblePattern().DoDefaultAction(waitTime=0)
    n.wait(lambda: app.active_document() == first, "MSAA activation")
    assert pages[0].GetLegacyIAccessiblePattern().State & 2
    assert pages[0].GetLegacyIAccessiblePattern().State & 4
    win32gui.PostMessage(tabs, win32con.WM_KEYDOWN, win32con.VK_END, 1)
    win32gui.PostMessage(tabs, win32con.WM_KEYUP, win32con.VK_END, -1073741823)
    n.wait(lambda: app.active_document() == second, "keyboard last tab")
    assert pages[1].GetLegacyIAccessiblePattern().State & 2
    origin = n.rectangle(tabs)
    points = []
    for page in pages:
        x, y = center(page)
        points.append(((x - origin[0]) & 0xffff) | (((y - origin[1]) & 0xffff) << 16))
    n.send(tabs, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, points[1])
    n.send(tabs, win32con.WM_MOUSEMOVE, win32con.MK_LBUTTON, points[0])
    n.send(tabs, win32con.WM_LBUTTONUP, 0, points[0])
    n.wait(lambda: app.tab_labels() == list(reversed(names)), "tab reorder")
    assert [page.Name for page in pages] == names
    assert all(not page.GetChildren() for page in pages)
    rect = pages[1].BoundingRectangle
    x, y = rect.right - 4 - origin[0], (rect.top + rect.bottom) // 2 - origin[1]
    point = (x & 0xffff) | ((y & 0xffff) << 16)
    n.send(tabs, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
    n.send(tabs, win32con.WM_LBUTTONUP, 0, point)
    assert len(app.documents()) == 2, "right edge is part of the tab, not a close button"
    n.send(tabs, win32con.WM_MBUTTONUP, 0, point)
    n.wait(lambda: app.documents() == [first], "middle click close")
    n.wait(lambda: app.focused() == tabs, "close keeps tab focus")
    win32gui.PostMessage(tabs, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(tabs, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    n.wait(lambda: app.focused() == n.get_item(first, 1203), "enter focuses content")


def test_tabs_focus_only_selection_none_and_cancel_capture(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    app.open_row()
    app.query("CloseHandle")
    active = app.open_row()
    tabs = n.get_item(app.frame, 1104)
    root = auto.ControlFromHandle(tabs)
    pages = app._tabs()
    assert pages[0].GetLegacyIAccessiblePattern().Select(1, waitTime=0)
    assert app.active_document() == active, "TAKEFOCUS must not select"
    assert pages[0].GetLegacyIAccessiblePattern().State & 4 and not pages[0].GetLegacyIAccessiblePattern().State & 2
    assert pages[1].GetLegacyIAccessiblePattern().State & 2 and not pages[1].GetLegacyIAccessiblePattern().State & 4
    assert root.GetLegacyIAccessiblePattern().Select(0, waitTime=0)
    assert pages[0].GetLegacyIAccessiblePattern().State & 4 and app.active_document() == active
    get_info = n.api(n.user, "GetGUIThreadInfo", w.BOOL, w.DWORD, c.POINTER(ThreadInfo))

    def capture():
        info = ThreadInfo()
        info.size = c.sizeof(info)
        assert get_info(n.get_pid(app.frame, None), c.byref(info))
        return info.capture

    origin = n.rectangle(tabs)
    points = []
    for page in pages:
        x, y = center(page)
        points.append(((x - origin[0]) & 0xffff) | (((y - origin[1]) & 0xffff) << 16))
    start, end = points
    names = app.tab_labels()
    for cancel in (win32con.WM_CANCELMODE, win32con.WM_KEYDOWN):
        n.send(tabs, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, start)
        n.send(tabs, win32con.WM_MOUSEMOVE, win32con.MK_LBUTTON, end)
        n.wait(lambda: capture() == tabs, "tab drag capture")
        if cancel == win32con.WM_KEYDOWN:
            win32gui.PostMessage(tabs, win32con.WM_KEYDOWN, win32con.VK_ESCAPE, 1)
            win32gui.PostMessage(tabs, win32con.WM_KEYUP, win32con.VK_ESCAPE, -1073741823)
        else:
            n.send(tabs, cancel)
        n.wait(lambda: not capture(), "cancel releases tab capture")
        n.send(tabs, win32con.WM_LBUTTONUP, 0, end)
        assert app.tab_labels() == names

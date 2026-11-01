"""Content-independent tab input and MSAA exposed by the running application."""
import win32con
from support import native as n
from support.accessibility import client


def test_msaa_nested_clients_preserve_retained_objects(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    app.open_row()
    tabs = n.get_item(app.frame, 1104)
    with client(tabs) as root, root.children() as pages:
        name = root.name
        assert len(pages) == 1
        title = pages[0].name
        with client(tabs) as nested, nested.children() as current:
            assert nested.name == name
            assert [page.name for page in current] == [title]
        assert root.name == name
        assert pages[0].name == title
        assert pages[0].location()[2] > 0


def test_tabs_msaa_activation_close_and_reorder(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    first = app.open_row()
    app.query("CloseHandle")
    second = app.open_row()
    tabs = n.get_item(app.frame, 1104)
    with client(tabs) as root, root.children() as pages:
        assert root.role == 0x3c  # ROLE_SYSTEM_PAGETABLIST
        assert len(pages) == 2 and all(page.role == 0x25 for page in pages)
        names = [page.name for page in pages]
        app.focus(tabs)
        pages[0].action()
        n.wait(lambda: app.active_document() == first, "MSAA activation")
        assert pages[0].state & 2 and pages[0].state & 4  # selected and focused
        n.key_message(tabs, win32con.VK_END)
        n.wait(lambda: app.active_document() == second, "keyboard last tab")
        assert pages[1].state & 2
        # Drag the second tab onto the first; retained accessible objects stay bound.
        points = []
        origin = n.rectangle(tabs)
        for page in pages:
            x, y, width, height = page.location()
            points.append((x - origin[0] + min(20, width // 2), y - origin[1] + height // 2))
        def packed(point):
            return (point[0] & 0xffff) | ((point[1] & 0xffff) << 16)
        n.send(tabs, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, packed(points[1]))
        n.send(tabs, win32con.WM_MOUSEMOVE, win32con.MK_LBUTTON, packed(points[0]))
        n.send(tabs, win32con.WM_LBUTTONUP, 0, packed(points[0]))
        n.wait(lambda: app.tab_labels() == list(reversed(names)), "tab reorder")
        assert [page.name for page in pages] == names
        for page in pages:
            with page.children() as children:
                assert not children
        x, y, width, height = pages[1].location()
        point = (x - origin[0] + width - 4, y - origin[1] + height // 2)
        n.send(tabs, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, packed(point))
        n.send(tabs, win32con.WM_LBUTTONUP, 0, packed(point))
        assert len(app.documents()) == 2, "right edge is part of the tab, not a close button"
        n.send(tabs, win32con.WM_MBUTTONUP, 0, packed(point))
        n.wait(lambda: app.documents() == [first], "middle click close")
        n.wait(lambda: app.focused() == tabs, "close keeps tab focus")
        n.key_message(tabs, win32con.VK_RETURN)
        n.wait(lambda: app.focused() == n.get_item(first, 1203), "enter focuses content")

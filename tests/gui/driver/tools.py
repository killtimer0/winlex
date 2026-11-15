"""Open, query and manipulate independent tool windows through user actions."""
import time
import win32con
import win32gui
import uiautomation as auto
from support import deadline, native as n
from support.session import select_combo


def tools(app):
    return [hwnd for hwnd in app.documents() if n.get_item(hwnd, 1816)]


def initialize_command_menu(app, command):
    menu = win32gui.GetMenu(app.frame)
    n.send(app.frame, win32con.WM_INITMENU, menu)
    for index in range(win32gui.GetMenuItemCount(menu)):
        submenu = win32gui.GetSubMenu(menu, index)
        if submenu and any(win32gui.GetMenuItemID(submenu, position) == command
                           for position in range(win32gui.GetMenuItemCount(submenu))):
            n.send(app.frame, win32con.WM_INITMENUPOPUP, submenu, index)
            return
    raise AssertionError('Menu command %s not found' % command)


def activate(doc):
    mdi = win32gui.GetParent(doc)
    n.send(mdi, win32con.WM_MDIACTIVATE, doc)
    n.wait(lambda: n.send(mdi, win32con.WM_MDIGETACTIVE) == doc, "tool document activated")


@deadline.operation('close tool document via UIA')
def close(doc):
    until = time.monotonic() + 5
    assert auto.ControlFromHandle(doc).GetWindowPattern().Close(waitTime=0)
    remaining = until - time.monotonic()
    assert remaining > 0, 'Tool document close exhausted its budget'
    n.wait(lambda: not n.is_window(doc), 'tool document closed', remaining)


@deadline.operation('set tool window state via UIA')
def visual_state(hwnd, state):
    assert auto.ControlFromHandle(hwnd).GetWindowPattern().SetWindowVisualState(state)


def tool(app, new=False):
    panel = app.tool_list
    if not n.is_visible(panel):
        app.command(1800)
        panel = n.wait(lambda: app.tool_list, "tools panel")
    app.select(0, panel)
    before = set(tools(app))
    app.command(1049 if new else 1050)
    return n.wait(lambda: app.active_document() if app.active_document() in tools(app)
                  and (not new or app.active_document() not in before)
                  else None, "active fixed tool")


def choose_scope(app, doc, index):
    combo = n.get_item(doc, 1811)
    select_combo(combo, app.choices(combo)[index])


@deadline.operation('edit tool value via UIA')
def edit(doc, identifier, value):
    until = time.monotonic() + 5
    hwnd = n.get_item(doc, identifier)
    control = auto.ControlFromHandle(hwnd)
    assert control.GetValuePattern().SetValue(value, waitTime=0)
    remaining = until - time.monotonic()
    assert remaining > 0, 'Tool value write exhausted its budget'
    n.wait(lambda: n.window_text(hwnd) == value, 'tool value accepted', remaining)


def body(doc):
    return n.rich_edit_text(n.get_item(doc, 1816))


def query(app, doc, value):
    edit(doc, 1810, value)
    # UIA Invoke may return before the command is dispatched, allowing the old
    # enabled button and body to satisfy the wait. Send through the existing
    # synchronous command entry point so submission has been acknowledged.
    app.command(1815, doc)

    def completed():
        if not win32gui.IsWindowEnabled(n.get_item(doc, 1815)):
            return None
        # Completion enables the button before rendering; finish that UI-thread
        # turn before reading the result. Identical requests need no text change.
        n.send(doc, win32con.WM_NULL)
        return body(doc)

    return n.wait(completed, "tool explicit query", 30)

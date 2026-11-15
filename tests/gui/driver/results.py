"""Select search results and activate captured result-menu actions."""
import struct
from commctrl import LVM_SETITEMSTATE, LVIS_FOCUSED, LVIS_SELECTED
import win32api
import win32gui
import win32con
import win32process
from support import native as n
from driver.menus import wait_closed


def select_result(app, row, control=None):
    view = app.results if control is None else control
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_WRITE,
                                  False, app.process.pid)
    memory = None
    try:
        memory = win32process.VirtualAllocEx(process, 0, 128, win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        item = bytearray(128)
        state = LVIS_FOCUSED | LVIS_SELECTED
        struct.pack_into('<IIIII', item, 0, 0, 0, 0, 0, state)
        buffer = n.c.create_string_buffer(bytes(item))
        assert n.write_process_memory(int(process), memory, buffer, len(item), None)
        n.send(view, LVM_SETITEMSTATE, -1, memory)
        struct.pack_into('<I', item, 12, state)
        buffer = n.c.create_string_buffer(bytes(item))
        assert n.write_process_memory(int(process), memory, buffer, len(item), None)
        assert n.send(view, LVM_SETITEMSTATE, row, memory)
        assert app.selected_row(view) == row
    finally:
        try:
            if memory is not None:
                win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        finally:
            win32api.CloseHandle(process)


def result(app, name):
    app.query(name)
    row = app.first_named_row(name)
    select_result(app, row)
    app.focus(app.results)
    return row


def choose(app, ids, command):
    actions = [ident for ident in ids if ident and ident != 0xffffffff]
    for code in [win32con.VK_DOWN] * (actions.index(command) + 1) + [win32con.VK_RETURN]:
        win32gui.PostMessage(app.results, win32con.WM_KEYDOWN, code, 1)
        win32gui.PostMessage(app.results, win32con.WM_KEYUP, code, -1073741823)
    wait_closed(app.results)
    n.send(app.results, win32con.WM_NULL)

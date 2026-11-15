"""Read native tooltip registrations and exercise visible hover/dismissal."""
import struct
import win32api
import win32con
import win32gui
import win32process
import uiautomation as auto
from support import deadline, messages, native as n
from support.input_state import InputState
from support.diagnostics import assert_unobscured, screenshot


def registered_tip(app, target, bits):
    pointer = '<Q' if bits == 64 else '<I'
    size = 64 if bits == 64 else 44
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ
                                  | win32con.PROCESS_VM_WRITE, False, app.process.pid)
    memory = None
    try:
        memory = win32process.VirtualAllocEx(process, 0, 8192,
                                            win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        info = bytearray(size)
        struct.pack_into('<I', info, 0, size)
        struct.pack_into(pointer, info, 8, target)
        struct.pack_into(pointer, info, 16 if bits == 64 else 12, target)
        struct.pack_into(pointer, info, 48 if bits == 64 else 36, memory + 128)
        info_buffer = n.c.create_string_buffer(bytes(info))
        tooltip = win32gui.FindWindowEx(0, 0, 'tooltips_class32', None)
        while tooltip:
            if win32gui.GetWindow(tooltip, win32con.GW_OWNER) == app.frame:
                if not n.write_process_memory(int(process), memory, info_buffer, len(info), None):
                    raise n.c.WinError(n.c.get_last_error())
                if n.send(tooltip, win32con.WM_USER + 53, 0, memory):
                    if not n.write_process_memory(int(process), memory, info_buffer, len(info), None):
                        raise n.c.WinError(n.c.get_last_error())
                    n.send(tooltip, win32con.WM_USER + 56, 0, memory)
                    data = n.c.create_string_buffer(4096)
                    size_read = n.c.c_size_t()
                    if not n.read_process_memory(int(process), memory + 128, data,
                                                 len(data), n.c.byref(size_read)):
                        raise n.c.WinError(n.c.get_last_error())
                    text = data.raw[:size_read.value].decode(
                        'utf-16-le', 'surrogatepass').split('\0', 1)[0]
                    return tooltip, text
            tooltip = win32gui.FindWindowEx(0, tooltip, 'tooltips_class32', None)
        raise AssertionError('control has no registered tooltip')
    finally:
        try:
            if memory is not None:
                win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        finally:
            win32api.CloseHandle(process)


def infotip(app, control, row, image):
    with deadline.operation('prepare list infotip row'):
        assert control and n.is_visible(control)
        item = app.row(row, control)
        item.GetScrollItemPattern().ScrollIntoView()
        # A desktop UIA search can outlast the tooltip's auto-pop interval even
        # with Exists(0, 0); wait on the list's associated HWND instead.
        tooltip_hwnd = n.send(control, messages.LVM_GETTOOLTIPS)
        assert tooltip_hwnd and n.is_window(tooltip_hwnd), 'list has a native tooltip'
        initial = n.send(tooltip_hwnd, messages.TTM_GETDELAYTIME, 3) / 1000
        autopop = n.send(tooltip_hwnd, messages.TTM_GETDELAYTIME, 2) / 1000
        assert initial > 0 and autopop > 0, 'native tooltip must have finite display delays'
    # Cover initial display and eventual hiding using the control's actual
    # timing and pointer delivery, not a whole-case timeout.
    with InputState(), deadline.operation('list infotip hover and dismissal'):
        auto.ControlFromHandle(app.frame).SetActive()
        assert win32gui.GetForegroundWindow() == app.frame, 'infotip foreground'
        # Neither list content nor the search Edit is a neutral hover target:
        # both can provide their own tooltip while the previous tip is hiding.
        with n.physical_dpi_context():
            left, top, right, bottom = n.titlebar_rectangle(app.frame)
            auto.MoveTo((left + right) // 2, (top + bottom) // 2, waitTime=0)
        n.wait(lambda: not n.is_visible(tooltip_hwnd), 'infotip hidden', autopop + 1)
        bounds = item.BoundingRectangle
        auto.MoveTo(min(bounds.left + 120, bounds.right - 8), bounds.top + bounds.height() // 4, waitTime=0)
        text = n.wait(lambda: n.window_text(tooltip_hwnd) if n.is_visible(tooltip_hwnd) else None,
                      'native infotip content', initial + autopop)
        assert n.is_visible(tooltip_hwnd)
        assert_unobscured(tooltip_hwnd)
        screenshot(tooltip_hwnd, image)
        with n.physical_dpi_context():
            left, top, right, bottom = n.titlebar_rectangle(app.frame)
            auto.MoveTo((left + right) // 2, (top + bottom) // 2, waitTime=0)
        n.wait(lambda: not n.is_visible(tooltip_hwnd), 'infotip hides after pointer leaves', autopop + 1)
        return text

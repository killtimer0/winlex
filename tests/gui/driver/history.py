"""Observe native history commands and deliver replay gestures without IPC gaps."""
import struct
import win32api
import win32con
import win32process
from support import native as n


HISTORY_BACK = 1055
HISTORY_FORWARD = 1056

_callback_type = n.c.WINFUNCTYPE(None, n.w.HWND, n.w.UINT, n.c.c_size_t, n.c.c_ssize_t)
_send_callback = n.api(n.user, 'SendMessageCallbackW', n.w.BOOL, n.w.HWND, n.w.UINT,
                       n.w.WPARAM, n.w.LPARAM, _callback_type, n.c.c_size_t)
# Windows may still call back after a failed delivery wait. Keep those callback
# objects alive until the test process exits; custody owns target termination.
_pending_callbacks = []


def replay_with_modifiers(app, editor, command):
    """Bracket the complete modifier gesture with recipient-side text snapshots."""
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ
                                  | win32con.PROCESS_VM_WRITE, False, app.process.pid)
    memory = None
    completed = {}
    queued = 0

    @_callback_type
    def finished(hwnd, message, index, result):
        completed[index] = result

    _pending_callbacks.append(finished)
    try:
        memory = win32process.VirtualAllocEx(process, 0, 384,
                                            win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        # GETTEXTEX starts with three DWORDs on both x86 and x64; its optional
        # pointer fields are null. Each read uses a separate 128-byte buffer.
        raw = bytearray(384)
        for offset in (0, 64):
            struct.pack_into('<III', raw, offset, 128, 0, 1200)
        data = n.c.create_string_buffer(bytes(raw))
        assert n.write_process_memory(int(process), memory, data, len(raw), None)
        get_text = win32con.WM_USER + 94  # EM_GETTEXTEX: explicit target memory, no marshaling.
        messages = [(app.frame, win32con.WM_COMMAND, command, 0),
                    (editor, get_text, memory, memory + 128)]
        for message, key in ((win32con.WM_SYSKEYDOWN, win32con.VK_MENU),
                             (win32con.WM_KEYDOWN, win32con.VK_CONTROL),
                             (win32con.WM_KEYDOWN, win32con.VK_SHIFT)):
            messages += [(editor, message, key, 1),
                         (editor, win32con.WM_KEYUP, key, n.c.c_int32(0xc0000001).value)]
        messages.append((editor, get_text, memory + 64, memory + 256))
        for index, (hwnd, message, wp, lp) in enumerate(messages):
            assert _send_callback(hwnd, message, wp, lp, finished, index), n.c.WinError(n.c.get_last_error())
            queued += 1

        def received():
            # PeekMessage dispatches callbacks in this sending thread. Do not
            # confuse successful enqueueing with recipient completion.
            n.dispatch_pending_messages()
            return len(completed) == len(messages)
        n.wait(received, 'recipient completion of replay/modifier message burst')
        texts = []
        for index, offset in ((1, 128), (len(messages) - 1, 256)):
            count = completed[index]
            assert 0 <= count < 64, 'Replay text snapshot exceeds its measurement buffer'
            buffer = n.c.create_string_buffer(count * 2)
            size_read = n.c.c_size_t()
            assert n.read_process_memory(int(process), memory + offset, buffer, len(buffer),
                                         n.c.byref(size_read))
            assert size_read.value == len(buffer), 'Incomplete replay text snapshot'
            texts.append(buffer.raw.decode('utf-16-le', 'surrogatepass').strip())
        return texts
    finally:
        try:
            if len(completed) == queued:
                _pending_callbacks.remove(finished)
                if memory is not None:
                    win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
            # Otherwise queued messages still own their target buffers. The
            # session/custody cleanup releases them by terminating the target.
        finally:
            win32api.CloseHandle(process)


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

"""Source-path and startup-dialog observations that preserve native identity."""
import struct
import win32api
import win32con
import win32gui
from support import native as n
import win32process
from support.messages import LVM_GETITEMTEXTW


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
    dialog = n.find_window(app.process.pid, '#32770', app.frame)
    return [dialog] if dialog else []


def assert_no_dialog(app):
    assert win32gui.IsWindowEnabled(app.frame)
    assert not owned_dialogs(app)


def complete_list_text(app, hwnd, row, bits):
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ
                                  | win32con.PROCESS_VM_WRITE, False, app.process.pid)
    memory = None
    capacity = 32768
    try:
        memory = win32process.VirtualAllocEx(process, 0, 128 + capacity * 2,
                                            win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        item = bytearray(128)
        struct.pack_into('<III', item, 0, 1, row, 0)
        struct.pack_into('<Q' if bits == 64 else '<I', item, 24 if bits == 64 else 20, memory + 128)
        struct.pack_into('<I', item, 32 if bits == 64 else 24, capacity)
        item_buffer = n.c.create_string_buffer(bytes(item))
        if not n.write_process_memory(int(process), memory, item_buffer, len(item), None):
            raise n.c.WinError(n.c.get_last_error())
        count = n.send(hwnd, LVM_GETITEMTEXTW, row, memory)
        assert 0 <= count < capacity - 1, 'Native list text exceeds measurement buffer'
        data = n.c.create_string_buffer(count * 2)
        size_read = n.c.c_size_t()
        if not n.read_process_memory(int(process), memory + 128, data, len(data), n.c.byref(size_read)):
            raise n.c.WinError(n.c.get_last_error())
        return data.raw[:size_read.value].decode('utf-16-le', 'surrogatepass')
    finally:
        try:
            if memory is not None:
                win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        finally:
            win32api.CloseHandle(process)

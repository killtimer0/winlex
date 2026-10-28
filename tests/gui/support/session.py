"""Owned GUI process and remote memory; partial-start failures use the same cleanup."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import struct
import subprocess
import win32con

from . import messages
from . import native as n
from .pe import is_64_bit


class Session:
    def __init__(self, exe, profile, arguments=(), dpi="auto"):
        self.process = self.handle = self.memory = self.frame = None
        self.cleanup_complete = False
        self.actual_dpi = None
        try:
            self.process = subprocess.Popen([str(exe), *map(str, arguments)], env=profile.environment(dpi=dpi))
            self.frame = n.wait(self.find_frame, "main window")
            self.search = n.get_item(self.frame, 1102)
            self.results = n.get_item(self.frame, 1103)
            self.sources = n.get_item(self.frame, 1112)
            self.status = n.get_item(self.frame, 1105)
            self.mdi = next(h for h in n.windows(self.frame) if n.class_name(h) == "MDIClient")
            self.handle = n.open_process(
                win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ | win32con.PROCESS_VM_WRITE,
                False, self.process.pid)
            assert self.handle
            self.memory = n.alloc(self.handle, None, 8192,
                                  win32con.MEM_COMMIT | win32con.MEM_RESERVE, win32con.PAGE_READWRITE)
            assert self.memory
            self.is64 = is_64_bit(exe)
            if hasattr(n.user, "GetDpiForWindow"):
                self.actual_dpi = n.api(n.user, "GetDpiForWindow", w.UINT, w.HWND)(self.frame)
        except BaseException:
            self.close()
            raise

    def find_frame(self):
        assert self.process.poll() is None, "GUI exited: %s" % self.process.returncode
        for hwnd in n.windows():
            pid = w.DWORD()
            n.get_pid(hwnd, c.byref(pid))
            if pid.value == self.process.pid and n.class_name(hwnd) == "Winlex.Frame":
                n.send(hwnd, win32con.WM_NULL)
                return hwnd

    def command(self, command, target=None):
        n.send(target or self.frame, win32con.WM_COMMAND, command)

    def remote_text(self, address, size):
        buffer = c.create_string_buffer(size)
        assert n.read_memory(self.handle, address, buffer, size, None)
        return buffer.raw.decode("utf-16-le", errors="surrogatepass").split("\0", 1)[0]

    def status_text(self):
        size = ((n.send(self.status, messages.SB_GETTEXTLENGTHW) & 0xffff) + 1) * 2
        assert size <= 8192
        n.send(self.status, messages.SB_GETTEXTW, 0, self.memory)
        return self.remote_text(self.memory, size)

    def toolbar_text(self, command):
        toolbar = n.get_item(self.frame, 1113)
        count = n.send(toolbar, messages.TB_GETBUTTONTEXTW, command, 0)
        assert 0 <= count < 4096
        assert n.send(toolbar, messages.TB_GETBUTTONTEXTW, command, self.memory) == count
        return self.remote_text(self.memory, (count + 1) * 2)

    def count(self, control=None):
        return n.send(control or self.results, messages.LVM_GETITEMCOUNT)

    def cell(self, row, control=None):
        item = bytearray(8192)
        struct.pack_into("<III", item, 0, 1, row, 0)
        struct.pack_into("<Q" if self.is64 else "<I", item, 24 if self.is64 else 20, self.memory + 128)
        struct.pack_into("<I", item, 32 if self.is64 else 24, 3500)
        raw = bytes(item)
        assert n.write_memory(self.handle, self.memory, raw, len(raw), None)
        n.send(control or self.results, messages.LVM_GETITEMTEXTW, row, self.memory)
        return self.remote_text(self.memory + 128, 7000)

    def select(self, row, control=None):
        control = control or self.results
        for index, state in ((-1, 0), (row, 3)):
            item = struct.pack("<IIIII", 8, max(index, 0), 0, state, 3) + bytes(108)
            assert n.write_memory(self.handle, self.memory, item, len(item), None)
            n.send(control, messages.LVM_SETITEMSTATE, c.c_size_t(index).value, self.memory)

    def item_rect(self, row):
        raw = bytes(16)
        assert n.write_memory(self.handle, self.memory, raw, len(raw), None)
        assert n.send(self.results, messages.LVM_GETITEMRECT, row, self.memory)
        buffer = c.create_string_buffer(16)
        assert n.read_memory(self.handle, self.memory, buffer, 16, None)
        return struct.unpack("<4i", buffer.raw)

    def query(self, value):
        n.set_text(self.search, value)
        n.wait(lambda: self.status_text().startswith(("Results ", "共 ")), "search completed", 45)

    def documents(self):
        # EnumChildWindows can observe an HWND during its WM_CREATE callback.
        return [h for h in n.windows(self.mdi) if n.class_name(h) == "Winlex.Document" and n.get_item(h, 1203)]

    def open_row(self, row=0, source=False):
        control = self.sources if source else self.results
        before = set(self.documents())
        self.select(row, control)
        n.key_message(control, win32con.VK_RETURN)
        return n.wait(lambda: next(iter(set(self.documents()) - before), None), "document opened")

    def content(self, doc):
        control = n.get_item(doc, 1203)
        return n.window_text(control) if control else ""

    def declaration_font(self, doc):
        raw = struct.pack("<I", 92) + bytes(88)  # CHARFORMATW, identical on x86/x64
        assert n.write_memory(self.handle, self.memory, raw, len(raw), None)
        n.send(n.get_item(doc, 1203), messages.EM_GETCHARFORMAT, messages.SCF_SELECTION, self.memory)
        buffer = c.create_string_buffer(92)
        assert n.read_memory(self.handle, self.memory, buffer, 92, None)
        return struct.unpack_from("<i", buffer.raw, 12)[0], buffer.raw[26:90].decode("utf-16-le").split("\0")[0]

    def pick(self, paths, command=1020, target=None):
        n.post(target or self.frame, win32con.WM_COMMAND, command, 0)
        def find():
            for hwnd in n.windows():
                pid = w.DWORD()
                n.get_pid(hwnd, c.byref(pid))
                if pid.value == self.process.pid and n.class_name(hwnd) == "#32770" and n.is_visible(hwnd):
                    return hwnd
        dialog = n.wait(find, "native file dialog")
        if paths:
            field = n.wait(lambda: n.get_item(dialog, 1148) or n.get_item(dialog, 1152), "file-name field")
            n.set_text(field, " ".join('"%s"' % path for path in paths))
        n.post(dialog, win32con.WM_COMMAND, win32con.IDOK if paths else win32con.IDCANCEL, 0)
        n.wait(lambda: not n.is_window(dialog), "file dialog closed")

    def diagnostics(self, destination):
        rows = []
        if self.frame:
            for h in [self.frame, *n.windows(self.frame)]:
                try:
                    rows.append({"hwnd": h, "class": n.class_name(h), "id": n.get_id(h),
                                 "visible": bool(n.is_visible(h)), "rect": n.rectangle(h), "text": n.window_text(h)[:4096]})
                except Exception as error:
                    rows.append({"hwnd": h, "error": str(error)})
        Path(destination).write_text(json.dumps(rows, ensure_ascii=True, indent=2), encoding="utf-8")

    def close(self):
        if self.cleanup_complete:
            return
        if self.process and self.process.poll() is None:
            if self.memory:
                assert n.free(self.handle, self.memory, 0, win32con.MEM_RELEASE)
                self.memory = None
            if self.frame:
                n.post(self.frame, win32con.WM_CLOSE, 0, 0)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
                raise AssertionError("GUI required forced termination")
        if self.handle:
            assert n.close_handle(self.handle)
            self.handle = None
        self.cleanup_complete = True

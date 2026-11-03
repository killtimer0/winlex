"""Owned GUI process and remote memory; partial-start failures use the same cleanup."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import struct
import subprocess
import time
import win32con

from . import messages
from . import native as n
from .pe import is_64_bit


class ThreadInfo(c.Structure):
    """GUI thread focus state; identical to GUITHREADINFO."""

    _fields_ = [("size", w.DWORD), ("flags", w.DWORD),
                ("active", w.HWND), ("focus", w.HWND),
                ("capture", w.HWND), ("menu", w.HWND),
                ("move", w.HWND), ("caret", w.HWND), ("rect", w.RECT)]


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
            if pid.value == self.process.pid and n.class_name(hwnd) == "Winlex.Frame" and n.is_visible(hwnd):
                n.send(hwnd, win32con.WM_NULL)
                return hwnd

    def command(self, command, target=None, code=0):
        n.send(target or self.frame, win32con.WM_COMMAND, (code << 16) | command)

    def settings(self):
        assert n.post(self.frame, win32con.WM_COMMAND, 1051, 0)
        owner = n.api(n.user, "GetWindow", w.HWND, w.HWND, w.UINT)
        sheet = n.wait(lambda: next((h for h in n.windows()
            if n.class_name(h) == "#32770" and owner(h, win32con.GW_OWNER) == self.frame
             and n.is_visible(h) and n.get_item(h, 1)), None), "settings property sheet")
        self.settings_page(sheet, 0)
        return sheet

    def settings_page(self, sheet, index):
        assert n.send(sheet, 0x400 + 101, index, 0)  # PSM_SETCURSEL
        previous = None
        since = time.monotonic()

        def stable():
            nonlocal previous, since
            page = n.send(sheet, 0x400 + 118)  # PSM_GETCURRENTPAGEHWND
            if not page or not n.is_visible(page):
                previous = None
                since = time.monotonic()
                return None
            with n.window_dpi_context(sheet):
                snapshot = tuple((h, n.rectangle(h), n.send(h, win32con.WM_GETFONT))
                                 for h in [sheet, page, *n.windows(page)])
            if snapshot != previous:
                previous = snapshot
                since = time.monotonic()
            return page if time.monotonic() - since >= .2 else None

        return n.wait(stable, "stable settings page", 5)

    def close_settings(self, sheet, accept=True):
        assert n.post(sheet, win32con.WM_COMMAND, 1 if accept else 2, 0)
        n.wait(lambda: not n.is_window(sheet), "settings closed")
        n.send(self.frame, win32con.WM_NULL)

    def choices(self, control):
        count = n.send(control, win32con.CB_GETCOUNT)
        assert 0 <= count < 32
        values = []
        for index in range(count):
            size = n.send(control, win32con.CB_GETLBTEXTLEN, index, 0)
            assert 0 <= size < 4096
            # Combo box messages marshal their text buffer across processes.
            buffer = c.create_unicode_buffer(size + 1)
            assert n.send(control, win32con.CB_GETLBTEXT, index, c.addressof(buffer)) == size
            values.append(buffer.value)
        return values

    def architecture(self, doc):
        return n.wait(lambda: n.get_item(doc, 1047), "architecture selector")

    def choice(self, doc):
        control = self.architecture(doc)
        index = n.send(control, win32con.CB_GETCURSEL)
        assert index >= 0
        return self.choices(control)[index]

    def choose(self, doc, architecture):
        control = self.architecture(doc)
        index = self.choices(control).index(architecture)
        n.send(control, win32con.CB_SETCURSEL, index)
        self.command(1047, doc, win32con.CBN_SELENDOK)
        return index

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

    def named_rows(self, name):
        return [index for index in range(self.count())
                if self.cell(index).splitlines()[0].rsplit(".", 1)[-1] == name]

    def selected_row(self, control=None):
        return c.c_int32(n.send(control or self.results, messages.LVM_GETNEXTITEM,
                               c.c_size_t(-1).value, messages.LVNI_SELECTED)).value

    def item_rect(self, row, control=None):
        raw = bytes(16)
        assert n.write_memory(self.handle, self.memory, raw, len(raw), None)
        assert n.send(control or self.results, messages.LVM_GETITEMRECT, row, self.memory)
        buffer = c.create_string_buffer(16)
        assert n.read_memory(self.handle, self.memory, buffer, 16, None)
        return struct.unpack("<4i", buffer.raw)

    def query(self, value):
        n.set_text(self.search, value)
        n.wait(lambda: self.status_text().startswith(("Results ", "共 ")), "search completed", 45)

    def documents(self):
        # EnumChildWindows can observe an HWND during its WM_CREATE callback.
        return [h for h in n.windows(self.mdi) if n.class_name(h) == "Winlex.Document" and n.get_item(h, 1203)]

    def open_row(self, row=0):
        return self._open_row(row, 1050, "document opened")

    def active_document(self):
        return n.send(self.mdi, win32con.WM_MDIGETACTIVE)

    def open_new(self, row=0):
        return self._open_row(row, 1049, "new document")

    def _open_row(self, row, command, description):
        self.select(row)
        self.command(command)
        return self.wait_document(description)

    def wait_document(self, description, name=None):
        def match():
            doc = self.active_document()
            if doc in self.documents() and (name is None or n.window_text(doc).startswith(name + " -")):
                return doc
        return n.wait(match, description)

    def preview_row(self, row=0):
        self.select(row)
        self.focus(self.results)
        n.key_message(self.results, win32con.VK_SPACE)
        name = self.cell(row).splitlines()[0].rsplit(".", 1)[-1]
        return self.wait_document("preview document", name)

    def single_click_row(self, inputs, row=0):
        return self._clicked_document(inputs, row, False, "preview document")

    def double_click_row(self, inputs, row=0):
        return self._clicked_document(inputs, row, True, "opened document")

    def _clicked_document(self, inputs, row, double, description):
        import win32gui
        assert inputs is not None, "Mouse gestures require an interactive input owner"
        name = self.cell(row).splitlines()[0].rsplit(".", 1)[-1]
        self.select(row)
        with n.window_dpi_context(self.results):
            left, top, right, bottom = self.item_rect(row)
            point = ((left + right) // 2, (top + bottom) // 2)
            position = win32gui.ClientToScreen(self.results, point)
            assert win32gui.GetForegroundWindow() == self.frame, "Application lost foreground"
            assert win32gui.WindowFromPoint(position) == self.results, "Result row is obscured"
        inputs.click(*position)
        if double:
            inputs.click(*position)
        return self.wait_document(description, name)

    def focus(self, control):
        attach = n.api(n.user, "AttachThreadInput", w.BOOL, w.DWORD, w.DWORD, w.BOOL)
        set_focus = n.api(n.user, "SetFocus", w.HWND, w.HWND)
        own = n.api(n.kernel, "GetCurrentThreadId", w.DWORD)()
        target = n.get_pid(self.frame, None)
        assert attach(own, target, True)
        try:
            set_focus(control)
        finally:
            attach(own, target, False)

    def focused(self):
        info = ThreadInfo()
        info.size = c.sizeof(info)
        get = n.api(n.user, "GetGUIThreadInfo", w.BOOL, w.DWORD, c.POINTER(ThreadInfo))
        assert get(n.get_pid(self.frame, None), c.byref(info))
        return info.focus

    def tab_labels(self):
        from .accessibility import client
        tabs = n.get_item(self.frame, 1104)
        with client(tabs) as root, root.children() as pages:
            return [page.name for page in pages]

    def tab_descriptions(self):
        from .accessibility import client
        with client(n.get_item(self.frame, 1104)) as root, root.children() as pages:
            return [page.string(12) for page in pages]

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

    def pick(self, paths):
        n.post(self.frame, win32con.WM_COMMAND, 1020, 0)
        def find():
            for hwnd in n.windows():
                pid = w.DWORD()
                n.get_pid(hwnd, c.byref(pid))
                if pid.value == self.process.pid and n.class_name(hwnd) == "#32770" and n.is_visible(hwnd):
                    return hwnd
        dialog = n.wait(find, "native file dialog")
        field = n.wait(lambda: n.get_item(dialog, 1148) or n.get_item(dialog, 1152), "file-name field")
        n.set_text(field, " ".join('"%s"' % path for path in paths))
        n.post(dialog, win32con.WM_COMMAND, win32con.IDOK, 0)
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

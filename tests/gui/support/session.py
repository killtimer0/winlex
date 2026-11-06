"""Owned GUI process, driven through the Windows UI Automation provider."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import subprocess

from commctrl import LVM_GETITEMCOUNT, LVM_GETNEXTITEM, LVM_GETTOPINDEX, LVNI_SELECTED
import uiautomation as uia
import win32api
import win32con
import win32gui
import win32process

from . import native as n


class ThreadInfo(c.Structure):
    _fields_ = [("size", w.DWORD), ("flags", w.DWORD),
                ("active", w.HWND), ("focus", w.HWND),
                ("capture", w.HWND), ("menu", w.HWND),
                ("move", w.HWND), ("caret", w.HWND), ("rect", w.RECT)]


def control(value):
    return uia.ControlFromHandle(value) if isinstance(value, int) else value


def find(root, identifier):
    item = control(root).Control(AutomationId=str(identifier))
    return item if item.Exists(0, 0) else None


def pattern(item, kind):
    result = item.GetPattern(kind)
    assert result is not None, "UI Automation pattern %s unavailable on %s" % (kind, item)
    return result


def invoke(value):
    item = control(value)
    action = item.GetPattern(uia.PatternId.InvokePattern)
    if action is not None:
        return action.Invoke(waitTime=0)
    return pattern(item, uia.PatternId.LegacyIAccessiblePattern).DoDefaultAction(waitTime=0)


def select_combo(value, label):
    combo = control(value)
    expansion = pattern(combo, uia.PatternId.ExpandCollapsePattern)
    expansion.Expand(waitTime=0)
    try:
        item = combo.ListItemControl(Name=label)
        assert item.Exists(2), "Combo option unavailable: %s" % label
        assert pattern(item, uia.PatternId.LegacyIAccessiblePattern).Select(2, waitTime=0)
        hwnd = combo.NativeWindowHandle
        win32gui.PostMessage(hwnd, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
        win32gui.PostMessage(hwnd, win32con.WM_KEYUP, win32con.VK_RETURN, 0xc0000001)
        n.wait(lambda: expansion.ExpandCollapseState == uia.ExpandCollapseState.Collapsed
               and pattern(combo, uia.PatternId.ValuePattern).Value == label,
               "combo option committed")
    finally:
        if expansion.ExpandCollapseState != uia.ExpandCollapseState.Collapsed:
            expansion.Collapse(waitTime=0)


class Session:
    def __init__(self, exe, profile, arguments=(), dpi="auto"):
        self.process = self.frame = None
        self.cleanup_complete = False
        self.actual_dpi = None
        try:
            self.process = subprocess.Popen([str(exe), *map(str, arguments)], env=profile.environment(dpi=dpi))
            self.frame = n.wait(self.find_frame, "main window")
            self.status = self.item(self.frame, 1105)
            self.mdi = n.wait(lambda: next((x.NativeWindowHandle for x, _ in uia.WalkControl(control(self.frame))
                                            if x.ClassName == "MDIClient"), None), "MDI client")
            if hasattr(n.user, "GetDpiForWindow"):
                self.actual_dpi = n.api(n.user, "GetDpiForWindow", w.UINT, w.HWND)(self.frame)
        except BaseException:
            self.close()
            raise

    def item(self, root, identifier):
        return n.wait(lambda: (find(root, identifier) or None), "control %s" % identifier).NativeWindowHandle

    @property
    def search(self):
        item = win32gui.GetDlgItem(self.frame, 1102)
        return item if n.is_visible(item) else None

    @property
    def results(self):
        item = win32gui.GetDlgItem(self.frame, 1103)
        return item if n.is_visible(item) else None

    @property
    def tool_list(self):
        item = win32gui.GetDlgItem(self.frame, 1801)
        return item if item and n.is_visible(item) else None

    @property
    def sources(self):
        item = win32gui.GetDlgItem(self.frame, 1112)
        return item if n.is_visible(item) else None

    def find_frame(self):
        assert self.process.poll() is None, "GUI exited: %s" % self.process.returncode
        frame = uia.WindowControl(searchDepth=1, ClassName="Winlex.Frame",
                                  Compare=lambda item, depth: item.ProcessId == self.process.pid)
        return frame.NativeWindowHandle if frame.Exists(0, 0) else None

    def command(self, command, target=None, code=0):
        if command in (1049, 1050):
            self.focus(self.tool_list or self.results)
        root = target or self.frame
        if command in (1020, 1051, 1053, 1054):
            win32gui.PostMessage(root, win32con.WM_COMMAND, (code << 16) | command, 0)
        else:
            n.send(root, win32con.WM_COMMAND, (code << 16) | command)

    def settings(self):
        self.command(1051)
        sheet_control = uia.WindowControl(searchDepth=3, ClassName="#32770",
                                          Compare=lambda item, depth: item.ProcessId == self.process.pid)
        sheet = n.wait(lambda: sheet_control.NativeWindowHandle if sheet_control.Exists(0, 0) else None,
                       "settings property sheet")
        self.settings_page(sheet, 0)
        return sheet

    def settings_page(self, sheet, index):
        tabs = [x for x, _ in uia.WalkControl(control(sheet)) if x.ControlType == uia.ControlType.TabItemControl]
        assert 0 <= index < len(tabs)
        selection = pattern(tabs[index], uia.PatternId.SelectionItemPattern)
        if not selection.IsSelected:
            assert n.send(sheet, win32con.WM_USER + 101, index)  # PSM_SETCURSEL
        identifier = (1300, 1310)[index]
        item = n.wait(lambda: find(sheet, identifier), "settings page")
        page = item.GetParentControl()
        return page.NativeWindowHandle

    def close_settings(self, sheet, accept=True):
        invoke(n.wait(lambda: find(sheet, 1 if accept else 2), "settings button"))
        n.wait(lambda: not n.is_window(sheet), "settings closed")

    def choices(self, value):
        combo = control(value)
        if not combo.IsEnabled:
            hwnd = combo.NativeWindowHandle
            count = n.send(hwnd, win32con.CB_GETCOUNT)
            assert count >= 0
            result = []
            for index in range(count):
                length = n.send(hwnd, win32con.CB_GETLBTEXTLEN, index)
                assert length >= 0
                text = c.create_unicode_buffer(length + 1)
                assert n.send(hwnd, win32con.CB_GETLBTEXT, index, c.addressof(text)) == length
                result.append(text.value)
            return result
        expansion = pattern(combo, uia.PatternId.ExpandCollapsePattern)
        expansion.Expand(waitTime=0)
        try:
            return [x.Name for x, _ in uia.WalkControl(combo) if x.ControlType == uia.ControlType.ListItemControl]
        finally:
            expansion.Collapse(waitTime=0)

    def architecture(self, doc):
        return self.item(doc, 1047)

    def choice(self, doc):
        combo = control(self.architecture(doc))
        return pattern(combo, uia.PatternId.ValuePattern).Value

    def choose(self, doc, architecture):
        self.focus(doc)
        n.wait(lambda: self.active_document() == doc, "document activated")
        combo = self.architecture(doc)
        label = c.create_unicode_buffer(architecture)
        index = n.send(combo, win32con.CB_FINDSTRINGEXACT, -1, c.addressof(label))
        assert index != win32con.CB_ERR, "Architecture unavailable: %s" % architecture
        assert n.send(combo, win32con.CB_SETCURSEL, index) == index
        assert n.send(combo, win32con.CB_GETCURSEL) == index
        parent = win32gui.GetParent(combo)
        identifier = win32gui.GetDlgCtrlID(combo)
        n.send(parent, win32con.WM_COMMAND, (win32con.CBN_SELENDOK << 16) | identifier, combo)

    def status_text(self):
        status = control(self.status)
        children = status.GetChildren()
        return children[0].Name if children else status.Name

    def toolbar_text(self, command):
        toolbar = control(self.item(self.frame, 1113))
        return toolbar.GetChildren()[(1040, 1021, 1800).index(command)].Name

    def _rows(self, value=..., reverse=False):
        value = self.results if value is ... else value
        assert value, "Requested list is not exposed by UI Automation"
        root = control(value)
        item = root.GetLastChildControl() if reverse else root.GetFirstChildControl()
        while item:
            if item.ControlType in (uia.ControlType.ListItemControl, uia.ControlType.DataItemControl):
                yield item
            item = item.GetPreviousSiblingControl() if reverse else item.GetNextSiblingControl()

    def rows(self, value=...):
        return list(self._rows(value))

    def row(self, row, value=...):
        total = self.count(value)
        if row < 0:
            row += total
        if not 0 <= row < total:
            raise IndexError("List row unavailable: %s" % row)
        reverse = row >= total // 2
        offset = total - 1 - row if reverse else row
        for index, item in enumerate(self._rows(value, reverse=reverse)):
            if index == offset:
                return item
        raise IndexError("List row unavailable: %s" % row)

    def count(self, value=...):
        value = self.results if value is ... else value
        assert value, "Requested list is unavailable"
        assert n.class_name(value) == "SysListView32", "Requested control is not a list view"
        return n.send(value, LVM_GETITEMCOUNT)

    def cell(self, row, control=...):
        return self.row(row, control).Name

    def select(self, row, control=...):
        assert pattern(self.row(row, control), uia.PatternId.SelectionItemPattern).Select(waitTime=0)

    def named_rows(self, name):
        return [index for index, item in enumerate(self.rows())
                if item.Name.splitlines()[0].rsplit(".", 1)[-1] == name]

    def selected_row(self, control=...):
        value = self.results if control is ... else control
        assert value, "Requested list is unavailable"
        assert n.class_name(value) == "SysListView32", "Requested control is not a list view"
        return c.c_int32(n.send(value, LVM_GETNEXTITEM, -1, LVNI_SELECTED)).value

    def top_row(self):
        return n.send(self.results, LVM_GETTOPINDEX)

    def item_rect(self, row, control=...):
        rect = self.row(row, control).BoundingRectangle
        origin = win32gui.ClientToScreen(self.results if control is ... else control, (0, 0))
        return rect.left - origin[0], rect.top - origin[1], rect.right - origin[0], rect.bottom - origin[1]

    def query(self, value):
        if not self.search or control(self.search).IsOffscreen:
            self.command(1040)
        pattern(control(self.search), uia.PatternId.ValuePattern).SetValue(value)
        n.wait(lambda: self.status_text().startswith(("Results ", "共 ")), "search completed", 45)

    def documents(self):
        return [x.NativeWindowHandle for x, _ in uia.WalkControl(control(self.mdi))
                if x.ClassName == "Winlex.Document"]

    def active_document(self):
        # Duplicate documents share window titles; tab labels cannot identify their HWNDs.
        return n.send(self.mdi, win32con.WM_MDIGETACTIVE) or None

    def open_row(self, row=0):
        return self._open_row(row, 1050, "document opened")

    def open_new(self, row=0):
        return self._open_row(row, 1049, "new document")

    def _open_row(self, row, command, description):
        name = self.cell(row).splitlines()[0].rsplit(".", 1)[-1]
        before = set(self.documents())
        self.select(row)
        self.focus(self.results)
        self.command(command)
        doc = self.wait_document(description, name)
        if command == 1049:
            doc = n.wait(lambda: self.active_document() not in before and self.active_document(), description)
        return doc

    def wait_document(self, description, name=None):
        def match():
            doc = self.active_document()
            if doc and (name is None or control(doc).Name.startswith(name + " -")):
                return doc
        return n.wait(match, description)

    def preview_row(self, row=0, inputs=None):
        self.select(row)
        name = self.cell(row).splitlines()[0].rsplit(".", 1)[-1]
        self.focus(self.results)
        if inputs:
            assert win32gui.GetForegroundWindow() == self.frame, "Application lost foreground"
            inputs.key(win32con.VK_SPACE)
        else:
            win32gui.PostMessage(self.results, win32con.WM_KEYDOWN, win32con.VK_SPACE, 1)
            win32gui.PostMessage(self.results, win32con.WM_KEYUP, win32con.VK_SPACE, 0xc0000001)
        return self.wait_document("preview document", name)

    def single_click_row(self, inputs, row=0):
        return self._clicked_document(inputs, row, False, "preview document")

    def double_click_row(self, inputs, row=0):
        return self._clicked_document(inputs, row, True, "opened document")

    def _clicked_document(self, inputs, row, double, description):
        assert inputs is not None, "Mouse gestures require an interactive input owner"
        item = self.row(row)
        name = item.Name.splitlines()[0].rsplit(".", 1)[-1]
        rect = item.BoundingRectangle
        position = ((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
        assert win32gui.GetForegroundWindow() == self.frame, "Application lost foreground"
        assert win32gui.WindowFromPoint(position) == self.results, "Result row is obscured"
        if double:
            inputs.double_click(*position)
        else:
            item.Click()
        return self.wait_document(description, name)

    def focus(self, value):
        hwnd = value if isinstance(value, int) else control(value).NativeWindowHandle
        assert hwnd, "Focus target has no native window"
        own = win32api.GetCurrentThreadId()
        target = win32process.GetWindowThreadProcessId(hwnd)[0]
        win32process.AttachThreadInput(own, target, True)
        try:
            win32gui.SetFocus(hwnd)
        finally:
            win32process.AttachThreadInput(own, target, False)

    def focused(self):
        info = ThreadInfo()
        info.size = c.sizeof(info)
        assert n.api(n.user, "GetGUIThreadInfo", w.BOOL, w.DWORD, c.POINTER(ThreadInfo))(
            n.get_pid(self.frame, None), c.byref(info))
        return info.focus or None

    def _tabs(self):
        return [x for x, _ in uia.WalkControl(control(self.item(self.frame, 1104)))
                if x.ControlType == uia.ControlType.TabItemControl]

    def tab_labels(self):
        return [x.Name for x in self._tabs()]

    def tab_descriptions(self):
        return [pattern(x, uia.PatternId.LegacyIAccessiblePattern).Description for x in self._tabs()]

    def content(self, doc):
        editor = find(doc, 1203)
        if not editor:
            return ""
        return n.window_text(editor.NativeWindowHandle)

    def declaration_font(self, doc):
        font = n.rich_edit_document(self.item(doc, 1203)).Range(0, 0).Font
        return round(font.Size * 20), font.Name

    def pick(self, paths):
        self.command(1020)
        def dialogs():
            found = []
            def visit(hwnd, _):
                if (win32process.GetWindowThreadProcessId(hwnd)[1] == self.process.pid
                        and n.is_visible(hwnd) and n.class_name(hwnd) == "#32770"):
                    found.append(hwnd)
                return True
            win32gui.EnumWindows(visit, None)
            return found
        candidates = n.wait(dialogs, "native file dialog")
        assert len(candidates) == 1, ("ambiguous file dialog", candidates)
        dialog = candidates[0]

        def controls():
            fields, buttons = [], []
            def visit(hwnd, _):
                if n.is_visible(hwnd):
                    name, identifier = n.class_name(hwnd), n.get_id(hwnd)
                    if name == "Edit" and identifier in (1148, 1152):
                        fields.append(hwnd)
                    elif name == "Button" and identifier == win32con.IDOK:
                        buttons.append(hwnd)
                return True
            win32gui.EnumChildWindows(dialog, visit, None)
            return (fields, buttons) if fields and buttons else None
        fields, buttons = n.wait(controls, "file dialog controls")
        assert len(fields) == len(buttons) == 1, ("ambiguous file dialog controls", fields, buttons)
        value = c.create_unicode_buffer(" ".join('"%s"' % path for path in paths))
        assert n.send(fields[0], win32con.WM_SETTEXT, 0, c.addressof(value))
        win32gui.PostMessage(dialog, win32con.WM_COMMAND, win32con.IDOK, buttons[0])
        n.wait(lambda: not n.is_window(dialog), "file dialog closed")

    def diagnostics(self, destination):
        rows = []
        if self.frame and n.is_window(self.frame):
            for item, _ in uia.WalkControl(control(self.frame), includeTop=True):
                try:
                    rect = item.BoundingRectangle
                    rows.append({"hwnd": item.NativeWindowHandle, "class": item.ClassName,
                                 "type": item.ControlTypeName,
                                 "id": item.AutomationId, "visible": not item.IsOffscreen,
                                 "rect": [rect.left, rect.top, rect.right, rect.bottom], "text": item.Name[:4096]})
                except Exception as error:
                    rows.append({"error": str(error)})
        Path(destination).write_text(json.dumps(rows, ensure_ascii=True, indent=2), encoding="utf-8")

    def close(self):
        if self.cleanup_complete:
            return
        if self.process and self.process.poll() is None:
            try:
                if self.frame and n.is_window(self.frame):
                    dialogs = []
                    owners = [control(self.frame)]
                    while owners:
                        for item in owners.pop().GetChildren():
                            if (item.ClassName == '#32770'
                                    and not win32gui.GetWindowLong(item.NativeWindowHandle, win32con.GWL_STYLE) & win32con.WS_CHILD):
                                dialogs.append(item)
                                owners.append(item)
                    for dialog in reversed(dialogs):
                        handle = dialog.NativeWindowHandle
                        button = dialog.ButtonControl(AutomationId='7')
                        if not button.Exists(0, 0):
                            button = dialog.ButtonControl(AutomationId='2')
                        if button.Exists(0, 0):
                            invoke(button)
                        else:
                            pattern(dialog, uia.PatternId.WindowPattern).Close(waitTime=0)
                        n.wait(lambda: not n.is_window(handle), 'owned dialog closed')
                    pattern(control(self.frame), uia.PatternId.WindowPattern).Close(waitTime=0)
            finally:
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
                    raise AssertionError("GUI required forced termination")
        self.cleanup_complete = True

"""Native picker and prompt modality, accessibility and displayed contrast."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import time
import uuid
import uiautomation as auto
import win32con
import win32gui
import win32process
from comtypes import COMError
from support import deadline, native as n
from support.session import invoke, message_button
from driver.surfaces import observed_colors as colors, text_contrast


def native_dialog(app):
    found = []

    def visit(hwnd, _):
        if (win32process.GetWindowThreadProcessId(hwnd)[1] == app.process.pid
                and n.is_visible(hwnd) and n.class_name(hwnd) == '#32770'
                and win32gui.GetWindow(hwnd, win32con.GW_OWNER) == app.frame):
            found.append(hwnd)
        return True

    for hwnd in n.top_level_windows():
        visit(hwnd, None)
    assert len(found) <= 1, ('ambiguous owned native dialog', found)
    return found[0] if found else None


def assert_modal(app, dialog):
    assert win32gui.GetWindow(dialog, win32con.GW_OWNER) == app.frame
    assert not win32gui.IsWindowEnabled(app.frame), 'native dialog must disable its owner'


def dismiss_no(app, dialog):
    invoke(n.wait(lambda: message_button(dialog, win32con.IDNO), 'native No button'))
    n.wait(lambda: not n.is_window(dialog) and win32gui.IsWindowEnabled(app.frame),
           'No dismisses prompt and restores owner')
    assert not native_dialog(app), 'No must not open another guide dialog'
    assert app.process.poll() is None


@deadline.operation('locate native dialog text surfaces via UIA')
def native_text_surfaces(dialog):
    """Locate displayed text and list-row text without sampling icons or borders."""
    root = auto.ControlFromHandle(dialog)
    with n.window_dpi_context(dialog):
        _, _, width, height = n.rectangle(dialog, client=True)
        origin = win32gui.ClientToScreen(dialog, (0, 0))
    try:
        convert = n.api(n.user, 'PhysicalToLogicalPointForPerMonitorDPI', n.w.BOOL,
                        n.w.HWND, n.c.POINTER(n.w.POINT))
    except AttributeError:
        convert = None

    def client_rect(bounds):
        # UIA reports physical screen pixels; capture_pixels crops in the
        # target's coordinate space.
        if bounds.right <= bounds.left or bounds.bottom <= bounds.top:
            # Named elements can still report an empty rectangle; it cannot be
            # converted or sampled, and every caller rejects it through inside().
            return (0, 0, 0, 0)
        first = n.w.POINT(bounds.left, bounds.top)
        last = n.w.POINT(bounds.right, bounds.bottom)
        if convert is not None:
            assert convert(dialog, n.c.byref(first)), 'Cannot convert native text bounds'
            assert convert(dialog, n.c.byref(last)), 'Cannot convert native text bounds'
        return (first.x - origin[0], first.y - origin[1],
                last.x - origin[0], last.y - origin[1])

    def inside(rect):
        left, top, right, bottom = rect
        return (left >= 0 and top >= 0 and right <= width and bottom <= height
                and left + 2 < right - 2 and top + 2 < bottom - 2)

    def contained(outer, inner):
        return (outer[0] - 1 <= inner[0] and outer[1] - 1 <= inner[1]
                and inner[2] <= outer[2] + 1 and inner[3] <= outer[3] + 1)

    def text_ranges(control):
        """Text-provider bounds are real glyph geometry; empty means unavailable."""
        try:
            pattern = control.GetPattern(auto.PatternId.TextPattern)
            if pattern is None:
                return []
            return [client_rect(bounds)
                    for visible in pattern.GetVisibleRanges()
                    for bounds in visible.GetBoundingRectangles()]
        except COMError:
            return []

    def row_text_rects(item):
        """Real text bounds inside one row, never the row minus guessed padding."""
        row = client_rect(item.BoundingRectangle)
        cells = []

        def collect(control, depth=0):
            if depth > 4:
                return
            for child in control.GetChildren():
                try:
                    kind = child.ControlType
                    if kind == auto.ControlType.TitleBarControl:
                        continue
                    rect = client_rect(child.BoundingRectangle)
                except COMError:
                    continue
                if (kind in (auto.ControlType.TextControl, auto.ControlType.EditControl)
                        and inside(rect) and contained(row, rect)):
                    cells.append(rect)
                elif depth < 4:
                    collect(child, depth + 1)

        collect(item)
        if not cells:
            # Some providers expose row text only through TextPattern ranges.
            ranges = text_ranges(item)
            if not ranges:
                parent = item.GetParentControl()
                if parent and parent.ControlType in (auto.ControlType.ListControl,
                                                     auto.ControlType.DataGridControl):
                    ranges = [rect for rect in text_ranges(parent)
                              if contained(row, rect)]
            cells = [rect for rect in ranges if inside(rect)]
        return cells[:2]

    texts, rows = [], []

    def visit(control, depth=0):
        if depth > 12 or (len(texts) >= 3 and len(rows) >= 3):
            return
        for child in control.GetChildren():
            if len(texts) >= 3 and len(rows) >= 3:
                return
            try:
                kind = child.ControlType
                # Non-client text is outside the captured client area.
                if kind == auto.ControlType.TitleBarControl:
                    continue
                name, offscreen = child.Name, child.IsOffscreen
                if not offscreen and name.strip():
                    rect = client_rect(child.BoundingRectangle)
                    if kind == auto.ControlType.TextControl and len(texts) < 3 and inside(rect):
                        left, top, right, bottom = rect
                        texts.append(('text', name,
                                      (left + 2, top + 2, right - 2, bottom - 2)))
                # Sample the node itself, but always keep traversing unnamed
                # Pane/Group containers: their children still carry the text.
                if (not offscreen and len(rows) < 3
                        and kind in (auto.ControlType.ListItemControl,
                                     auto.ControlType.DataItemControl)):
                    for rect in row_text_rects(child):
                        if len(rows) >= 3:
                            break
                        left, top, right, bottom = rect
                        rows.append(('row', name,
                                     (left + 1, top + 1, right - 1, bottom - 1)))
            except COMError:
                pass
            visit(child, depth + 1)

    visit(root)
    return texts + rows


def assert_native_text_contrast(dialog):
    surfaces = n.wait(lambda: native_text_surfaces(dialog),
                      'native dialog text surfaces', 15)

    def readable():
        return all(text_contrast(colors(dialog, rect)) for _kind, _name, rect in surfaces)

    n.wait(readable, 'native client text must remain readable', 15)
    return surfaces


@deadline.operation('read current native TaskDialog theme')
def native_task_dialog_primary_fill(dialog):
    """Read the public native TaskDialog primary-panel color as BGR."""
    # UxTheme caches caller-process state. Opening a new HTHEME is insufficient
    # while real theme-change notifications remain queued on this STA thread.
    # Dispatch pending messages only; never synthesize a theme notification or
    # treat an empty queue as completion of the system's theme transition.
    n.dispatch_pending_messages()
    ux = n.c.WinDLL('uxtheme')
    theme = n.api(ux, 'OpenThemeData', n.c.c_void_p, n.w.HWND, n.w.LPCWSTR)(
        dialog, 'TaskDialog')
    assert theme, 'native TaskDialog theme unavailable'
    try:
        color = n.w.DWORD()
        # vssym32.h: TDLG_PRIMARYPANEL = 1; TMT_FILLCOLOR = 3802.
        status = n.api(ux, 'GetThemeColor', n.c.c_long, n.c.c_void_p,
                       n.c.c_int, n.c.c_int, n.c.c_int, n.c.POINTER(n.w.DWORD))(
                           theme, 1, 0, 3802, n.c.byref(color))
        assert status >= 0, ('native TaskDialog primary fill unavailable', status)
        # COLORREF is 0x00BBGGRR; captured pixel tuples are BGR.
        return (color.value >> 16 & 255, color.value >> 8 & 255, color.value & 255)
    finally:
        n.api(ux, 'CloseThemeData', n.c.c_long, n.c.c_void_p)(theme)


def filename_field(dialog):
    fields = []

    def visit(hwnd, _):
        if n.is_visible(hwnd) and n.class_name(hwnd) == 'Edit' and n.get_id(hwnd) in (1148, 1152):
            fields.append(hwnd)
        return True

    win32gui.EnumChildWindows(dialog, visit, None)
    assert len(fields) <= 1, ('ambiguous native filename field', fields)
    return fields[0] if fields else None


@contextmanager
def picker_uia_calls():
    """Observe swallowed REGDB_E_IIDNOTREG on this thread; never handle it."""
    class ExceptionRecord(n.c.Structure):
        _fields_ = [('code', n.w.DWORD), ('flags', n.w.DWORD),
                    ('record', n.c.c_void_p), ('address', n.c.c_void_p)]

    class ExceptionPointers(n.c.Structure):
        _fields_ = [('record', n.c.POINTER(ExceptionRecord)), ('context', n.c.c_void_p)]

    callback_type = n.c.WINFUNCTYPE(n.c.c_long, n.c.POINTER(ExceptionPointers))
    get_thread = n.api(n.kernel, 'GetCurrentThreadId', n.w.DWORD)
    thread = get_thread()
    first = []

    @callback_type
    def observe(pointers):
        record = pointers.contents.record.contents
        if record.code == 0x80040155 and get_thread() == thread and not first:
            first.append(record.address)
        return 0  # EXCEPTION_CONTINUE_SEARCH: preserve handlers/faulthandler.

    add = n.api(n.kernel, 'AddVectoredExceptionHandler', n.c.c_void_p,
                n.w.ULONG, callback_type)
    remove = n.api(n.kernel, 'RemoveVectoredExceptionHandler', n.w.ULONG, n.c.c_void_p)
    token = add(1, observe)
    assert token, 'Cannot observe native picker traversal exceptions'

    def call(operation, *args):
        started = time.monotonic()
        try:
            return operation(*args)
        finally:
            if first:
                evidence = {'code': '0x80040155', 'address': first[0], 'thread': thread,
                            'call_seconds': time.monotonic() - started,
                            'operation': getattr(operation, '__name__', type(operation).__name__)}
                ledger = os.environ.get('WINLEX_TEST_LEDGER')
                if ledger:
                    path = Path(ledger).parent / ('picker-native-exception-' + uuid.uuid4().hex + '.json')
                    path.write_text(json.dumps(evidence, indent=2), encoding='utf-8')
                    evidence['artifact'] = str(path)
                raise AssertionError(('First native exception during picker UIA traversal', evidence))

    try:
        yield call
    finally:
        assert remove(token), 'Cannot remove picker exception observer'


@deadline.operation('locate native folder entry via UIA')
def picker_entry(dialog, sentinel):
    # Exists/FindControl can traverse past native exceptions swallowed below
    # Python. Check each provider call instead of retrying that whole search.
    with picker_uia_calls() as call:
        root = call(auto.ControlFromHandle, dialog)

        def find(parent, depth, in_list=False):
            child = call(parent.GetFirstChildControl)
            while child:
                kind = call(lambda: child.ControlType)
                # Non-client titlebars cannot contain folder entries. In
                # particular, do not ask their provider for children or names.
                if kind != auto.ControlType.TitleBarControl:
                    if (in_list and kind in (auto.ControlType.ListItemControl,
                                             auto.ControlType.DataItemControl)
                            and call(lambda: child.Name) in (sentinel.name, sentinel.stem)):
                        return child
                    if depth < 12:
                        found = find(child, depth + 1, in_list or kind in (
                            auto.ControlType.ListControl, auto.ControlType.DataGridControl))
                        if found:
                            return found
                child = call(child.GetNextSiblingControl)
            return None

        return find(root, 1)


@deadline.operation('measure native file picker surfaces via UIA')
def file_picker_colors(dialog, field, entry):
    """Measure native filename and folder surfaces without fixing an exact palette."""
    origin = win32gui.ClientToScreen(dialog, (0, 0))
    _, _, width, height = n.rectangle(dialog, client=True)
    view = entry.GetParentControl()
    for _ in range(10):
        if not view or view.ControlType in (auto.ControlType.ListControl,
                                            auto.ControlType.DataGridControl):
            break
        view = view.GetParentControl()
    surfaces = {'filename': auto.ControlFromHandle(field).BoundingRectangle}
    if view and view.ControlType in (auto.ControlType.ListControl, auto.ControlType.DataGridControl):
        surfaces['contents'] = view.BoundingRectangle
    navigation = auto.ControlFromHandle(dialog).TreeControl(searchDepth=12)
    if navigation.Exists(0, 0):
        surfaces['navigation'] = navigation.BoundingRectangle
    for identifier in (win32con.IDOK, win32con.IDCANCEL):
        button = n.get_item(dialog, identifier)
        assert button, ('native file command unavailable', identifier)
        surfaces['command_%s' % identifier] = auto.ControlFromHandle(button).BoundingRectangle

    def combo_surface(hwnd, _):
        if n.is_visible(hwnd) and n.class_name(hwnd) == 'ComboBox':
            surfaces['combo_%s' % len(surfaces)] = auto.ControlFromHandle(hwnd).BoundingRectangle
        return True

    win32gui.EnumChildWindows(dialog, combo_surface, None)
    result = {}
    for name, bounds in surfaces.items():
        rect = (max(3, bounds.left - origin[0] + 3),
                max(3, bounds.top - origin[1] + 3),
                min(width - 3, bounds.right - origin[0] - 3),
                min(height - 3, bounds.bottom - origin[1] - 3))
        assert rect[0] < rect[2] and rect[1] < rect[3], ('native surface not visible', name, rect)
        palette = colors(dialog, rect)
        result[name] = {'rect': rect, 'colors_bgr': palette.most_common(8)}
    rect = (3, height - 8, width - 3, height - 3)
    palette = colors(dialog, rect)
    result['command_pane'] = {
        'rect': rect, 'colors_bgr': palette.most_common(8)}
    return result

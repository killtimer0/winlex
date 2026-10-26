"""Native GUI shell integration checks on an interactive Windows desktop.

Only processes started by this invocation are closed. Cross-DPI checks require
two displays with different scaling; no display settings are changed.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import subprocess
import sys
import time
import uuid

user = c.WinDLL("user32", use_last_error=True)
gdi = c.WinDLL("gdi32", use_last_error=True)
callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)


def api(dll, name, result, *arguments):
    fn = getattr(dll, name)
    fn.restype, fn.argtypes = result, arguments
    return fn


enum_windows = api(user, "EnumWindows", w.BOOL, callback, w.LPARAM)
enum_children = api(user, "EnumChildWindows", w.BOOL, w.HWND, callback, w.LPARAM)
get_pid = api(user, "GetWindowThreadProcessId", w.DWORD, w.HWND, c.POINTER(w.DWORD))
get_parent = api(user, "GetParent", w.HWND, w.HWND)
get_class = api(user, "GetClassNameW", c.c_int, w.HWND, w.LPWSTR, c.c_int)
get_rect = api(user, "GetWindowRect", w.BOOL, w.HWND, c.POINTER(w.RECT))
get_client = api(user, "GetClientRect", w.BOOL, w.HWND, c.POINTER(w.RECT))
client_screen = api(user, "ClientToScreen", w.BOOL, w.HWND, c.POINTER(w.POINT))
send_timeout = api(user, "SendMessageTimeoutW", c.c_ssize_t, w.HWND, w.UINT, c.c_size_t, c.c_ssize_t, w.UINT, w.UINT, c.POINTER(c.c_size_t))
post = api(user, "PostMessageW", w.BOOL, w.HWND, w.UINT, c.c_size_t, c.c_ssize_t)
set_pos = api(user, "SetWindowPos", w.BOOL, w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT)
get_dpi = api(user, "GetDpiForWindow", w.UINT, w.HWND)
get_id = api(user, "GetDlgCtrlID", c.c_int, w.HWND)
is_zoomed = api(user, "IsZoomed", w.BOOL, w.HWND)
is_window = api(user, "IsWindow", w.BOOL, w.HWND)
is_visible = api(user, "IsWindowVisible", w.BOOL, w.HWND)
get_context = api(user, "GetWindowDpiAwarenessContext", c.c_void_p, w.HWND)
set_context = api(user, "SetThreadDpiAwarenessContext", c.c_void_p, c.c_void_p)
equal_context = api(user, "AreDpiAwarenessContextsEqual", w.BOOL, c.c_void_p, c.c_void_p)
get_object = api(gdi, "GetObjectW", c.c_int, c.c_void_p, c.c_int, c.c_void_p)
get_foreground = api(user, "GetForegroundWindow", w.HWND)
get_menu = api(user, "GetMenu", w.HMENU, w.HWND)
get_submenu = api(user, "GetSubMenu", w.HMENU, w.HMENU, c.c_int)
get_menu_count = api(user, "GetMenuItemCount", c.c_int, w.HMENU)
get_menu_string = api(user, "GetMenuStringW", c.c_int, w.HMENU, w.UINT, w.LPWSTR, c.c_int, w.UINT)


class GuiThreadInfo(c.Structure):
    _fields_ = [("size", w.DWORD), ("flags", w.DWORD), ("active", w.HWND), ("focus", w.HWND),
                ("capture", w.HWND), ("menu", w.HWND), ("move", w.HWND), ("caret", w.HWND), ("rect", w.RECT)]


get_thread_info = api(user, "GetGUIThreadInfo", w.BOOL, w.DWORD, c.POINTER(GuiThreadInfo))


def send(hwnd, message, wp, lp):
    result = c.c_size_t()
    assert send_timeout(hwnd, message, wp, lp, 3, 3000, c.byref(result)), f"Message timeout: {message:#x}"
    return c.c_ssize_t(result.value).value


def focus_of(hwnd):
    info = GuiThreadInfo()
    info.size = c.sizeof(info)
    assert get_thread_info(get_pid(hwnd, None), c.byref(info))
    return info.focus


def window_text(hwnd):
    buffer = c.create_unicode_buffer(send(hwnd, 0xE, 0, 0) + 1)
    send(hwnd, 0xD, len(buffer), c.addressof(buffer))
    return buffer.value


def key(hwnd, code):
    assert post(hwnd, 0x100, code, 1)
    assert post(hwnd, 0x101, code, 0xC0000001)


def windows(parent=None):
    result = []
    fn = callback(lambda hwnd, _: result.append(hwnd) or True)
    if parent:
        enum_children(parent, fn, 0)
    else:
        enum_windows(fn, 0)
    return result


def class_name(hwnd):
    buffer = c.create_unicode_buffer(256)
    get_class(hwnd, buffer, len(buffer))
    return buffer.value


def rectangle(hwnd, client=False):
    r = w.RECT()
    assert (get_client if client else get_rect)(hwnd, c.byref(r))
    return r.left, r.top, r.right, r.bottom


def origin(hwnd):
    p = w.POINT()
    assert client_screen(hwnd, c.byref(p))
    return p.x, p.y


def wait(probe, description, timeout=8):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        value = probe()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"Timeout: {description}")


def stable(probe):
    previous, count = None, 0

    def settled():
        nonlocal previous, count
        value = probe()
        count = count + 1 if value == previous else 0
        previous = value
        return value if count >= 3 else None

    return wait(settled, "stable geometry")


def monitors():
    values = []
    cb_type = c.WINFUNCTYPE(w.BOOL, c.c_void_p, c.c_void_p, c.POINTER(w.RECT), w.LPARAM)

    def visit(_monitor, _dc, r, _data):
        values.append((r.contents.left, r.contents.top, r.contents.right, r.contents.bottom))
        return True

    enum = api(user, "EnumDisplayMonitors", w.BOOL, c.c_void_p, c.c_void_p, cb_type, w.LPARAM)
    enum(None, None, cb_type(visit), 0)
    return values


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--dpi", choices=["auto", "pmv1", "pmv2", "system", "unaware"], default="pmv2")
    parser.add_argument("--lang", choices=["auto", "en-US", "zh-CN"], default="en-US")
    parser.add_argument("--artifacts", type=Path, default=Path("tmp/gui-check"))
    parser.add_argument("--desktop", choices=["isolated", "current"], default="isolated")
    parser.add_argument("--on-test-desktop", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.desktop == "isolated" and not args.on_test_desktop:
        create = api(user, "CreateDesktopW", c.c_void_p, w.LPCWSTR, c.c_void_p, c.c_void_p, w.DWORD, w.DWORD, c.c_void_p)
        close = api(user, "CloseDesktop", w.BOOL, c.c_void_p)
        name = "Winlex.Test." + uuid.uuid4().hex
        desktop = create(name, None, None, 0, 0xF01FF, None)
        if not desktop:
            raise c.WinError(c.get_last_error())
        try:
            startup = subprocess.STARTUPINFO()
            startup.lpDesktop = name
            result = subprocess.run([sys.executable, "-X", "utf8", str(Path(__file__).resolve()), *sys.argv[1:], "--on-test-desktop"],
                                    startupinfo=startup, creationflags=subprocess.CREATE_NO_WINDOW, timeout=180,
                                    capture_output=True, encoding="utf-8")
            print(result.stdout, end="")
            print(result.stderr, end="", file=sys.stderr)
            raise SystemExit(result.returncode)
        finally:
            close(desktop)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    set_context(c.c_void_p(-4))
    displays = monitors()
    samples = []
    focus_events = []
    font_heights = {}
    process = subprocess.Popen([str(args.exe.resolve()), "--dpi", args.dpi, "--lang", args.lang])
    frame = None
    try:
        def find_frame():
            if process.poll() is not None:
                raise AssertionError(f"GUI exited: {process.returncode}")
            for hwnd in windows():
                pid = w.DWORD()
                get_pid(hwnd, c.byref(pid))
                if pid.value == process.pid and class_name(hwnd) == "Winlex.Frame":
                    return hwnd
        frame = wait(find_frame, "main window")
        if args.lang != "auto":
            chinese = args.lang == "zh-CN"
            assert ("图形界面骨架" if chinese else "GUI shell") in window_text(frame)
            button = next(h for h in windows(frame) if get_id(h) == 1101)
            assert window_text(button) == ("打开页面(&O)" if chinese else "&Open page")
            menu_text = c.create_unicode_buffer(128)
            menu = get_menu(frame)
            for index in range(get_menu_count(menu)):
                if get_menu_string(get_submenu(menu, index), 1001, menu_text, len(menu_text), 0):
                    break
            assert ("新建" if chinese else "New") in menu_text.value, "localized native menu"
        context = get_context(frame)
        expected = {"pmv1": -3, "pmv2": -4, "system": -2, "unaware": -1}.get(args.dpi)
        if expected:
            assert equal_context(context, c.c_void_p(expected)), "actual awareness"
        # Query geometry in the application's coordinate space.
        set_context(context)
        mdi = wait(lambda: next((h for h in windows(frame) if class_name(h) == "MDIClient"), None), "MDICLIENT")

        def docs():
            return sorted(h for h in windows(mdi) if class_name(h) == "Winlex.Document")

        wait(lambda: len(docs()) == 2, "initial documents")
        document_tabs = next(h for h in windows(frame) if get_id(h) == 1104)
        assert not any(get_parent(h) == frame and get_id(h) == 1003 for h in windows(frame)), "obsolete Close button"

        def middle_close_first():
            value = get_dpi(document_tabs)
            point = (10 * value // 96 << 16) | (20 * value // 96)
            send(document_tabs, 0x207, 0x10, point)  # WM_MBUTTONDOWN
            send(document_tabs, 0x208, 0, point)  # WM_MBUTTONUP
        tab_documents = []
        for index in range(2):
            send(document_tabs, 0x1330, index, 0)  # TCM_SETCURFOCUS sends real selection notifications.
            wait(lambda: send(document_tabs, 0x130B, 0, 0) == index, "document Tab selected")
            tab_documents.append(send(mdi, 0x229, 0, 0))
        assert len(set(tab_documents)) == 2 and set(tab_documents) == set(docs()), "Tab activation mapping"
        send(frame, 0x111, get_id(tab_documents[0]), 0)  # Native Window menu command.
        wait(lambda: send(document_tabs, 0x130B, 0, 0) == 0, "Window menu synchronizes Tab")
        send(mdi, 0x222, tab_documents[1], 0)  # WM_MDIACTIVATE
        wait(lambda: send(document_tabs, 0x130B, 0, 0) == 1, "MDI activation synchronizes Tab")
        send(mdi, 0x224, tab_documents[1], 0)  # WM_MDINEXT, also used by MDI keyboard switching.
        wait(lambda: send(document_tabs, 0x130B, 0, 0) == 0, "MDI next-window synchronizes Tab")
        architecture_controls = [next(h for h in windows(d) if get_id(h) == 1201) for d in docs()]
        selections = [send(h, 0x147, 0, 0) for h in architecture_controls]  # CB_GETCURSEL
        assert sorted(selections) == [0, 1], "independent initial architectures"
        for combo in architecture_controls:
            declaration = next(h for h in windows(get_parent(combo)) if get_id(h) == 1203)
            original = send(combo, 0x147, 0, 0)
            for index, architecture in enumerate(("x86", "x64")):
                label = c.create_unicode_buffer(128)
                send(combo, 0x148, index, c.addressof(label))  # CB_GETLBTEXT
                assert label.value == architecture, "resource/model option mapping"
                send(combo, 0x14E, index, 0)
                send(get_parent(combo), 0x111, 1201 | (1 << 16), combo)
                assert architecture in window_text(get_parent(combo))
                content = window_text(declaration)
                assert architecture in content and "{arch}" not in content and "{title}" not in content
                if args.lang != "auto":
                    assert ("架构：" if args.lang == "zh-CN" else "Architecture:") in content
            send(combo, 0x14E, original, 0)
            send(get_parent(combo), 0x111, 1201 | (1 << 16), combo)
        combo = architecture_controls[0]
        send(combo, 0x14E, 1 - selections[0], 0)  # CB_SETCURSEL
        send(get_parent(combo), 0x111, 1201 | (1 << 16), combo)  # CBN_SELCHANGE
        assert send(architecture_controls[1], 0x147, 0, 0) == selections[1], "architecture leaked to another document"

        def snapshot():
            return {"dpi": get_dpi(frame), "frame": rectangle(frame), "mdi": rectangle(mdi, True),
                    "docs": {str(h): [rectangle(h), rectangle(h, True), origin(h), bool(is_zoomed(h))] for h in docs()}}

        def check(label, tiled=False):
            s = stable(snapshot)
            assert s["dpi"] > 0
            assert send(document_tabs, 0x1304, 0, 0) == len(docs()), "document Tab count"
            if len(docs()) == 2 and set(docs()) == set(tab_documents):
                selected = send(document_tabs, 0x130B, 0, 0)
                assert selected >= 0 and tab_documents[selected] == send(mdi, 0x229, 0, 0), "Tab/MDI selection mismatch"
            assert rectangle(document_tabs)[3] <= origin(mdi)[1], "Tab strip overlaps workspace"
            assert send(document_tabs, 0x31, 0, 0), "document Tab font"
            for hwnd in docs():
                assert get_dpi(hwnd) == s["dpi"], "document DPI"
                if is_zoomed(hwnd):
                    assert rectangle(hwnd, True) == rectangle(mdi, True), "maximized client size"
                    assert origin(hwnd) == origin(mdi), "maximized client origin"
                for control in windows(hwnd):
                    if get_id(control) in (1201, 1202, 1203, 1204):
                        font = send(control, 0x31, 0, 0)
                        assert font, "control has no application font"
                        logfont = c.create_string_buffer(92)
                        assert get_object(c.c_void_p(font), len(logfont), logfont) == 92
                        height = c.c_long.from_buffer(logfont).value
                        assert height < 0, "font character height"
                        assert font_heights.setdefault(s["dpi"], height) == height, "inconsistent font at same DPI"
            if tiled:
                bounds = [rectangle(h) for h in docs()]
                for i, a in enumerate(bounds):
                    for b in bounds[i + 1:]:
                        assert max(a[0], b[0]) >= min(a[2], b[2]) or max(a[1], b[1]) >= min(a[3], b[3]), "tile overlap"
                mx, my = origin(mdi)
                _, _, mw, mh = rectangle(mdi, True)
                assert all(mx <= r[0] <= r[2] <= mx + mw and my <= r[1] <= r[3] <= my + mh for r in bounds), "tile outside workspace"
            samples.append({"step": label, **s})
            return s

        check("initial-maximized")
        send(frame, 0x111, 1010, 0)
        check("columns", True)
        send(document_tabs, 0x1330, 0, 0)
        check("tab-switch-tiled", True)
        assert all(is_visible(h) for h in docs()), "Tab switch hid a tiled document"
        splitter = next(h for h in windows(frame) if class_name(h) == "Winlex.Splitter")
        before_split = origin(mdi)[0]
        send(splitter, 0x201, 1, 0)  # WM_LBUTTONDOWN
        send(splitter, 0x200, 1, 60)  # WM_MOUSEMOVE, local x=60
        send(splitter, 0x202, 0, 60)  # WM_LBUTTONUP
        check("splitter", True)
        assert origin(mdi)[0] > before_split, "splitter did not resize workspace"

        def move(screen):
            previous = set_context(c.c_void_p(-4))
            try:
                assert set_pos(frame, None, screen[0] + 40, screen[1] + 40, 0, 0, 0x15)
            finally:
                set_context(previous)

        for index, screen in enumerate(displays):
            move(screen)
            check(f"columns-monitor-{index}", True)
        for index, screen in reversed(list(enumerate(displays))):
            move(screen)
            check(f"columns-return-{index}", True)
        send(frame, 0x111, 1011, 0)
        check("rows", True)
        send(frame, 0x111, 1013, 0)
        check("maximize")
        for index, screen in enumerate(displays):
            move(screen)
            check(f"maximized-monitor-{index}")
        current = next(h for h in docs() if is_zoomed(h))
        send(current, 0x112, 0xF120, 0)  # SC_RESTORE
        check("restore-at-destination", True)
        send(frame, 0x111, 1001, 0)
        wait(lambda: len(docs()) == 3, "new document")
        check("new-document", True)
        send(frame, 0x111, 1003, 0)
        wait(lambda: len(docs()) == 2, "close document")
        check("close-document", True)
        send(frame, 0x111, 1012, 0)
        check("free")
        # A completed user move/size updates the logical placement once.
        doc = docs()[0]
        send(doc, 0x231, 0, 0)  # WM_ENTERSIZEMOVE
        value = get_dpi(doc)
        assert set_pos(doc, None, 20, 20, 400 * value // 96, 280 * value // 96, 0x14)
        send(doc, 0x232, 0, 0)  # WM_EXITSIZEMOVE
        base = check("free-user-size")
        for index, screen in enumerate(displays):
            move(screen)
            check(f"free-monitor-{index}")
        move(displays[-1])
        after = check("free-return")
        if base["dpi"] == after["dpi"]:
            a, b = base["docs"][str(doc)][1], after["docs"][str(doc)][1]
            assert a == b, "free document size drift"
        # Page navigation must preserve content state rather than rebuilding it.
        owner = send(mdi, 0x229, 0, 0)
        pages = next(h for h in windows(owner) if get_id(h) == 1202)
        declaration = next(h for h in windows(owner) if get_id(h) == 1203)
        table = next(h for h in windows(owner) if get_id(h) == 1204)
        send(declaration, 0xB1, 5, 15)  # EM_SETSEL
        send(table, 0x1043, 0, 1)  # LVM_SETSELECTIONMARK
        for page in (1, 2, 0):
            send(pages, 0x1330, page, 0)
            assert bool(is_visible(declaration)) == (page == 0)
            assert bool(is_visible(table)) == (page != 0)
        assert send(declaration, 0xB0, 0, 0) == (15 << 16) | 5, "page switch reset text selection"
        assert send(table, 0x1042, 0, 0) == 1, "page switch rebuilt table"
        # Keyboard messages go through the real application's message loop.
        # Drive activation explicitly, like the other message-based controls,
        # rather than depending on the system's foreground-window policy.
        send(frame, 0x6, 1, 0)  # WM_ACTIVATE / WA_ACTIVE
        send(send(mdi, 0x229, 0, 0), 0x7, 0, 0)  # WM_SETFOCUS
        wait(lambda: focus_of(frame), "document input focus")
        search = next(h for h in windows(frame) if get_id(h) == 1102)
        current = focus_of(frame)
        def focus_detail(label):
            h = focus_of(frame)
            focus_events.append({"step": label, "hwnd": h, "class": class_name(h), "id": get_id(h),
                                 "parent": get_parent(h), "active": send(mdi, 0x229, 0, 0), "foreground": get_foreground(),
                                 "foreground_class": class_name(get_foreground())})
        focus_detail("before-F6")
        key(current or frame, 0x75)  # F6
        wait(lambda: focus_of(frame) != current, "F6 changes navigation region")
        stable(lambda: (focus_of(frame),))
        if focus_of(frame) == search:
            key(search, 0x75)
            wait(lambda: focus_of(frame) != search, "F6 restores document focus")
        first = stable(lambda: (focus_of(frame),))[0]
        focus_detail("before-Tab")
        key(first, 9)
        wait(lambda: focus_of(frame) != first, f"document Tab navigation from {class_name(first)} id={get_id(first)}")
        remembered = stable(lambda: (focus_of(frame),))[0]
        focus_detail("after-Tab")
        if get_id(remembered) == 1202:
            key(remembered, 0x27)  # Right arrow changes the Tab page.
            wait(lambda: send(remembered, 0x130B, 0, 0) != 0, "Tab page selection")
            owner = get_parent(remembered)
            table = next(h for h in windows(owner) if get_id(h) == 1204)
            wait(lambda: is_visible(table), "selected page visible")
            remembered = stable(lambda: (focus_of(frame),))[0]
        key(remembered, 0x75)
        try:
            wait(lambda: focus_of(frame) == search, "F6 returns to search")
        except AssertionError:
            focus_detail("failed-F6")
            raise
        key(search, 0x75)
        wait(lambda: focus_of(frame) == remembered, "document focus restoration")
        if len(font_heights) > 1:
            assert len(set(font_heights.values())) > 1, "fonts did not scale with DPI"
        # Middle-click closes the pointed inactive document without switching the active one.
        send(document_tabs, 0x1330, 1, 0)
        current = send(mdi, 0x229, 0, 0)
        middle_close_first()
        assert docs() == [current], "middle-click closed the wrong document"
        assert send(mdi, 0x229, 0, 0) == current
        # A click outside the tabs does nothing.
        send(document_tabs, 0x208, 0, 0xFFFFFFFF)
        assert docs() == [current]
        # Closing the last document leaves an empty strip; creating again reuses it.
        while docs():
            count = len(docs())
            middle_close_first()
            wait(lambda: len(docs()) == count - 1, "middle-click closes document")
        assert send(document_tabs, 0x1304, 0, 0) == 0
        assert c.c_int(send(document_tabs, 0x130B, 0, 0)).value == -1
        middle_close_first()  # Empty strip is harmless.
        send(frame, 0x111, 1002, 0)
        wait(lambda: len(docs()) == 1, "reopen after last close")
        assert send(document_tabs, 0x1304, 0, 0) == 1
        assert send(document_tabs, 0x130B, 0, 0) == 0
        for _ in range(11):
            send(frame, 0x111, 1001, 0)
        wait(lambda: send(document_tabs, 0x1304, 0, 0) == 12, "overflow Tab count")
        send(document_tabs, 0x1330, 0, 0)
        first_document = send(mdi, 0x229, 0, 0)
        send(first_document, 0x112, 0xF020, 0)  # SC_MINIMIZE
        send(document_tabs, 0x1330, 11, 0)
        last_document = send(mdi, 0x229, 0, 0)
        assert last_document != first_document
        send(document_tabs, 0x1330, 0, 0)
        assert send(mdi, 0x229, 0, 0) == first_document, "overflow/minimized Tab activation"
        send(first_document, 0x10, 0, 0)  # Native close command also uses the workspace lifecycle.
        assert not is_window(first_document)
        assert len(docs()) == 11 and send(document_tabs, 0x1304, 0, 0) == 11
        print(f"PASS {args.exe.name} {args.dpi} {args.lang}: {len(samples)} snapshots, DPI values={sorted({s['dpi'] for s in samples})}")
        if args.dpi in ("auto", "pmv1", "pmv2") and len({s["dpi"] for s in samples}) < 2:
            print("SKIP cross-DPI coverage: no differing effective DPI observed")
    finally:
        (args.artifacts / f"{args.dpi}.json").write_text(json.dumps(samples, indent=2), encoding="utf-8")
        (args.artifacts / f"{args.dpi}-focus.json").write_text(json.dumps(focus_events, indent=2), encoding="utf-8")
        if frame and is_window(frame):
            post(frame, 0x10, 0, 0)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


if __name__ == "__main__":
    main()

import time
import shutil
import win32con
import win32gui
from support import native as n

RESIZE_NO_MOVE = win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE


def test_online_button_tracks_document_metadata(launch, settings):
    from support.accessibility import client

    app = launch([settings[1]])
    for query, labels in [
        ("CreateFileW", ("Official documentation", "官方文档")),
        ("Windows.Win32.Foundation.WIN32_ERROR.ERROR_ACCESS_DENIED", ("Search online", "在线搜索")),
        ("CreateFileW", ("Official documentation", "官方文档")),
    ]:
        app.query(query)
        doc = app.open_row()
        button = n.get_item(doc, 1211)
        n.wait(lambda: n.window_text(button) in labels, "online action label")
        assert n.is_visible(button)
        assert win32gui.IsWindowEnabled(button)
        with client(button) as accessible:
            assert accessible.name == n.window_text(button)


def test_document_controls_and_close_during_architecture_reload(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "initial declaration", 45)
    editor = n.get_item(doc, 1203)
    assert n.class_name(editor) == "RichEdit20W"
    body = app.content(doc)
    n.send(editor, win32con.WM_CHAR, ord("X"))  # WM_CHAR: declaration stays read-only.
    assert app.content(doc) == body
    assert str(settings[1]) not in body
    assert '"parameters"' not in body
    assert "Windows.Win32.Storage.FileSystem" in n.window_text(n.get_item(doc, 1209))
    assert n.window_text(n.get_item(doc, 1207)) == "CreateFileW"

    app.choose(doc, "x64")
    n.send(doc, win32con.WM_CLOSE)  # Close while a replacement may still be running.
    n.wait(lambda: not app.documents(), "document closed")
    reopened = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(reopened), "worker after cancellation", 45)
    assert app.choice(reopened) == "x64"


def test_architecture_preference_applies_to_new_documents(launch, profile, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    first = app.open_row()
    assert app.choices(app.architecture(first)) == ["x86", "x64", "arm64"]
    assert app.choice(first) == "x86"

    app.choose(first, "x64")
    n.wait(lambda: "// [x64] size:" in app.content(first), "x64 declaration", 45)
    first_body = app.content(first)

    app.command(1049)  # Open the same result in a new document.
    second = n.wait(lambda: len(app.documents()) == 2 and app.active_document(), "new document")
    n.wait(lambda: app.choice(second) == "x64", "new document uses preference")
    assert app.choices(app.architecture(second)) == ["x86", "x64", "arm64"]

    app.choose(second, "arm64")
    n.wait(lambda: "// [arm64] size:" in app.content(second), "arm64 declaration", 45)
    assert app.choice(first) == "x64" and app.content(first) == first_body

    app.command(1049)
    third = n.wait(lambda: len(app.documents()) == 3 and app.active_document(), "third document")
    n.wait(lambda: app.choice(third) == "arm64", "new document uses latest preference")

    app.query("ARM64_NT_CONTEXT")
    fallback = app.open_row()
    n.wait(lambda: "ARM64_NT_CONTEXT" in app.content(fallback), "fallback declaration", 45)
    assert app.choice(fallback) == "x86"
    assert profile.get("PreferredArchitecture") == "arm64"

    # Reselecting an older document's current architecture updates the preference
    # without replacing its text or selection.
    editor = n.get_item(first, 1203)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    app.choose(first, "x64")
    assert profile.get("PreferredArchitecture") == "x64"
    assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2


def test_architecture_preference_survives_restart(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "x64" in n.window_text(doc), "x64 selection")
    assert profile.get("PreferredArchitecture") == "x64"
    app.close()

    restarted = launch([settings[1]])
    restarted.query("CreateFileW")
    reopened = restarted.open_row()
    n.wait(lambda: restarted.choice(reopened) == "x64", "preference persisted")


def test_architecture_cancel_keeps_current_document(launch, profile, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    combo = app.architecture(doc)
    assert app.choice(doc) == "x86"
    n.send(combo, win32con.CB_SETCURSEL, 1)
    app.command(1047, doc, win32con.CBN_SELENDCANCEL)
    assert app.choice(doc) == "x86"
    assert "x86" in n.window_text(doc)
    assert profile.get("PreferredArchitecture") is None


def test_document_minimum_limits_normal_not_maximized_windows(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)  # SC_RESTORE
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        width, height = (320 * dpi + 48) // 96, (400 * dpi + 48) // 96
        assert n.set_pos(doc, None, 0, 0, 100, 100, RESIZE_NO_MOVE)
        left, top, right, bottom = n.rectangle(doc)
        assert (right - left, bottom - top) == (width, height)

        # A maximized document must still fit a container smaller than its normal minimum.
        assert n.set_pos(app.frame, None, 0, 0, 600 * dpi // 96, 300 * dpi // 96, RESIZE_NO_MOVE)
        for state in (win32con.SC_MAXIMIZE, win32con.SC_MINIMIZE, win32con.SC_RESTORE,
                      win32con.SC_MAXIMIZE):
            if state == win32con.SC_MAXIMIZE and n.is_zoomed(doc):
                continue
            n.send(doc, win32con.WM_SYSCOMMAND, state)
            if n.is_zoomed(doc):
                # MDI layout can finish after the synchronous system command returns.
                stable_since = None
                def aligned():
                    nonlocal stable_since
                    origin = n.w.POINT()
                    n.api(n.user, "MapWindowPoints", n.c.c_int, n.w.HWND, n.w.HWND,
                          n.c.POINTER(n.w.POINT), n.w.UINT)(doc, app.mdi, n.c.byref(origin), 1)
                    if ((origin.x, origin.y) != (0, 0)
                            or n.rectangle(doc, client=True) != n.rectangle(app.mdi, client=True)):
                        stable_since = None
                        return False
                    if stable_since is None:
                        stable_since = time.monotonic()
                    return time.monotonic() - stable_since >= 0.2
                n.wait(aligned, "stable maximized document fills MDI client", 5)
        assert n.rectangle(app.mdi, client=True)[3] < height
        n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)
        assert n.set_pos(doc, None, 0, 0, 100, 100, RESIZE_NO_MOVE)
        left, top, right, bottom = n.rectangle(doc)
        assert (right - left, bottom - top) == (width, height)


def test_tab_traverses_document_controls(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    editor = n.get_item(doc, 1203)
    order = [app.architecture(doc), n.get_item(doc, 1206), n.get_item(doc, 1211), editor]
    app.focus(editor)
    for target in order:
        current = app.focused() or editor
        n.key_message(current, win32con.VK_TAB)
        n.wait(lambda: app.focused() == target, "tab to %d" % n.get_id(target))

    body = app.content(doc)
    n.key_message(editor, win32con.VK_RETURN)  # Read-only body keeps Enter.
    assert app.content(doc) == body
    assert len(app.documents()) == 1


def test_narrow_document_keeps_complete_body_usable(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)  # SC_RESTORE
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        assert n.set_pos(doc, None, 0, 0, 340 * dpi // 96, 440 * dpi // 96, RESIZE_NO_MOVE)
        editor = n.get_item(doc, 1203)
        assert "Windows.Win32.Storage.FileSystem" in n.window_text(n.get_item(doc, 1209))
        assert '"parameters"' not in app.content(doc)
        n.send(editor, win32con.EM_SETSEL, 0, -1)
        assert n.send(editor, win32con.EM_GETSEL) != 0
        a, e = map(n.rectangle, (app.architecture(doc), editor))
        assert a[3] <= e[1] < e[3]
        n.send(editor, win32con.EM_SETSEL, -1, -1)
        n.send(editor, win32con.EM_SCROLLCARET)


def test_document_source_and_responsive_reading_layout(launch, settings):
    path = settings[2] / ("来源😀" + "-metadata" * 12 + ".winmd")
    shutil.copyfile(settings[1], path)
    app = launch([path])
    app.query("Windows.Win32.Storage.FileSystem.CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    body = app.content(doc)
    editor = n.get_item(doc, 1203)
    source = n.get_item(doc, 1205)
    selector = n.get_item(doc, 1047)
    assert path.name in n.window_text(source)
    assert not win32gui.IsWindowVisible(n.get_item(doc, 1204))

    with n.window_dpi_context(app.frame):
        n.send(app.mdi, win32con.WM_MDIRESTORE, doc)
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        counts = []
        for width in [850, 320, 850]:
            win32gui.SetWindowPos(doc, 0, 0, 0, width * dpi // 96, 450 * dpi // 96,
                                  win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
            assert app.content(doc) == body
            sr = n.rectangle(source)
            ar = n.rectangle(selector)
            er = n.rectangle(editor)
            if width == 320:
                assert sr[3] <= ar[1]
            else:
                assert sr[2] < ar[0]
            assert er[1] >= sr[3]
            left, top = win32gui.ClientToScreen(doc, (0, 0))
            _, _, right, bottom = win32gui.GetClientRect(doc)
            for control in [source, selector, editor]:
                x1, y1, x2, y2 = n.rectangle(control)
                assert left <= x1 <= x2 <= left + right
                assert top <= y1 <= y2 <= top + bottom
            counts.append(n.send(editor, win32con.EM_GETLINECOUNT))
        assert counts[1] > counts[0]
        assert counts[2] == counts[0]


def test_document_button_uses_native_ideal_size_and_row_centering(launch, settings):
    import struct

    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    button = n.get_item(doc, 1206)
    combo = app.architecture(doc)
    get_dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND) if hasattr(n.user, "GetDpiForWindow") else None
    with n.window_dpi_context(app.frame):
        n.send(app.mdi, win32con.WM_MDIRESTORE, doc)
        for left, top, _, _ in n.monitors():
            assert n.set_pos(app.frame, None, left + 20, top + 20, 0, 0,
                             win32con.SWP_NOSIZE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
            dpi = get_dpi(doc) if get_dpi else 96
            for width in (850, 320, 200, 850):
                assert n.set_pos(doc, None, 0, 0, width * dpi // 96, 450 * dpi // 96, RESIZE_NO_MOVE)
                assert n.write_memory(app.handle, app.memory, bytes(8), 8, None)
                assert n.send(button, 0x1601, 0, app.memory)
                size = n.c.create_string_buffer(8)
                assert n.read_memory(app.handle, app.memory, size, 8, None)
                ideal = struct.unpack("<ii", size.raw)
                assert n.is_visible(button)
                br, cr = n.rectangle(button), n.rectangle(combo)
                assert (br[2] - br[0], br[3] - br[1]) == ideal
                if br[0] >= cr[2]:
                    assert abs(br[1] + br[3] - cr[1] - cr[3]) <= 1
                else:
                    assert br[1] >= cr[3]
                online = n.get_item(doc, 1211)
                assert n.is_visible(online)
                assert n.write_memory(app.handle, app.memory, bytes(8), 8, None)
                assert n.send(online, 0x1601, 0, app.memory)
                assert n.read_memory(app.handle, app.memory, size, 8, None)
                online_ideal = struct.unpack("<ii", size.raw)
                online_rect = n.rectangle(online)
                assert (online_rect[2] - online_rect[0], online_rect[3] - online_rect[1]) == online_ideal
                assert online_rect[0] >= br[2] or online_rect[1] >= br[3]
                er = n.rectangle(n.get_item(doc, 1203))
                assert er[1] >= max(br[3], cr[3], online_rect[3])


def test_constant_summary_pseudocode_and_information_copy(launch, settings):
    from test_context import find_popup, wait_closed, wait_clipboard_available, clipboard_text
    from support.accessibility import client
    import win32clipboard as clipboard

    path = settings[2] / "来源&😀.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    app.query("Windows.Win32.Foundation.WIN32_ERROR.ERROR_ACCESS_DENIED")
    doc = app.open_row()
    n.wait(lambda: "ERROR_ACCESS_DENIED = 5;" in app.content(doc), "constant", 45)
    assert app.content(doc).strip() == "WIN32_ERROR ERROR_ACCESS_DENIED = 5;"
    assert n.window_text(n.get_item(doc, 1207)) == "ERROR_ACCESS_DENIED"
    assert n.window_text(n.get_item(doc, 1208)) in ("Constant", "常量")
    assert n.window_text(n.get_item(doc, 1209)) == "Windows.Win32.Foundation · WIN32_ERROR"
    for control, names in [(1207, ("Name", "名称")),
                           (1209, ("Namespace and declaring type", "命名空间与所属类型")),
                           (1205, ("Source:", "来源：")),
                           (1203, ("Declaration", "声明"))]:
        hwnd = n.get_item(doc, control)
        with client(hwnd) as accessible:
            assert accessible.name in names
    button = n.get_item(doc, 1206)
    expected = [str(path), "0::0x04002ffc", "ERROR_ACCESS_DENIED",
                "Windows.Win32.Foundation.WIN32_ERROR.ERROR_ACCESS_DENIED", path.name]
    for index, value in enumerate(expected):
        app.focus(button)
        assert app.focused() == button
        wait_clipboard_available()
        sequence = clipboard.GetClipboardSequenceNumber()
        assert n.post(button, win32con.WM_KEYDOWN, win32con.VK_SPACE, 1)
        n.wait(lambda: n.send(button, win32con.BM_GETSTATE) & 4, "button pressed")
        assert n.post(button, win32con.WM_KEYUP, win32con.VK_SPACE, n.KEY_UP_LPARAM)
        menu, items = find_popup(app)
        assert items[2] == 0  # Separator between identity and convenient text copies.
        assert len(items) == 6
        for _ in range(index + 1):
            n.key_message(button, win32con.VK_DOWN)
        n.key_message(button, win32con.VK_RETURN)
        wait_closed(button)
        n.send(doc, win32con.WM_NULL)
        n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "information copied")
        assert clipboard_text() == value
        assert app.focused() == button
    sequence = clipboard.GetClipboardSequenceNumber()
    assert n.post(button, win32con.BM_CLICK, 0, 0)
    find_popup(app)
    n.key_message(button, win32con.VK_ESCAPE)
    wait_closed(button)
    n.send(doc, win32con.WM_NULL)
    assert clipboard.GetClipboardSequenceNumber() == sequence
    assert app.focused() == button


def test_tiny_maximized_document_clips_summary_without_control_overlap(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        n.send(app.mdi, win32con.WM_MDIMAXIMIZE, doc)
        assert n.set_pos(app.frame, None, 0, 0, 650 * dpi // 96, 140 * dpi // 96, RESIZE_NO_MOVE)
        for control in [1047, 1206, 1211]:
            assert not n.is_visible(n.get_item(doc, control))
        assert n.rectangle(n.get_item(doc, 1203), client=True)[3] == 0

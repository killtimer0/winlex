import time
import shutil
import pytest
import win32con
import win32gui
import uiautomation as auto
from support import native as n
from test_context import select_text, selection_packed

def window_state(hwnd, state):
    pattern = auto.ControlFromHandle(hwnd).GetPattern(auto.PatternId.WindowPattern)
    assert pattern.SetWindowVisualState(state, waitTime=0)
    n.wait(lambda: (bool(n.is_zoomed(hwnd)), bool(win32gui.IsIconic(hwnd))) == (
        state == auto.WindowVisualState.Maximized, state == auto.WindowVisualState.Minimized),
        "requested native window state")


def resize(hwnd, width, height):
    pattern = auto.ControlFromHandle(hwnd).GetPattern(auto.PatternId.TransformPattern)
    assert pattern.Resize(width, height, waitTime=0)


def test_large_api_document_stays_responsive(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Graphics.Gdi.Apis")
    doc = app.open_row()
    editor = n.get_item(doc, 1203)
    n.wait(lambda: "EnumDisplayMonitors(" in n.rich_edit_text(editor), "complete Gdi declaration", 45)
    app.focus(editor)
    app.choose(doc, "x64" if app.choice(doc) == "x86" else "x86")
    n.wait(lambda: "EnumDisplayMonitors(" in n.rich_edit_text(editor), "focused Gdi declaration reload", 45)
    body = n.rich_edit_text(editor)
    assert len(body) > 100_000
    assert body.rstrip().endswith("};")
    app.focus(editor)
    select_text(editor, 6000, 6005)
    n.send(editor, win32con.EM_LINESCROLL, 0, 30)
    first = n.send(editor, win32con.EM_GETFIRSTVISIBLELINE)
    assert first > 0
    # All cross-process messages have a bounded timeout, including recoloring.
    n.send(app.frame, win32con.WM_SYSCOLORCHANGE)
    assert n.rich_edit_text(editor) == body
    assert selection_packed(editor) == (6005 << 16) | 6000
    assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == first


def test_online_button_tracks_document_metadata(launch, settings):
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
        accessible = auto.ControlFromHandle(button).GetLegacyIAccessiblePattern()
        assert accessible.Name == n.window_text(button)


def test_document_controls_and_close_during_architecture_reload(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "initial declaration", 45)
    editor = n.get_item(doc, 1203)
    body = app.content(doc)
    n.send(editor, win32con.WM_CHAR, ord("X"))
    assert app.content(doc) == body
    assert "Windows.Win32.Storage.FileSystem" in n.window_text(n.get_item(doc, 1209))
    assert n.window_text(n.get_item(doc, 1207)) == "CreateFileW"

    app.choose(doc, "x64")
    auto.ControlFromHandle(doc).GetPattern(auto.PatternId.WindowPattern).Close(waitTime=0)
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
    select_text(editor, 2, 8)
    app.choose(first, "x64")
    assert profile.get("PreferredArchitecture") == "x64"
    assert selection_packed(editor) == (8 << 16) | 2


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
    n.send(doc, win32con.WM_COMMAND,
           1047 | ((win32con.CBN_SELENDCANCEL & 0xffff) << 16), combo)
    assert app.choice(doc) == "x86"
    assert "x86" in n.window_text(doc)
    assert profile.get("PreferredArchitecture") is None


def test_document_minimum_limits_normal_not_maximized_windows(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    window_state(doc, auto.WindowVisualState.Normal)
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        width, height = (320 * dpi + 48) // 96, (400 * dpi + 48) // 96
        resize(doc, 100, 100)
        left, top, right, bottom = n.rectangle(doc)
        assert (right - left, bottom - top) == (width, height)

        # A maximized document must still fit a container smaller than its normal minimum.
        resize(app.frame, 600 * dpi // 96, 300 * dpi // 96)
        for state in (auto.WindowVisualState.Maximized, auto.WindowVisualState.Minimized,
                      auto.WindowVisualState.Normal, auto.WindowVisualState.Maximized):
            if state == auto.WindowVisualState.Maximized and n.is_zoomed(doc):
                continue
            window_state(doc, state)
            if state == auto.WindowVisualState.Maximized:
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
        window_state(doc, auto.WindowVisualState.Normal)
        resize(doc, 100, 100)
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
        win32gui.PostMessage(current, win32con.WM_KEYDOWN, win32con.VK_TAB, 1)
        win32gui.PostMessage(current, win32con.WM_KEYUP, win32con.VK_TAB, -1073741823)
        n.wait(lambda: app.focused() == target, "tab to %d" % n.get_id(target))

    body = app.content(doc)
    win32gui.PostMessage(editor, win32con.WM_KEYDOWN, win32con.VK_RETURN, 1)
    win32gui.PostMessage(editor, win32con.WM_KEYUP, win32con.VK_RETURN, -1073741823)
    assert app.content(doc) == body
    assert len(app.documents()) == 1


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
    buttons = [n.get_item(doc, control) for control in (1206, 1211)]
    assert path.name in n.window_text(source)
    assert not win32gui.IsWindowVisible(n.get_item(doc, 1204))

    with n.window_dpi_context(app.frame):
        window_state(doc, auto.WindowVisualState.Normal)
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        counts = []
        for width in [850, 320, 850]:
            resize(doc, width * dpi // 96, 450 * dpi // 96)
            assert app.content(doc) == body
            sr = n.rectangle(source)
            ar = n.rectangle(selector)
            er = n.rectangle(editor)
            if width == 320:
                assert sr[3] <= ar[1]
            else:
                assert sr[2] < ar[0]
            assert er[1] >= max(sr[3], ar[3]) and er[3] > er[1]
            previous = ar
            for button in buttons:
                assert n.is_visible(button)
                bounds = n.rectangle(button)
                if width == 320:
                    assert bounds[0] >= previous[2] or bounds[1] >= previous[3]
                else:
                    assert bounds[0] >= previous[2]
                previous = bounds
            select_text(editor, 0, -1)
            assert selection_packed(editor) != 0
            n.send(editor, win32con.EM_SETSEL, -1, -1)
            n.send(editor, win32con.EM_SCROLLCARET)
            left, top = win32gui.ClientToScreen(doc, (0, 0))
            _, _, right, bottom = win32gui.GetClientRect(doc)
            for control in [source, selector, editor, *buttons]:
                x1, y1, x2, y2 = n.rectangle(control)
                assert left <= x1 < x2 <= left + right
                assert top <= y1 < y2 <= top + bottom
            counts.append(n.send(editor, win32con.EM_GETLINECOUNT))
        assert counts[1] > counts[0]
        assert counts[2] == counts[0]


@pytest.mark.parametrize('copy', [False, pytest.param(True, marks=pytest.mark.interactive)],
                         ids=['summary-contract', 'clipboard'])
def test_constant_summary_pseudocode_and_information_copy(launch, settings, copy):
    import json
    import subprocess
    from test_context import (
        clipboard_text, find_popup, menu_key, popup_window, wait_closed,
        wait_clipboard_available,
    )
    from support.input_state import InputState, foreground
    import win32clipboard as clipboard

    path = settings[2] / "来源&😀.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    app.query("Windows.Win32.Foundation.WIN32_ERROR.ERROR_ACCESS_DENIED")
    matches = json.loads(subprocess.check_output([
        str(settings[0].with_name("winlex.exe")), str(path), "search",
        app.cell(0).splitlines()[0], "--name", "--json",
    ], encoding="utf-8"))["result"]["matches"]
    assert len(matches) == 1
    identity = matches[0]["id"]
    entity_id = f'{identity["source_id"]}::0x{identity["token"]:08x}'
    doc = app.open_row()
    n.wait(lambda: "ERROR_ACCESS_DENIED = 5;" in app.content(doc), "constant", 45)
    assert app.content(doc).strip() == "WIN32_ERROR ERROR_ACCESS_DENIED = 5; // 0x00000005"
    assert n.window_text(n.get_item(doc, 1207)) == "ERROR_ACCESS_DENIED"
    assert n.window_text(n.get_item(doc, 1209)) == "Windows.Win32.Foundation · WIN32_ERROR"
    for control, names in [(1207, ("Name", "名称")),
                           (1209, ("Namespace and declaring type", "命名空间与所属类型")),
                           (1205, ("Source:", "来源：")),
                           (1203, ("Declaration", "声明"))]:
        hwnd = n.get_item(doc, control)
        accessible = auto.ControlFromHandle(hwnd).GetLegacyIAccessiblePattern()
        assert accessible.Name in names
    if not copy:
        return
    button = n.get_item(doc, 1206)
    expected = [str(path), entity_id, "ERROR_ACCESS_DENIED",
                "Windows.Win32.Foundation.WIN32_ERROR.ERROR_ACCESS_DENIED", path.name]
    with InputState() as inputs, foreground(app, inputs):
        for index, value in enumerate(expected):
            app.focus(button)
            assert app.focused() == button
            wait_clipboard_available()
            sequence = clipboard.GetClipboardSequenceNumber()
            auto.ControlFromHandle(button).Click()
            find_popup(app)
            window = popup_window(app.process.pid)
            assert window, "information menu window"
            for _ in range(index + 1):
                menu_key(window, win32con.VK_DOWN)
            menu_key(window, win32con.VK_RETURN)
            wait_closed(button)
            n.send(doc, win32con.WM_NULL)
            n.wait(lambda: clipboard.GetClipboardSequenceNumber() != sequence, "information copied")
            assert clipboard_text() == value
            assert app.focused() == button
        sequence = clipboard.GetClipboardSequenceNumber()
        auto.ControlFromHandle(button).Click()
        find_popup(app)
        window = popup_window(app.process.pid)
        assert window, "information menu window"
        menu_key(window, win32con.VK_ESCAPE)
        wait_closed(button)
        n.send(doc, win32con.WM_NULL)
        assert clipboard.GetClipboardSequenceNumber() == sequence
        assert app.focused() == button


def test_tiny_maximized_document_clips_summary_without_control_overlap(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    editor = n.get_item(doc, 1203)
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        window_state(doc, auto.WindowVisualState.Maximized)
        resize(app.frame, 650 * dpi // 96, 140 * dpi // 96)
        for control in [1047, 1206, 1211]:
            assert not n.is_visible(n.get_item(doc, control))
        assert n.rectangle(editor, client=True)[3] == 0

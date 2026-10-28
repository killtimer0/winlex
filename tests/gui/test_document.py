import time
import win32con
from support import native as n

RESIZE_NO_MOVE = win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE


def test_document_controls_and_close_during_architecture_reload(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "initial declaration", 45)
    editor = n.get_item(doc, 1203)
    assert n.class_name(editor) == "RichEdit20W"
    body = app.content(doc)
    n.send(editor, win32con.WM_CHAR, ord("X"))  # WM_CHAR: declaration stays read-only.
    assert app.content(doc) == body
    details = n.get_item(doc, 1205)
    assert not n.is_visible(details)
    app.command(1048, doc)
    assert n.is_visible(details)
    assert str(settings[1]) in n.window_text(details)
    assert '"parameters"' in n.window_text(details)
    app.command(1048, doc)
    assert not n.is_visible(details)

    app.choose(doc, "x64")
    n.send(doc, win32con.WM_CLOSE)  # Close while a replacement may still be running.
    n.wait(lambda: not app.documents(), "document closed")
    reopened = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(reopened), "worker after cancellation", 45)
    assert app.choice(reopened) == "x64"


def test_architecture_preference_applies_to_new_documents(launch, profile, settings):
    app = launch(["--winmd", settings[1]])
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
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    app.choose(doc, "x64")
    n.wait(lambda: "x64" in n.window_text(doc), "x64 selection")
    assert profile.get("PreferredArchitecture") == "x64"
    app.close()

    restarted = launch(["--winmd", settings[1]])
    restarted.query("CreateFileW")
    reopened = restarted.open_row()
    n.wait(lambda: restarted.choice(reopened) == "x64", "preference persisted")


def test_architecture_cancel_keeps_current_document(launch, profile, settings):
    app = launch(["--winmd", settings[1]])
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
    app = launch(["--winmd", settings[1]])
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
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    app.command(1048, doc)  # Show details so they join the tab order.
    details = n.get_item(doc, 1205)
    assert n.is_visible(details)

    editor = n.get_item(doc, 1203)
    order = [n.get_item(doc, 1204), details, app.architecture(doc),
             n.get_item(doc, 1048), editor]
    app.focus(editor)
    for target in order:
        current = app.focused() or editor
        n.key_message(current, win32con.VK_TAB)
        n.wait(lambda: app.focused() == target, "tab to %d" % n.get_id(target))

    body = app.content(doc)
    n.key_message(editor, win32con.VK_RETURN)  # Read-only body keeps Enter.
    assert app.content(doc) == body
    assert len(app.documents()) == 1


def test_narrow_document_keeps_details_and_declaration_usable(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
    app.command(1048, doc)
    n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)  # SC_RESTORE
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        assert n.set_pos(doc, None, 0, 0, 340 * dpi // 96, 440 * dpi // 96, RESIZE_NO_MOVE)
        header = n.get_item(doc, 1204)
        details = n.get_item(doc, 1205)
        editor = n.get_item(doc, 1203)
        assert n.class_name(header) == "Edit"
        assert "Windows.Win32.Storage.FileSystem" in n.window_text(header)
        n.send(header, win32con.EM_SETSEL, 0, -1)  # Full header is selectable despite the narrow view.
        assert n.send(header, win32con.EM_GETSEL) != 0
        h, d, e = map(n.rectangle, (header, details, editor))
        assert h[3] <= d[1] < d[3] < e[1] < e[3]
        assert e[3] - e[1] >= d[3] - d[1]
        n.send(details, win32con.EM_LINESCROLL, 0, 20)  # EM_LINESCROLL
        assert n.send(details, win32con.EM_GETFIRSTVISIBLELINE) > 0  # EM_GETFIRSTVISIBLELINE

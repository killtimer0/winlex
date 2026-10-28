import win32con
from support import native as n

RESIZE_NO_MOVE = win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE


def test_architecture_selection_reloads_in_place(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(doc), "initial declaration", 45)
    combo = n.get_item(doc, 1047)
    assert n.send(combo, win32con.CB_GETCOUNT) == 3
    assert n.send(combo, win32con.CB_GETCURSEL) == 1  # CB_GETCURSEL: x64
    for index in (0, 2, 1, 2):
        n.send(combo, win32con.CB_SETCURSEL, index)
        app.command(1047, doc)
    n.wait(lambda: "CreateFileW(" in app.content(doc), "final architecture", 45)
    assert "arm64" in n.window_text(doc)
    assert app.documents() == [doc]
    editor = n.get_item(doc, 1203)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    app.command(1047, doc)  # Reselecting the same architecture preserves selection.
    assert n.send(editor, win32con.EM_GETSEL) == (8 << 16) | 2
    body = app.content(doc)
    n.send(editor, win32con.WM_CHAR, ord("X"))  # WM_CHAR: declaration stays read-only.
    assert app.content(doc) == body
    details = n.get_item(doc, 1205)
    assert not n.is_visible(details)
    app.command(1048, doc)
    assert n.is_visible(details)
    assert str(settings[1]) in n.window_text(details)
    metadata = n.window_text(details)
    assert '"parameters"' in metadata and '"attributes"' in metadata
    assert '"KERNEL32.dll"' in metadata and '"calling_convention"' in metadata
    app.command(1048, doc)
    assert not n.is_visible(details)

    n.send(combo, win32con.CB_SETCURSEL, 0)
    app.command(1047, doc)
    n.send(doc, win32con.WM_CLOSE)  # Close while a replacement may still be running.
    n.wait(lambda: not app.documents(), "document closed")
    reopened = app.open_row()
    n.wait(lambda: "CreateFileW(" in app.content(reopened), "worker after cancellation", 45)


def test_arm64_only_document(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("WHV_ARM64_PENDING_EVENT_TYPE")
    doc = app.open_row()
    n.wait(lambda: "enum WHV_ARM64_PENDING_EVENT_TYPE" in app.content(doc), "ARM64-only declaration", 45)
    assert "arm64" in n.window_text(doc)
    combo = n.get_item(doc, 1047)
    assert n.send(combo, win32con.CB_GETCOUNT) == 1
    assert n.send(combo, win32con.CB_GETCURSEL) == 0


def test_abi_annotations_follow_document_architecture(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    doc = app.open_row()
    n.wait(lambda: "// [x64] size: 24(0x18) bytes" in app.content(doc), "x64 struct layout", 45)
    assert "UINT32 nLength;" in app.content(doc)
    combo = n.get_item(doc, 1047)
    n.send(combo, win32con.CB_SETCURSEL, 0)
    app.command(1047, doc)
    n.wait(lambda: "// [x86] size: 12(0xC) bytes" in app.content(doc), "x86 struct layout", 45)
    assert "offset: 4(0x4) bytes, size: 4(0x4) bytes\r\n    void *lpSecurityDescriptor;" in app.content(doc)
    app.query("Windows.Win32.System.Com.IUnknown")
    interface = app.open_row()
    n.wait(lambda: "slot 2(0x2), vtable +16(0x10) bytes" in app.content(interface), "x64 vtable", 45)
    n.send(n.get_item(interface, 1047), win32con.CB_SETCURSEL, 0)
    app.command(1047, interface)
    n.wait(lambda: "slot 2(0x2), vtable +8(0x8) bytes" in app.content(interface), "x86 vtable", 45)
    assert "slot 2(0x2), vtable +16(0x10) bytes" not in app.content(interface)


def test_document_minimum_only_limits_interactive_sizing(launch, settings):
    app = launch(["--winmd", settings[1]])
    app.query("CreateFileW")
    doc = app.open_row()
    n.send(doc, win32con.WM_SYSCOMMAND, win32con.SC_RESTORE)  # SC_RESTORE
    with n.window_dpi_context(app.frame):
        dpi = n.api(n.user, "GetDpiForWindow", n.w.UINT, n.w.HWND)(doc) if hasattr(n.user, "GetDpiForWindow") else 96
        width, height = (320 * dpi + 48) // 96, (400 * dpi + 48) // 96
        for edge in range(win32con.WMSZ_LEFT, win32con.WMSZ_BOTTOMRIGHT + 1):
            rect = n.w.RECT(100, 100, 200, 200)
            assert n.send(doc, win32con.WM_SIZING, edge, n.c.addressof(rect))  # WM_SIZING marshals RECT
            expected = [100, 100, 200, 200]
            if edge in (win32con.WMSZ_LEFT, win32con.WMSZ_TOPLEFT, win32con.WMSZ_BOTTOMLEFT):
                expected[0] = 200 - width
            if edge in (win32con.WMSZ_RIGHT, win32con.WMSZ_TOPRIGHT, win32con.WMSZ_BOTTOMRIGHT):
                expected[2] = 100 + width
            if edge in (win32con.WMSZ_TOP, win32con.WMSZ_TOPLEFT, win32con.WMSZ_TOPRIGHT):
                expected[1] = 200 - height
            if edge in (win32con.WMSZ_BOTTOM, win32con.WMSZ_BOTTOMLEFT, win32con.WMSZ_BOTTOMRIGHT):
                expected[3] = 100 + height
            assert [rect.left, rect.top, rect.right, rect.bottom] == expected
        # DPI restoration and MDI arrangement must be able to size below the interactive minimum.
        for w, h in ((280 * dpi // 96, 340 * dpi // 96), (width, height), (280 * dpi // 96, 340 * dpi // 96)):
            assert n.set_pos(doc, None, 0, 0, w, h, RESIZE_NO_MOVE)
            left, top, right, bottom = n.rectangle(doc)
            assert (right - left, bottom - top) == (w, h)


def test_narrow_document_keeps_details_and_declaration_usable(launch, settings):
    from support.diagnostics import screenshot
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
        n.send(details, win32con.EM_LINESCROLL, 0, -20)
        screenshot(app.frame, settings[2] / "document-narrow.bmp")
    app.command(1048, doc)
    app.command(1013)
    screenshot(app.frame, settings[2] / "document-default.bmp")

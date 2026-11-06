"""Declaration links preserve document identity and ordinary editor gestures."""
import shutil

import pytest
import win32con
import win32gui
import uiautomation as auto

from support import native as n
from support.session import invoke
from support.input_state import foreground as real_foreground
from test_context import character_point, dismiss, find_popup, popup, select_text, selection_packed, wait_closed


def activate(app, doc):
    app.focus(doc)
    n.wait(lambda: app.active_document() == doc, "document activated")


def reference(app, doc, spelling):
    n.wait(lambda: spelling in app.content(doc), "reference text", 45)
    text = app.content(doc).replace("\r\n", "\r")
    return len(text[:text.index(spelling)].encode("utf-16-le")) // 2


def position(app, editor, index):
    editor_selection(app, editor, (index, index))
    screen = character_point(app, editor, index)
    point = n.w.POINT(n.c.c_short(screen & 0xffff).value,
                      n.c.c_short(screen >> 16).value)
    assert n.api(n.user, "ScreenToClient", n.w.BOOL, n.w.HWND,
                 n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
    left, top, right, bottom = n.rectangle(editor, client=True)
    assert left <= point.x < right and top <= point.y < bottom
    return (point.x & 0xffff) | ((point.y & 0xffff) << 16)


def click(app, doc, index, control=True):
    editor = n.get_item(doc, 1203)
    activate(app, doc)
    app.focus(editor)
    with n.window_dpi_context(app.frame):
        point = position(app, editor, index)
        if control:
            win32gui.PostMessage(editor, win32con.WM_LBUTTONDOWN, win32con.MK_CONTROL | win32con.MK_LBUTTON, point)
            win32gui.PostMessage(editor, win32con.WM_LBUTTONUP, win32con.MK_CONTROL, point)
        else:
            n.send(editor, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
            n.send(editor, win32con.WM_LBUTTONUP, 0, point)
    n.send(app.frame, win32con.WM_NULL)


def destination(app, source, name, architecture):
    doc = n.wait(lambda: (lambda doc: doc if doc in app.documents()
                         and n.window_text(doc).split(" (", 1)[0] == name + " - " + architecture else None)(
                             app.active_document()), "navigation destination")
    assert app.choice(doc) == architecture
    n.wait(lambda: app.focused() == n.get_item(doc, 1203), "destination editor focused")
    n.wait(lambda: name in app.content(doc), "destination declaration", 45)
    return doc


def pin_active_tab(app):
    page = next(page for page in app._tabs() if page.GetSelectionItemPattern().IsSelected)
    rect = page.BoundingRectangle
    tabs = n.get_item(app.frame, 1104)
    left, top, _, _ = n.rectangle(tabs)
    point = ((rect.left - left + min(20, (rect.right - rect.left) // 2)) & 0xffff) | (
        (((rect.top + rect.bottom) // 2 - top) & 0xffff) << 16)
    n.send(tabs, win32con.WM_LBUTTONDBLCLK, win32con.MK_LBUTTON, point)
    n.send(tabs, win32con.WM_LBUTTONUP, 0, point)
    n.wait(lambda: not any(text.startswith(("Preview · ", "预览 · "))
                          for text in app.tab_descriptions()), "tab kept open")


def editor_selection(app, editor, value=None):
    if value is not None:
        select_text(editor, *value)
    selected = n.rich_edit_document(editor).Selection
    return selected.Start, selected.End


def follow_caret(app, doc, index):
    activate(app, doc)
    editor = n.get_item(doc, 1203)
    app.focus(editor)
    editor_selection(app, editor, (index, index))
    _, items = popup(app, editor, keyboard=True)
    menu = auto.MenuControl(searchDepth=3, ClassName="#32768", ProcessId=app.process.pid)
    invoke(menu.MenuItemControl(AutomationId=str(items[0])))
    wait_closed(editor)


def test_ctrl_click_reuses_explicitly_fixed_architecture_links(launch, settings, profile):
    app = launch([settings[1]])
    app.query("Windows.Win32.Storage.FileSystem.CreateFileW")
    source = app.open_row()
    targets = {}
    for architecture, size in (("x64", "24 bytes"), ("x86", "12 bytes"), ("x64", "24 bytes")):
        app.choose(source, architecture)
        index = reference(app, source, "SECURITY_ATTRIBUTES") + 3
        n.send(app.frame, win32con.WM_SYSCOLORCHANGE)
        editor = n.get_item(source, 1203)
        n.send(editor, win32con.EM_LINESCROLL, 0, 100)
        before = (n.window_text(app.search), [row.Name for row in app.rows()],
                  app.selected_row(), profile.get("PreferredArchitecture"))
        click(app, source, index)
        target = destination(app, source, "SECURITY_ATTRIBUTES", architecture)
        n.wait(lambda: size in app.content(target), "architecture-specific layout", 45)
        assert "Windows.Win32.Security" in n.window_text(n.get_item(target, 1209))
        if architecture in targets:
            assert target == targets[architecture]
            assert not any(value.startswith(("Preview · ", "预览 · "))
                           for value in app.tab_descriptions())
        else:
            assert sum(value.startswith(("Preview · ", "预览 · "))
                       for value in app.tab_descriptions()) == 1
            pin_active_tab(app)
        targets[architecture] = target
        assert source in app.documents()
        assert len(app.documents()) == 1 + len(targets)
        assert not any(value.startswith(("Preview · ", "预览 · "))
                       for value in app.tab_descriptions())
        assert before == (n.window_text(app.search), [row.Name for row in app.rows()],
                          app.selected_row(), profile.get("PreferredArchitecture"))
    app.query("CloseHandle")
    preview = app.preview_row()
    assert preview not in [source, *targets.values()]
    assert "CreateFileW(" in app.content(source)
    app.choose(preview, "x86")
    preferred = profile.get("PreferredArchitecture")
    assert app.choice(source) == "x64" and app.choice(preview) == "x86"
    follow_caret(app, source, reference(app, source, "SECURITY_ATTRIBUTES") + 3)
    assert destination(app, source, "SECURITY_ATTRIBUTES", "x64") == targets["x64"]
    assert profile.get("PreferredArchitecture") == preferred
    assert n.window_text(app.search) == "CloseHandle"


def test_link_clicks_keep_selection_and_require_a_complete_ctrl_gesture(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    source = app.open_row()
    editor = n.get_item(source, 1203)
    index = reference(app, source, "SECURITY_ATTRIBUTES")
    click(app, source, index + 3, control=False)
    selection = selection_packed(editor)
    assert selection & 0xffff == selection >> 16
    with n.window_dpi_context(app.frame):
        point = position(app, editor, index + 3)
        n.send(editor, win32con.WM_LBUTTONDBLCLK, win32con.MK_LBUTTON, point)
        n.send(editor, win32con.WM_LBUTTONUP, 0, point)
        selection = selection_packed(editor)
        assert selection & 0xffff == index
        assert selection >> 16 == index + len("SECURITY_ATTRIBUTES")

        # A release alone, or Ctrl added only on release, is not a link click.
        n.send(editor, win32con.WM_LBUTTONUP, win32con.MK_CONTROL, point)
        n.send(editor, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, point)
        n.send(editor, win32con.WM_LBUTTONUP, win32con.MK_CONTROL, point)
    n.send(app.frame, win32con.WM_NULL)
    assert app.documents() == [source]
    assert app.active_document() == source


@pytest.mark.parametrize("keyboard", [False, True], ids=["right-click", "apps-key"])
def test_reference_context_menu_navigates_from_mouse_or_caret(launch, settings, keyboard):
    app = launch([settings[1]])
    app.query("CreateFileW")
    source = app.preview_row()
    editor = n.get_item(source, 1203)
    index = reference(app, source, "SECURITY_ATTRIBUTES") + 4
    app.focus(editor)
    with n.window_dpi_context(app.frame):
        position(app, editor, index)
        location = character_point(app, editor, index)
        _, items = popup(app, editor, location, keyboard=keyboard, protocol_mouse=not keyboard)
        assert items[0] not in (0, 2071, 2072, 2073, 2074)
        menu = auto.MenuControl(searchDepth=3, ClassName="#32768", ProcessId=app.process.pid)
        invoke(menu.MenuItemControl(AutomationId=str(items[0])))
        wait_closed(editor)
    target = destination(app, source, "SECURITY_ATTRIBUTES", "x86")
    assert app.documents() == [source] and target == source
    assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))


def test_reference_activates_fixed_target_without_pinning_or_rebinding_source_preview(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    target = app.open_row()
    n.wait(lambda: "SECURITY_ATTRIBUTES" in app.content(target), "fixed target declaration", 45)
    app.query("CreateFileW")
    source = app.preview_row()
    index = reference(app, source, "SECURITY_ATTRIBUTES") + 3
    body = app.content(source)
    descriptions = app.tab_descriptions()
    assert sum(text.startswith(("Preview · ", "预览 · ")) for text in descriptions) == 1
    click(app, source, index)
    assert destination(app, source, "SECURITY_ATTRIBUTES", "x86") == target
    assert set(app.documents()) == {source, target}
    assert app.content(source) == body
    assert app.tab_descriptions() == descriptions
    app.query("CloseHandle")
    assert app.preview_row() == source
    assert "SECURITY_ATTRIBUTES" in app.content(target)


def test_primitives_unresolved_types_and_whitespace_have_no_navigation(launch, settings):
    app = launch([settings[1]])
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    source = app.open_row()
    editor = n.get_item(source, 1203)
    primitive = reference(app, source, "UINT32")
    unresolved = reference(app, source, "ValueType")
    text = app.content(source).replace("\r\n", "\r")
    space = text.index("UINT32") - 1
    assert text[space].isspace()
    whitespace = len(text[:space].encode("utf-16-le")) // 2
    for index in (primitive + 2, unresolved + 3, whitespace):
        click(app, source, index)
        assert app.documents() == [source]
        _, items = popup(app, editor, keyboard=True)
        assert items == [2074]
        dismiss(editor)
        n.send(editor, win32con.WM_NULL)
        assert app.documents() == [source]


@pytest.mark.parametrize("reload", [True, False], ids=["reload", "unload-reopen"])
def test_navigation_uses_source_snapshot_not_current_catalog(launch, settings, reload):
    path = settings[2] / "navigation-snapshot😀.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    app.query("CreateFileW")
    source = app.open_row()
    index = reference(app, source, "SECURITY_ATTRIBUTES") + 2
    click(app, source, index)
    old_target = destination(app, source, "SECURITY_ATTRIBUTES", "x86")
    pin_active_tab(app)
    app.command(1021)
    app.select(0, app.sources)
    app.command(1044 if reload else 1022)
    if reload:
        n.wait(lambda: n.is_visible(n.get_item(source, 1204)), "old snapshot hint", 45)
    else:
        n.wait(lambda: app.count(app.sources) == 0, "source unloaded", 45)
        app.pick([path])
        n.wait(lambda: app.count(app.sources) == 1, "source reopened", 45)
    app.command(1040)
    app.query("Windows.Win32.Security.SECURITY_ATTRIBUTES")
    current = app.open_row()
    assert current != old_target
    n.wait(lambda: "SECURITY_ATTRIBUTES" in app.content(current), "current target", 45)
    follow_caret(app, source, index)
    assert destination(app, source, "SECURITY_ATTRIBUTES", "x86") == old_target
    assert len(app.documents()) == 3


def test_identical_short_names_from_copied_sources_keep_their_identity(launch, settings):
    paths = [settings[2] / "navigation-first😀.winmd", settings[2] / "navigation-second.winmd"]
    for path in paths:
        shutil.copyfile(settings[1], path)
    app = launch(paths)
    app.query("Windows.Win32.Storage.FileSystem.CreateFileW")
    rows = app.named_rows("CreateFileW")
    assert len(rows) == 2
    sources = [app.open_row(row) for row in rows]
    assert sources[0] != sources[1]
    targets = []
    for source in sources:
        filename = n.window_text(n.get_item(source, 1205))
        assert any(path.name in filename for path in paths)
        follow_caret(app, source, reference(app, source, "SECURITY_ATTRIBUTES") + 2)
        target = destination(app, source, "SECURITY_ATTRIBUTES", "x86")
        assert n.window_text(n.get_item(target, 1205)) == filename
        pin_active_tab(app)
        targets.append(target)
    assert targets[0] != targets[1]
    assert len(app.documents()) == 4


def test_last_reference_glyph_and_trailing_line_space_have_precise_hit_bounds(launch, settings):
    app = launch([settings[1]])
    app.query("CreateFileW")
    source = app.open_row()
    editor = n.get_item(source, 1203)
    start = reference(app, source, "SECURITY_ATTRIBUTES")
    end = start + len("SECURITY_ATTRIBUTES")
    with n.window_dpi_context(app.frame):
        position(app, editor, start)
        last = character_point(app, editor, end - 1)
        after = character_point(app, editor, end)
        x1, x2 = last & 0xffff, after & 0xffff
        assert x2 > x1
        # Use the right half of the final glyph, not the next caret boundary.
        right_half = (last & ~0xffff) | (x1 + (x2 - x1) * 3 // 4)
        body = app.content(source).replace("\r\n", "\r")
        prefix = body[:body.index("SECURITY_ATTRIBUTES")]
        line_end = body.index("\r", len(prefix))
        line_end = len(body[:line_end].encode("utf-16-le")) // 2
        trailing = character_point(app, editor, line_end)
        trailing = (trailing & ~0xffff) | ((trailing & 0xffff) + (x2 - x1) * 3)
        for screen, navigate in ((trailing, False), (right_half, True)):
            point = n.w.POINT(n.c.c_short(screen & 0xffff).value,
                              n.c.c_short(screen >> 16).value)
            assert n.api(n.user, "ScreenToClient", n.w.BOOL, n.w.HWND,
                         n.c.POINTER(n.w.POINT))(editor, n.c.byref(point))
            _, _, width, height = n.rectangle(editor, client=True)
            assert 0 <= point.x < width and 0 <= point.y < height
            local = (point.x & 0xffff) | ((point.y & 0xffff) << 16)
            if navigate:
                hit = n.rich_edit_document(editor).RangeFromPoint(
                    n.c.c_short(screen & 0xffff).value, n.c.c_short(screen >> 16).value)
                assert hit.Start == end
                editor_selection(app, editor, (start, end))
                menu, items = popup(app, editor, screen, protocol_mouse=True)
                assert selection_packed(editor) == (end << 16) | start
                assert 2074 in items
                state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)
                assert not state(menu, 2074, 0) & 3
                dismiss(editor)
                n.send(editor, win32con.WM_NULL)
            win32gui.PostMessage(editor, win32con.WM_LBUTTONDOWN, win32con.MK_CONTROL | win32con.MK_LBUTTON, local)
            win32gui.PostMessage(editor, win32con.WM_LBUTTONUP, win32con.MK_CONTROL, local)
            n.send(app.frame, win32con.WM_NULL)
            if navigate:
                destination(app, source, "SECURITY_ATTRIBUTES", "x86")
            else:
                assert app.documents() == [source]
                _, items = popup(app, editor, screen, protocol_mouse=True)
                assert items == [2074]
                dismiss(editor)
                n.send(editor, win32con.WM_NULL)


@pytest.mark.parametrize("gesture", ["ctrl-click", "mouse-context-menu"])
@pytest.mark.interactive
def test_real_unfocused_scrolled_reference_navigates_on_first_gesture(launch, settings, gesture):
    import re
    import win32api
    from support.input_state import InputState
    from support.session import ThreadInfo

    with InputState() as inputs:
        app = launch([settings[1]])
        n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
        with real_foreground(app, inputs) as foreground:
            app.query("Windows.Win32.Graphics.Gdi.Apis")
            app.select(app.named_rows("Apis")[0])
            app.focus(app.results)
            assert foreground(), "real input requires foreground ownership"
            inputs.key(win32con.VK_SPACE)
            source = app.wait_document("Gdi preview", "Apis")
            n.wait(lambda: "HDC" in app.content(source), "loaded Gdi declaration", 45)
            editor = n.get_item(source, 1203)
            body = n.rich_edit_text(editor)
            text = body.replace("\r\n", "\r")
            assert len(text.encode("utf-16-le")) // 2 > 70000
            assert app.focused() == app.results
            assert editor_selection(app, editor) == (0, 0)
            assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == 0
            # A real Gdi handle reference well beyond the first screen, not the caret.
            match = next(match for match in re.finditer(r"\bHDC\b", text) if match.start() > 10000)
            index = len(text[:match.start()].encode("utf-16-le")) // 2 + 1
            line = n.send(editor, win32con.WM_USER + 54, 0, index)  # EM_EXLINEFROMCHAR
            assert line > 10
            rect = auto.ControlFromHandle(editor).BoundingRectangle
            auto.MoveTo((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
            auto.WheelDown(wheelTimes=max(1, (line - 3) // 3), interval=0, waitTime=0)
            def visible_reference():
                _, y = n.rich_edit_document(editor).Range(index, index + 1).GetPoint(32 | 256 | 512)
                height = n.rectangle(editor, client=True)[3]
                if 0 <= y < height:
                    return True
                if y < 0:
                    auto.WheelUp(wheelTimes=1, interval=0, waitTime=0)
                else:
                    auto.WheelDown(wheelTimes=1, interval=0, waitTime=0)
                return False
            n.wait(visible_reference, "scrolled reference visible", 45)
            top = n.send(editor, win32con.EM_GETFIRSTVISIBLELINE)
            assert top > 0
            with n.window_dpi_context(app.frame):
                screen = character_point(app, editor, index)
                point = (n.c.c_short(screen & 0xffff).value, n.c.c_short(screen >> 16).value)
                left, above, right, bottom = n.rectangle(editor)
                assert left <= point[0] < right and above <= point[1] < bottom
            assert editor_selection(app, editor) == (0, 0)
            assert app.focused() == app.results
            assert foreground(), "first link gesture requires foreground ownership"
            if gesture == "ctrl-click":
                inputs.state["held"].extend((win32con.VK_CONTROL, "mouse"))
                inputs.save()
                auto.PressKey(win32con.VK_CONTROL)
                try:
                    auto.MoveTo(*point)
                    auto.PressMouse(*auto.GetCursorPos())
                    def tracking():
                        info = ThreadInfo()
                        info.size = n.c.sizeof(info)
                        assert n.api(n.user, "GetGUIThreadInfo", n.w.BOOL, n.w.DWORD,
                                     n.c.POINTER(ThreadInfo))(n.get_pid(app.frame, None), n.c.byref(info))
                        return (app.focused() == editor and info.capture == editor
                                and win32api.GetAsyncKeyState(win32con.VK_CONTROL) & 0x8000
                                and win32api.GetAsyncKeyState(win32con.VK_LBUTTON) & 0x8000)
                    n.wait(tracking, "first Ctrl-click acquired editor focus and capture")
                    expected_selection = editor_selection(app, editor)
                    assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == top
                    assert foreground(), "Ctrl-click release requires foreground ownership"
                    auto.ReleaseMouse()
                    inputs.state["held"].remove("mouse")
                    inputs.save()
                    assert destination(app, source, "HDC", "x86") == source
                finally:
                    if "mouse" in inputs.state["held"]:
                        auto.ReleaseMouse()
                        inputs.state["held"].remove("mouse")
                    auto.ReleaseKey(win32con.VK_CONTROL)
                    inputs.state["held"].remove(win32con.VK_CONTROL)
                    inputs.save()
            else:
                auto.MoveTo(*point)
                # InputState recovery owns left mouse releases, so deliver the right pair together.
                auto.RightClick(*point)
                menu, _ = find_popup(app)
                def menu_positioned():
                    rect = n.w.RECT()
                    return (n.api(n.user, "GetMenuItemRect", n.w.BOOL, n.w.HWND, n.w.HMENU,
                                  n.w.UINT, n.c.POINTER(n.w.RECT))(
                                      app.frame, menu, 0, n.c.byref(rect))
                            and rect.right > rect.left and rect.bottom > rect.top)
                n.wait(menu_positioned, "reference menu positioned")
                expected_selection = editor_selection(app, editor)
                assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == top, (
                    "context menu must preserve the scrolled hit viewport", expected_selection, app.focused())
                assert foreground(), "context menu requires foreground ownership"
                inputs.key(win32con.VK_DOWN)
                inputs.key(win32con.VK_RETURN)
                wait_closed(editor)
            assert destination(app, source, "HDC", "x86") == source
            assert app.documents() == [source]
            assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))
            assert foreground(), "navigation retains foreground ownership"
            app.command(1055)
            n.wait(lambda: n.window_text(source).startswith("Apis -") and app.content(source) == body,
                   "Back restores Gdi declaration", 45)
            n.wait(lambda: editor_selection(app, editor) == expected_selection,
                   "Back restores source selection", 45)
            n.wait(lambda: n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == top,
                   "Back restores scrolled source top", 45)
            assert app.documents() == [source]
            assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))


@pytest.mark.interactive
def test_real_ctrl_click_and_drag_do_not_confuse_navigation_with_selection(launch, settings):
    import time
    import win32api
    import win32gui
    from support.input_state import InputState
    from support.session import ThreadInfo

    with InputState() as inputs:
        app = launch([settings[1]])
        n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
        auto.ControlFromHandle(app.frame).SetTopmost(True)
        try:
            auto.ControlFromHandle(app.frame).Restore()
            with n.window_dpi_context(app.frame):
                left, top, right, bottom = n.rectangle(app.search)
                inputs.click((left + right) // 2, (top + bottom) // 2)
            try:
                n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "foreground ownership")
            except AssertionError:
                foreground = win32gui.GetForegroundWindow()
                pytest.skip(f"physical input unverified: foreground acquisition failed: frame={app.frame}, foreground={foreground}, "
                            f"title={n.window_text(foreground)!r}, cursor={win32api.GetCursorPos()}, "
                            f"search={n.rectangle(app.search)}, hit={win32gui.WindowFromPoint(win32api.GetCursorPos())}")
            app.query("CreateFileW")
            app.select(0)
            app.focus(app.results)
            auto.SendKeys("{Space}")
            source = n.wait(lambda: (lambda doc: doc if doc in app.documents()
                            and n.window_text(doc).startswith("CreateFileW -") else None)(
                                app.active_document()), "source preview")
            editor = n.get_item(source, 1203)
            start = reference(app, source, "SECURITY_ATTRIBUTES")
            with n.window_dpi_context(app.frame):
                position(app, editor, start)
                first = character_point(app, editor, start + 1)
                last = character_point(app, editor, start + len("SECURITY_ATTRIBUTES") - 1)
                after = character_point(app, editor, start + len("SECURITY_ATTRIBUTES"))
                final_glyph = ((last & 0xffff) + ((after & 0xffff) - (last & 0xffff)) * 3 // 4,
                               n.c.c_short(last >> 16).value)
                first = (n.c.c_short(first & 0xffff).value, n.c.c_short(first >> 16).value)
                last = (n.c.c_short(last & 0xffff).value, n.c.c_short(last >> 16).value)
            def foreground(focus=editor):
                if win32gui.GetForegroundWindow() != app.frame:
                    pytest.skip("physical input unverified: application lost foreground ownership")
                if focus is not None:
                    assert app.focused() == focus
                return True

            def capture():
                info = ThreadInfo()
                info.size = n.c.sizeof(info)
                assert n.api(n.user, "GetGUIThreadInfo", n.w.BOOL, n.w.DWORD,
                             n.c.POINTER(ThreadInfo))(n.get_pid(app.frame, None), n.c.byref(info))
                return info.capture

            def hold(key):
                inputs.state["held"].append(key)
                inputs.save()
                if key == "mouse":
                    auto.PressMouse(*auto.GetCursorPos())
                else:
                    auto.PressKey(key)

            def release(key):
                if key == "mouse":
                    auto.ReleaseMouse()
                else:
                    auto.ReleaseKey(key)
                inputs.state["held"].remove(key)
                inputs.save()

            def press(point, control_editor=editor):
                auto.MoveTo(*point)
                hold(win32con.VK_CONTROL)
                hold("mouse")
                n.wait(lambda: foreground(None) and app.focused() == control_editor
                       and capture() == control_editor
                       and win32api.GetAsyncKeyState(win32con.VK_CONTROL) & 0x8000
                       and win32api.GetAsyncKeyState(win32con.VK_LBUTTON) & 0x8000,
                       "held Ctrl and mouse acknowledged by editor capture")

            def unpress():
                release("mouse")
                release(win32con.VK_CONTROL)
                n.wait(lambda: not capture()
                       and not win32api.GetAsyncKeyState(win32con.VK_LBUTTON) & 0x8000,
                       "physical release acknowledged")

            pin_active_tab(app)
            app.focus(app.search)
            foreground(app.search)
            press(final_glyph)
            unpress()
            target = destination(app, source, "SECURITY_ATTRIBUTES", "x86")
            assert sum(value.startswith(("Preview · ", "预览 · "))
                       for value in app.tab_descriptions()) == 1
            pin_active_tab(app)
            activate(app, source)
            app.focus(editor)
            with n.window_dpi_context(app.frame):
                far = character_point(app, editor, reference(app, source, "lpSecurityAttributes") + 3)
                far = (n.c.c_short(far & 0xffff).value, n.c.c_short(far >> 16).value)
            double_click_seconds = n.api(n.user, "GetDoubleClickTime", n.w.UINT)() / 1000
            for control, return_to_origin in ((False, False), (True, False), (True, True)):
                time.sleep(double_click_seconds + .05)
                foreground()
                editor_selection(app, editor, (start, start))
                auto.MoveTo(*first)
                if control:
                    hold(win32con.VK_CONTROL)
                hold("mouse")
                # Real RichEdit tracking consumes queued input; SendMessage drags do not model it.
                time.sleep(.1)
                foreground()
                auto.MoveTo(*(far if return_to_origin else last))
                n.wait(lambda: foreground() and (lambda value: value & 0xffff != value >> 16)(
                    selection_packed(editor)), "physical drag selects text")
                if return_to_origin:
                    n.wait(lambda: foreground() and selection_packed(editor) >> 16
                           > start + len("SECURITY_ATTRIBUTES"), "native drag extends beyond reference")
                    far_selection = selection_packed(editor)
                    assert capture() == editor
                    auto.MoveTo(*first)
                    n.wait(lambda: foreground() and selection_packed(editor) != far_selection,
                           "native drag processes return to original down point")
                time.sleep(.1)
                foreground()
                release("mouse")
                if control:
                    release(win32con.VK_CONTROL)
                time.sleep(.1)
                assert set(app.documents()) == {source, target}
                assert app.active_document() == source

            for cancel in ("cancelmode", "capture", "focus", "recolor", "architecture"):
                time.sleep(double_click_seconds + .05)
                app.focus(editor)
                foreground()
                editor_selection(app, editor, (start, start))
                press(first)
                if cancel == "cancelmode":
                    n.send(editor, win32con.WM_CANCELMODE)
                elif cancel == "capture":
                    n.send(editor, win32con.WM_CAPTURECHANGED, 0, app.search)
                elif cancel == "focus":
                    app.focus(app.search)
                elif cancel == "recolor":
                    n.send(app.frame, win32con.WM_SYSCOLORCHANGE)
                else:
                    app.choose(source, "x64")
                    n.wait(lambda: app.choice(source) == "x64", "updated architecture")
                foreground(app.search if cancel == "focus" else editor)
                unpress()
                time.sleep(.1)
                assert app.active_document() == source, cancel
                assert set(app.documents()) == {source, target}, cancel
            app.choose(source, "x86")

            def preview(query):
                held = capture()
                assert auto.ControlFromHandle(app.search).GetValuePattern().SetValue(query)
                n.wait(lambda: app.named_rows(query), "replacement search results", 45)
                app.select(app.named_rows(query)[0])
                assert capture() == held, "preview setup must preserve mouse capture"
                # Inject a preview update while the editor still owns mouse capture.
                win32gui.PostMessage(app.results, win32con.WM_KEYDOWN, win32con.VK_SPACE, 1)
                win32gui.PostMessage(app.results, win32con.WM_KEYUP, win32con.VK_SPACE, 0xc0000001)
                return n.wait(lambda: (lambda doc: doc if doc in app.documents()
                              and n.window_text(doc).startswith(query + " -") else None)(
                                  app.active_document()), "replacement preview")

            replacement = preview("CreateFileTransactedW")
            replacement_editor = n.get_item(replacement, 1203)
            replacement_start = reference(app, replacement, "SECURITY_ATTRIBUTES") + 3
            app.focus(replacement_editor)
            with n.window_dpi_context(app.frame):
                position(app, replacement_editor, replacement_start)
                screen = character_point(app, replacement_editor, replacement_start)
                replacement_point = (n.c.c_short(screen & 0xffff).value,
                                     n.c.c_short(screen >> 16).value)
            time.sleep(double_click_seconds + .05)
            press(replacement_point, replacement_editor)
            assert preview("CreateFileA") == replacement
            updated_start = reference(app, replacement, "SECURITY_ATTRIBUTES") + 3
            with n.window_dpi_context(app.frame):
                assert character_point(app, replacement_editor, updated_start) == screen
            foreground(replacement_editor)
            unpress()
            time.sleep(.1)
            assert app.active_document() == replacement
            assert set(app.documents()) == {source, target, replacement}
            time.sleep(double_click_seconds + .05)
            press(replacement_point, replacement_editor)
            unpress()
            assert destination(app, replacement, "SECURITY_ATTRIBUTES", "x86") == target

            activate(app, source)
            app.focus(editor)

            time.sleep(double_click_seconds + .05)
            foreground()
            editor_selection(app, editor, (start + 3, start + 3))
            selection = selection_packed(editor)
            hold(win32con.VK_SHIFT)
            inputs.key(win32con.VK_F10)
            release(win32con.VK_SHIFT)
            find_popup(app)
            assert selection_packed(editor) == selection
            foreground()
            inputs.key(win32con.VK_DOWN)
            inputs.key(win32con.VK_RETURN)
            wait_closed(editor)
            assert destination(app, source, "SECURITY_ATTRIBUTES", "x86") == target
        finally:
            auto.ControlFromHandle(app.frame).SetTopmost(False)

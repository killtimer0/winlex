"""Workspace history preserves targets, snapshot identity and reading positions."""
import shutil
import pytest
import win32con
import uiautomation as auto

from support import native as n
from test_context import find_popup, wait_closed
from test_navigation import editor_selection, follow_caret, reference


HISTORY_BACK = 1055
HISTORY_FORWARD = 1056


def enabled(app, command):
    _, _, menu = history_menu(app)
    state = n.api(n.user, "GetMenuState", n.w.UINT, n.w.HMENU, n.w.UINT, n.w.UINT)(
        menu, command, win32con.MF_BYCOMMAND)
    assert state != 0xffffffff, "history command exists in native menu"
    return not state & (win32con.MF_DISABLED | win32con.MF_GRAYED)


def history_menu(app):
    menu = n.api(n.user, "GetMenu", n.w.HMENU, n.w.HWND)(app.frame)
    count = n.api(n.user, "GetMenuItemCount", n.c.c_int, n.w.HMENU)
    submenu = n.api(n.user, "GetSubMenu", n.w.HMENU, n.w.HMENU, n.c.c_int)
    item = n.api(n.user, "GetMenuItemID", n.w.UINT, n.w.HMENU, n.c.c_int)
    for index in range(count(menu)):
        child = submenu(menu, index)
        if child and HISTORY_BACK in [item(child, i) for i in range(count(child))]:
            return menu, index, child
    raise AssertionError("native Navigate menu contains history commands")


def state(app, back, forward):
    n.wait(lambda: (enabled(app, HISTORY_BACK), enabled(app, HISTORY_FORWARD)) == (back, forward),
           "history menu availability")


def current(app, name, architecture=None, document=None, loaded=True):
    def match():
        doc = app.active_document()
        if doc not in app.documents() or (document is not None and doc != document):
            return None
        if not n.window_text(doc).startswith(name + " - "):
            return None
        if architecture is not None and app.choice(doc) != architecture:
            return None
        return doc
    doc = n.wait(match, "history target " + name)
    if loaded:
        n.wait(lambda: name in app.content(doc), "history declaration " + name, 45)
    return doc


def visit(app, name, pin=False):
    app.query(name)
    rows = app.named_rows(name.rsplit(".", 1)[-1])
    assert rows, name
    app.focus(app.results)
    if pin:
        doc = app.open_row(rows[0])
    else:
        doc = app.preview_row(rows[0])
    current(app, name.rsplit(".", 1)[-1], document=doc)
    if not pin:
        n.wait(lambda: app.focused() == app.results, "preview preserves result focus")
    return doc


def replay(app, command, name, architecture=None, document=None):
    app.command(command)
    doc = current(app, name, architecture, document)
    n.wait(lambda: app.focused() == n.get_item(doc, 1203), "history focuses editor")
    return doc


def close(app, doc):
    assert auto.ControlFromHandle(doc).GetPattern(auto.PatternId.WindowPattern).Close(waitTime=0)
    n.wait(lambda: doc not in app.documents(), "historical document closed")


def test_preview_history_reuses_one_document_and_new_visit_branches(launch, settings):
    app = launch([settings[1]])
    state(app, False, False)
    preview = visit(app, "CreateFileW")
    state(app, False, False)
    assert visit(app, "CloseHandle") == preview
    assert visit(app, "DeleteFileW") == preview
    state(app, True, False)
    search = n.window_text(app.search)
    for command, name, back, forward in (
            (HISTORY_BACK, "CloseHandle", True, True),
            (HISTORY_BACK, "CreateFileW", False, True),
            (HISTORY_FORWARD, "CloseHandle", True, True),
            (HISTORY_FORWARD, "DeleteFileW", True, False),
            (HISTORY_BACK, "CloseHandle", True, True)):
        replay(app, command, name, document=preview)
        state(app, back, forward)
        assert app.documents() == [preview]
        assert n.window_text(app.search) == search
    assert visit(app, "GetLastError") == preview
    state(app, True, False)
    replay(app, HISTORY_BACK, "CloseHandle", document=preview)
    replay(app, HISTORY_FORWARD, "GetLastError", document=preview)
    state(app, True, False)


def test_pin_is_not_a_visit_and_pins_are_not_rebound(launch, settings):
    app = launch([settings[1]])
    pinned = visit(app, "CreateFileW")
    assert app.open_row(app.named_rows("CreateFileW")[0]) == pinned
    state(app, False, False)
    original = app.content(pinned)
    preview = visit(app, "CloseHandle")
    assert preview != pinned
    assert visit(app, "DeleteFileW") == preview
    replay(app, HISTORY_BACK, "CloseHandle", document=preview)
    replay(app, HISTORY_BACK, "CreateFileW", document=pinned)
    state(app, False, True)
    replay(app, HISTORY_FORWARD, "CloseHandle", document=preview)
    replay(app, HISTORY_FORWARD, "DeleteFileW", document=preview)
    assert app.content(pinned) == original
    assert set(app.documents()) == {pinned, preview}
    assert sum(text.startswith(("Preview · ", "预览 · "))
               for text in app.tab_descriptions()) == 1


def test_architecture_visits_restore_without_writing_preference(launch, settings, profile):
    app = launch([settings[1]])
    doc = visit(app, "Windows.Win32.Security.SECURITY_ATTRIBUTES")
    app.choose(doc, "x64")
    n.wait(lambda: "24 bytes" in app.content(doc), "x64 declaration", 45)
    assert profile.get("PreferredArchitecture") == "x64"
    replay(app, HISTORY_BACK, "SECURITY_ATTRIBUTES", "x86", doc)
    n.wait(lambda: "12 bytes" in app.content(doc), "restored x86 declaration", 45)
    assert profile.get("PreferredArchitecture") == "x64"
    state(app, False, True)
    replay(app, HISTORY_FORWARD, "SECURITY_ATTRIBUTES", "x64", doc)
    assert profile.get("PreferredArchitecture") == "x64"
    state(app, True, False)


def test_duplicate_documents_replay_exact_original_and_architecture(launch, settings):
    app = launch([settings[1]])
    first = visit(app, "CreateFileW", pin=True)
    duplicate = app.open_new(app.named_rows("CreateFileW")[0])
    assert duplicate != first
    current(app, "CreateFileW", document=duplicate)
    app.choose(duplicate, "x64")
    n.wait(lambda: app.choice(duplicate) == "x64", "duplicate architecture")
    last = visit(app, "CloseHandle", pin=True)
    replay(app, HISTORY_BACK, "CreateFileW", "x64", duplicate)
    replay(app, HISTORY_BACK, "CreateFileW", "x86", duplicate)
    replay(app, HISTORY_BACK, "CreateFileW", "x86", first)
    state(app, False, True)
    assert set(app.documents()) == {first, duplicate, last}


@pytest.mark.parametrize("pinned", [False, True], ids=["preview-source", "fixed-source"])
def test_reference_history_reuses_preview_and_preserves_explicit_pins(launch, settings, pinned):
    app = launch([settings[1]])
    source = visit(app, "CreateFileW", pin=pinned)
    follow_caret(app, source, reference(app, source, "SECURITY_ATTRIBUTES") + 2)
    target = current(app, "SECURITY_ATTRIBUTES", "x86")
    assert (target != source) == pinned
    expected = {source, target}
    follow_caret(app, target, reference(app, target, "BOOL") + 1)
    current(app, "BOOL", "x86", target)
    assert set(app.documents()) == expected
    assert sum(text.startswith(("Preview · ", "预览 · "))
               for text in app.tab_descriptions()) == 1
    replay(app, HISTORY_BACK, "SECURITY_ATTRIBUTES", document=target)
    replay(app, HISTORY_BACK, "CreateFileW", document=source)
    state(app, False, True)
    replay(app, HISTORY_FORWARD, "SECURITY_ATTRIBUTES", document=target)
    replay(app, HISTORY_FORWARD, "BOOL", document=target)
    state(app, True, False)
    assert set(app.documents()) == expected
    assert sum(text.startswith(("Preview · ", "预览 · "))
               for text in app.tab_descriptions()) == 1
    if pinned:
        assert "CreateFileW(" in app.content(source)


@pytest.mark.parametrize("activation", ["mdi", "tab"])
def test_manual_activation_records_but_close_repositions_without_extra_visit(
        launch, settings, activation):
    app = launch([settings[1]])
    first = visit(app, "CreateFileW", pin=True)
    second = visit(app, "CloseHandle", pin=True)
    third = visit(app, "DeleteFileW", pin=True)
    if activation == "mdi":
        app.focus(first)
    else:
        assert app._tabs()[0].GetSelectionItemPattern().Select(waitTime=0)
    current(app, "CreateFileW", document=first)
    replay(app, HISTORY_BACK, "DeleteFileW", document=third)
    replay(app, HISTORY_FORWARD, "CreateFileW", document=first)
    close(app, first)
    survivor = n.wait(lambda: app.active_document() if app.active_document() in
                      (second, third) else None, "automatic surviving activation")
    if survivor == third:
        replay(app, HISTORY_BACK, "CloseHandle", document=second)
    else:
        replay(app, HISTORY_BACK, "CreateFileW")
        # The older first visit is recreated, not a synthetic activation of third.
        assert app.active_document() != survivor


def test_closed_historical_document_recreated_after_source_unload(launch, settings):
    path = settings[2] / "history-unloaded-deleted😀.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    first = visit(app, "CreateFileW", pin=True)
    keeper = visit(app, "CloseHandle", pin=True)
    close(app, first)
    app.command(1021)
    app.select(0, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0, "source unloaded", 45)
    path.unlink()
    assert not path.exists()
    app.command(1040)
    recreated = replay(app, HISTORY_BACK, "CreateFileW")
    assert recreated != keeper and keeper in app.documents()
    assert any(text.startswith(("Preview · ", "预览 · "))
               for text in app.tab_descriptions())
    replay(app, HISTORY_FORWARD, "CloseHandle", document=keeper)


def test_close_prefers_previous_visit_over_closer_future_match(launch, settings):
    app = launch([settings[1]])
    repeated = visit(app, "CloseHandle", pin=True)  # B
    first = visit(app, "CreateFileW", pin=True)  # A
    visit(app, "DeleteFileW", pin=True)  # C
    closing = visit(app, "GetLastError", pin=True)  # D
    tabs = n.get_item(app.frame, 1104)
    labels = app.tab_labels()
    pages = app._tabs()
    points = []
    left, top, _, _ = n.rectangle(tabs)
    for page in (pages[0], pages[-1]):
        rect = page.BoundingRectangle
        x = rect.left - left + min(20, (rect.right - rect.left) // 2)
        y = (rect.top + rect.bottom) // 2 - top
        points.append((x & 0xffff) | ((y & 0xffff) << 16))
    n.send(tabs, win32con.WM_LBUTTONDOWN, win32con.MK_LBUTTON, points[0])
    n.send(tabs, win32con.WM_MOUSEMOVE, win32con.MK_LBUTTON, points[1])
    n.send(tabs, win32con.WM_LBUTTONUP, 0, points[1])
    n.wait(lambda: app.tab_labels() == labels[1:] + labels[:1], "B reordered after D")
    current(app, "CloseHandle", document=repeated)  # B again
    visit(app, "SetLastError", pin=True)  # E; history is B,A,C,D,B,E.
    replay(app, HISTORY_BACK, "CloseHandle", document=repeated)
    replay(app, HISTORY_BACK, "GetLastError", document=closing)
    close(app, closing)
    current(app, "CloseHandle", document=repeated)
    state(app, False, True)
    replay(app, HISTORY_FORWARD, "CreateFileW", document=first)


def test_back_from_empty_workspace_restores_latest_valid_visit(launch, settings):
    app = launch([settings[1]])
    doc = visit(app, "CreateFileW")
    assert visit(app, "CloseHandle") == doc
    close(app, doc)
    assert not app.documents()
    state(app, True, False)
    restored = replay(app, HISTORY_BACK, "CloseHandle")
    assert app.documents() == [restored]
    assert app.tab_descriptions()[0].startswith(("Preview · ", "预览 · "))
    replay(app, HISTORY_BACK, "CreateFileW", document=restored)
    state(app, False, True)


def test_expired_history_is_skipped_not_substituted_with_reloaded_source(launch, settings):
    path = settings[2] / "history-expiry😀.winmd"
    shutil.copyfile(settings[1], path)
    app = launch([path])
    old = visit(app, "CreateFileW")
    assert visit(app, "CloseHandle") == old
    assert app.open_row(app.named_rows("CloseHandle")[0]) == old
    app.command(1021)
    app.select(0, app.sources)
    app.command(1044)
    n.wait(lambda: n.is_visible(n.get_item(old, 1204)), "reloaded old snapshot", 45)
    app.command(1040)
    latest = visit(app, "DeleteFileW", pin=True)
    assert latest != old
    close(app, old)
    # Completed public searches/renders drain earlier load work; no private Arc timing.
    app.query("GetLastError")
    app.query("DeleteFileW")
    current(app, "DeleteFileW", document=latest)
    app.command(HISTORY_BACK)
    assert app.active_document() == latest and app.documents() == [latest]
    state(app, False, False)


def selection(app, editor, value=None):
    return editor_selection(app, editor, value)


def test_long_declaration_restores_full_utf16_selection_and_top_anchor(launch, settings):
    app = launch([settings[1]])
    doc = visit(app, "Windows.Win32.Graphics.Gdi.Apis")
    editor = n.get_item(doc, 1203)
    length = n.rich_edit_document(editor).Range(0, 0).StoryLength - 1
    assert length > 70000, "fixture must exercise selection beyond 16-bit offsets"
    wanted = (66000, 66023)
    app.focus(editor)
    assert selection(app, editor, wanted) == wanted
    assert auto.ControlFromHandle(editor).GetPattern(auto.PatternId.ScrollPattern).SetScrollPercent(-1, 75)
    top = n.send(editor, win32con.EM_GETFIRSTVISIBLELINE)
    assert top > 0
    assert visit(app, "CloseHandle") == doc
    replay(app, HISTORY_BACK, "Apis", document=doc)
    n.wait(lambda: selection(app, editor) == wanted, "full UTF-16 selection restored", 45)
    n.wait(lambda: n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == top,
           "long declaration top anchor restored", 45)
    replay(app, HISTORY_FORWARD, "CloseHandle", document=doc)
    replay(app, HISTORY_BACK, "Apis", document=doc)
    assert selection(app, editor) == wanted
    assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == top


def test_rapid_history_replay_does_not_apply_stale_restore_to_final_target(launch, settings):
    app = launch([settings[1]])
    doc = visit(app, "Windows.Win32.Graphics.Gdi.Apis")
    editor = n.get_item(doc, 1203)
    app.focus(editor)
    selection(app, editor, (66000, 66023))
    n.send(editor, win32con.EM_SCROLLCARET)
    assert visit(app, "CloseHandle") == doc
    selection(app, editor, (3, 9))
    loading = ("Loading declaration...", "正在加载声明…")
    pending_modifiers = 0
    # Send replay and modifier gestures without waiting for intermediate loads.
    for _ in range(12):
        for command in (HISTORY_BACK, HISTORY_FORWARD):
            app.command(command)
            pending = app.content(doc).strip() in loading
            for message, key in ((win32con.WM_SYSKEYDOWN, win32con.VK_MENU),
                                 (win32con.WM_KEYDOWN, win32con.VK_CONTROL),
                                 (win32con.WM_KEYDOWN, win32con.VK_SHIFT)):
                n.send(editor, message, key, 1)
                n.send(editor, win32con.WM_KEYUP, key, 0xc0000001)
            if pending and app.content(doc).strip() in loading:
                pending_modifiers += 1
    assert pending_modifiers, "modifier delivery must be observed during public loading state"
    # Finish on a different target so the pre-burst body cannot satisfy the wait.
    app.command(HISTORY_BACK)
    current(app, "Apis", document=doc)
    n.wait(lambda: selection(app, editor) == (66000, 66023), "older target keeps its own selection", 45)
    state(app, False, True)
    replay(app, HISTORY_FORWARD, "CloseHandle", document=doc)
    n.wait(lambda: selection(app, editor) == (3, 9), "latest target selection survives burst", 45)
    assert n.send(editor, win32con.EM_GETFIRSTVISIBLELINE) == 0
    state(app, True, False)


def test_appcommand_routes_from_editor_and_search(launch, settings):
    app = launch([settings[1]])
    doc = visit(app, "CreateFileW")
    assert visit(app, "CloseHandle") == doc
    editor = n.get_item(doc, 1203)
    n.send(editor, 0x0319, editor, 1 << 16)  # WM_APPCOMMAND, browser back
    current(app, "CreateFileW", document=doc)
    n.wait(lambda: app.focused() == editor, "appcommand back focuses editor")
    app.focus(app.search)
    n.send(app.search, 0x0319, app.search, 2 << 16)
    current(app, "CloseHandle", document=doc)
    n.wait(lambda: app.focused() == editor, "appcommand forward focuses editor")


@pytest.mark.parametrize("gesture", ["alt-arrows", "mouse-side-buttons", "menu"])
@pytest.mark.interactive
def test_real_foreground_history_gestures(launch, settings, gesture):
    import win32gui

    app = launch([settings[1]])
    doc = visit(app, "CreateFileW")
    assert visit(app, "CloseHandle") == doc
    editor = n.get_item(doc, 1203)
    for command, name in ((HISTORY_BACK, "CreateFileW"),
                          (HISTORY_FORWARD, "CloseHandle")):
        app.focus(app.search)
        assert win32gui.GetForegroundWindow() == app.frame, "real history input requires foreground ownership"
        if gesture == "alt-arrows":
            auto.SendKeys("{Alt}{Left}" if command == HISTORY_BACK else "{Alt}{Right}")
        elif gesture == "menu":
            menu, index, _ = history_menu(app)
            rect = n.w.RECT()
            with n.window_dpi_context(app.frame):
                assert n.api(n.user, "GetMenuItemRect", n.w.BOOL, n.w.HWND, n.w.HMENU,
                             n.w.UINT, n.c.POINTER(n.w.RECT))(app.frame, menu, index, n.c.byref(rect))
                auto.Click((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
            popup_menu, items = find_popup(app)
            assert popup_menu and command in items
            with n.window_dpi_context(app.frame):
                assert n.api(n.user, "GetMenuItemRect", n.w.BOOL, n.w.HWND, n.w.HMENU,
                             n.w.UINT, n.c.POINTER(n.w.RECT))(
                                 app.frame, popup_menu, items.index(command), n.c.byref(rect))
                assert rect.right > rect.left and rect.bottom > rect.top
                auto.Click((rect.left + rect.right) // 2, (rect.top + rect.bottom) // 2)
            wait_closed(editor)
        else:
            # Native X-button input lets DefWindowProc generate WM_APPCOMMAND.
            with n.window_dpi_context(app.frame):
                left, top, right, bottom = n.rectangle(editor)
                auto.MoveTo((left + right) // 2, (top + bottom) // 2)
            side = 1 if command == HISTORY_BACK else 2
            assert auto.SendInput(auto.MouseInput(0, 0, side, auto.MouseEventFlag.XDown)) == 1
            assert auto.SendInput(auto.MouseInput(0, 0, side, auto.MouseEventFlag.XUp)) == 1
        current(app, name, document=doc)
        n.wait(lambda: app.focused() == editor, "real gesture focuses history editor")
        assert win32gui.GetForegroundWindow() == app.frame

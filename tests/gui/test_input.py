"""Actual mouse/keyboard delivery, deliberately separate from message checks."""
import pytest
import win32con
import win32gui
from support import native as n
from support.input_state import InputState


def test_real_search_keyboard(request, launch, settings):
    if not request.config.getoption("--real-input"):
        pytest.skip("actual input not requested")
    with InputState() as input_state:
        app = launch([settings[1]])
        n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
        # Expose our own window, then let a real mouse click obtain foreground
        # ownership (SetForegroundWindow may be rejected by the foreground lock).
        win32gui.SetWindowPos(app.frame, win32con.HWND_TOPMOST, 0, 0, 0, 0,
                              win32con.SWP_NOMOVE | win32con.SWP_NOSIZE | win32con.SWP_NOACTIVATE)
        try:
            with n.window_dpi_context(app.frame):
                left, top, right, bottom = n.rectangle(app.search)
                input_state.click((left+right)//2, (top+bottom)//2)
            n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "foreground ownership")
            n.wait(lambda: app.focused() == app.search, "search input focus")
        finally:
            win32gui.SetWindowPos(app.frame, win32con.HWND_NOTOPMOST, 0, 0, 0, 0,
                                  win32con.SWP_NOMOVE | win32con.SWP_NOSIZE | win32con.SWP_NOACTIVATE)
        input_state.disable_ime(app.search)
        for character in "CREATEFILEW":
            input_state.key(ord(character))
        n.wait(lambda: n.window_text(app.search).casefold() == "createfilew", "real input delivered")
        n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "real-input search completed", 45)
        assert "CreateFileW" in app.cell(0)
        mode = n.get_item(app.frame, 1114)
        input_state.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == mode, "real Tab reaches mode")
        input_state.key(win32con.VK_HOME)
        input_state.key(win32con.VK_DOWN)
        input_state.key(win32con.VK_RETURN)
        n.wait(lambda: n.send(mode, win32con.CB_GETCURSEL) == 1, "real keyboard selects condition mode")
        n.wait(lambda: "condition" in app.status_text() or "条件" in app.status_text(), "condition diagnostic", 45)
        assert app.count() == 0
        input_state.key(win32con.VK_HOME)
        input_state.key(win32con.VK_RETURN)
        n.wait(lambda: app.status_text().startswith(("Results ", "共 ")), "quick search restored", 45)
        assert app.count() > 0
        input_state.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == app.results, "real Tab reaches results")

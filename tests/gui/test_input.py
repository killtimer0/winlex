"""Actual mouse/keyboard delivery, deliberately separate from message checks."""
import pytest
import win32api
import win32con
import win32gui
import win32service
from support import native as n
from support.input_state import InputState


def test_real_search_keyboard(request, launch, settings):
    if not request.config.getoption("--real-input"):
        pytest.skip("actual input not requested")
    current = win32service.GetThreadDesktop(win32api.GetCurrentThreadId())
    active = win32service.OpenInputDesktop(0, False, win32con.DESKTOP_READOBJECTS)
    try:
        assert win32service.GetUserObjectInformation(current, win32con.UOI_NAME) == win32service.GetUserObjectInformation(active, win32con.UOI_NAME), "actual input requires the input desktop"
    finally:
        active.CloseDesktop()
    with InputState() as input_state:
        app = launch(["--winmd", settings[1]])
        n.wait(lambda: app.status_text().startswith(("Ready.", "已就绪")), "loaded", 45)
        # Expose our own window, then let a real mouse click obtain foreground
        # ownership (SetForegroundWindow may be rejected by the foreground lock).
        win32gui.SetWindowPos(app.frame, win32con.HWND_TOPMOST, 0, 0, 0, 0,
                              win32con.SWP_NOMOVE | win32con.SWP_NOSIZE | win32con.SWP_NOACTIVATE)
        try:
            left, top, right, bottom = n.rectangle(app.search)
            input_state.click((left+right)//2, (top+bottom)//2)
            n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "foreground ownership")
        finally:
            win32gui.SetWindowPos(app.frame, win32con.HWND_NOTOPMOST, 0, 0, 0, 0,
                                  win32con.SWP_NOMOVE | win32con.SWP_NOSIZE | win32con.SWP_NOACTIVATE)
        for character in "CREATEFILEW":
            input_state.key(ord(character))
        n.wait(lambda: app.count() > 0, "real-input search", 45)
        assert "CreateFileW" in app.cell(0)

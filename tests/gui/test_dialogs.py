import pytest
import win32con
from support import native as n


@pytest.mark.parametrize("command", [1020, 1041])
def test_native_picker_cancel_keeps_application_state(launch, profile, command):
    app = launch()
    app.command(1021)  # sources panel owns both picker commands
    owner = n.api(n.user, "GetWindow", n.w.HWND, n.w.HWND, n.w.UINT)
    enabled = n.api(n.user, "IsWindowEnabled", n.w.BOOL, n.w.HWND)

    def dialogs():
        return [h for h in n.windows() if n.class_name(h) == "#32770"
                and owner(h, 4) == app.frame and n.is_visible(h)]

    assert n.post(app.frame, win32con.WM_COMMAND, command, 0)
    dialog = n.wait(lambda: next(iter(dialogs()), None), "owned system picker")
    assert not enabled(app.frame)
    assert n.post(dialog, win32con.WM_COMMAND, win32con.IDCANCEL, 0)
    n.wait(lambda: not dialogs() and enabled(app.frame), "picker cancellation")
    assert not app.documents()
    assert profile.get("Files") is None
    assert profile.get("Directories") is None
    n.send(app.frame, win32con.WM_NULL)  # the owner remains responsive; no error or fallback dialog

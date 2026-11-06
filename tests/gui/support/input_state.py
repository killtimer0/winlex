"""Persist owned input changes before delivery so the outer runner can undo them."""
import json
import os
from contextlib import contextmanager
from pathlib import Path
import uiautomation as auto
import win32api
import win32con
import win32gui
import win32service
from . import native as n

IMC_GETOPENSTATUS = 5
IMC_SETOPENSTATUS = 6
GA_ROOT = 2
get_ancestor = n.api(n.user, "GetAncestor", n.w.HWND, n.w.HWND, n.w.UINT)


@contextmanager
def foreground(app, inputs):
    window = auto.ControlFromHandle(app.frame)
    topmost = window.IsTopmost()
    window.SetTopmost(True)
    try:
        window.Restore()
        window.TitleBarControl().Click(simulateMove=False)
        n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "foreground input")
        yield lambda: win32gui.GetForegroundWindow() == app.frame
    finally:
        window.SetTopmost(topmost)


def ime_window(hwnd):
    return n.api(n.c.WinDLL("imm32"), "ImmGetDefaultIMEWnd", n.w.HWND, n.w.HWND)(hwnd)


def recover(path):
    if not path.exists():
        return
    state = json.loads(path.read_text(encoding="utf-8"))
    for key in state["held"]:
        if key == "mouse":
            auto.ReleaseMouse(waitTime=0)
        else:
            auto.ReleaseKey(key, waitTime=0)
    for key in state.get("released", []):
        if auto.IsKeyPressed(key):
            auto.ReleaseKey(key, waitTime=0)
    releases = {win32con.VK_LBUTTON: auto.ReleaseMouse,
                win32con.VK_RBUTTON: auto.RightReleaseMouse,
                win32con.VK_MBUTTON: auto.MiddleReleaseMouse}
    for key in state.get("released_mouse", []):
        if auto.IsKeyPressed(key):
            releases[key](waitTime=0)
    for button in state.get("released_x", []):
        if auto.IsKeyPressed(auto.Keys.VK_XBUTTON1 + button - 1):
            assert auto.SendInput(auto.MouseInput(0, 0, button, auto.MouseEventFlag.XUp)) == 1
    ime = state.get("ime")
    if ime and n.is_window(ime["control"]):
        pid = n.w.DWORD()
        n.get_pid(ime["control"], n.c.byref(pid))
        if pid.value == ime["pid"] and ime_window(ime["control"]) == ime["window"]:
            n.send(ime["window"], win32con.WM_IME_CONTROL, IMC_SETOPENSTATUS, ime["open"])
            n.wait(lambda: n.send(ime["window"], win32con.WM_IME_CONTROL, IMC_GETOPENSTATUS) == ime["open"],
                   "IME state restored")
    foreground = state["foreground"]
    try:
        if foreground and win32gui.IsWindow(foreground):
            foreground = get_ancestor(foreground, GA_ROOT)
            assert foreground, "Cannot resolve previous foreground root"
        if foreground and win32gui.IsWindow(foreground) and win32gui.GetForegroundWindow() != foreground:
            window = auto.ControlFromHandle(foreground)
            alt_pressed = auto.IsKeyPressed(auto.Keys.VK_MENU)
            if not alt_pressed:
                auto.PressKey(auto.Keys.VK_MENU, waitTime=0)
            try:
                auto.SetForegroundWindow(window.NativeWindowHandle)
            finally:
                if not alt_pressed:
                    auto.ReleaseKey(auto.Keys.VK_MENU, waitTime=0)
            n.wait(lambda: win32gui.GetForegroundWindow() == foreground, "foreground restored")
    finally:
        auto.MoveTo(*state["cursor"], waitTime=0)
    path.unlink()


class InputState:
    def __init__(self, path=None):
        root = os.environ.get("WINLEX_TEST_LEDGER")
        if path is None and (not root or os.environ.get("WINLEX_TEST_REAL_INPUT") != "1"):
            raise RuntimeError("Actual-input tests require tests/gui/run.py cleanup ownership")
        self.path = Path(path) if path is not None else Path(root) / "input.json"
        self.path.parent.mkdir(parents=True, exist_ok=True)
        keys = range(8, 256)
        raw_foreground = win32gui.GetForegroundWindow()
        foreground = get_ancestor(raw_foreground, GA_ROOT) if raw_foreground else 0
        assert not raw_foreground or foreground, "Cannot resolve foreground root"
        self.state = {"held": [], "cursor": auto.GetCursorPos(),
                      "foreground": foreground, "raw_foreground": raw_foreground,
                      "released": [key for key in keys if not auto.IsKeyPressed(key)],
                      "released_mouse": [key for key in (win32con.VK_LBUTTON, win32con.VK_RBUTTON,
                                                         win32con.VK_MBUTTON) if not auto.IsKeyPressed(key)],
                      "released_x": [button for button in (1, 2)
                                     if not auto.IsKeyPressed(auto.Keys.VK_XBUTTON1 + button - 1)]}

    def save(self):
        temporary = self.path.with_suffix(".new")
        temporary.write_text(json.dumps(self.state), encoding="utf-8")
        temporary.replace(self.path)

    def __enter__(self):
        current = win32service.GetThreadDesktop(win32api.GetCurrentThreadId())
        active = win32service.OpenInputDesktop(0, False, win32con.DESKTOP_READOBJECTS)
        try:
            assert (
                win32service.GetUserObjectInformation(int(current), win32con.UOI_NAME)
                == win32service.GetUserObjectInformation(int(active), win32con.UOI_NAME)
            ), "actual input requires the input desktop"
        finally:
            active.CloseDesktop()
        self.save()
        return self

    def click(self, x, y):
        self.state["held"].append("mouse")
        self.save()
        auto.Click(x, y)
        self.state["held"].remove("mouse")
        self.save()

    def double_click(self, x, y):
        self.state["held"].append("mouse")
        self.save()
        try:
            auto.Click(x, y, waitTime=0)
            auto.Click(x, y)
        except BaseException:
            auto.ReleaseMouse(waitTime=0)
            raise
        finally:
            self.state["held"].remove("mouse")
            self.save()

    def key(self, code):
        self.state["held"].append(code)
        self.save()
        auto.PressKey(code)
        auto.ReleaseKey(code)
        self.state["held"].remove(code)
        self.save()

    def disable_ime(self, control):
        window = ime_window(control)
        if not window:
            return
        opened = n.send(window, win32con.WM_IME_CONTROL, IMC_GETOPENSTATUS)
        if not opened:
            return
        pid = n.w.DWORD()
        n.get_pid(control, n.c.byref(pid))
        self.state["ime"] = {"control": control, "window": window, "pid": pid.value, "open": opened}
        self.save()
        n.send(window, win32con.WM_IME_CONTROL, IMC_SETOPENSTATUS, 0)
        n.wait(lambda: n.send(window, win32con.WM_IME_CONTROL, IMC_GETOPENSTATUS) == 0, "IME closed")

    def __exit__(self, *args):
        recover(self.path)

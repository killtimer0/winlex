"""Persist owned input changes before delivery so the outer runner can undo them."""
import json
import logging
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
            if not foreground:
                raise RuntimeError("Cannot resolve previous foreground root")
        if foreground and win32gui.IsWindow(foreground) and win32gui.GetForegroundWindow() != foreground:
            # Windows may deny foreground activation. Do not inject keys or wait
            # for permission: restoring the user's foreground is best effort.
            win32gui.SetForegroundWindow(foreground)
            if win32gui.GetForegroundWindow() != foreground:
                logging.getLogger(__name__).warning("Could not restore previous foreground window %s", foreground)
    except Exception as error:
        logging.getLogger(__name__).warning("Could not restore previous foreground window: %s", error)
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
        raw_foreground = win32gui.GetForegroundWindow()
        foreground = get_ancestor(raw_foreground, GA_ROOT) if raw_foreground else 0
        assert not raw_foreground or foreground, "Cannot resolve foreground root"
        # Recovery releases only inputs explicitly recorded before delivery.
        self.state = {"held": [], "cursor": auto.GetCursorPos(),
                      "foreground": foreground, "raw_foreground": raw_foreground}

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
        # A key() is a tap, not a held key. The provider's default 500 ms pause
        # occurs before key-up and can cross the Windows auto-repeat threshold.
        auto.PressKey(code, waitTime=0)
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

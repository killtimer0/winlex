"""Persist owned input changes before delivery so the outer runner can undo them."""
import json
import os
from pathlib import Path
import win32api
import win32con
import win32gui
import win32service
from . import native as n

IMC_GETOPENSTATUS = 5
IMC_SETOPENSTATUS = 6


def ime_window(hwnd):
    return n.api(n.c.WinDLL("imm32"), "ImmGetDefaultIMEWnd", n.w.HWND, n.w.HWND)(hwnd)


def recover(path):
    if not path.exists():
        return
    state = json.loads(path.read_text(encoding="utf-8"))
    for key in state["held"]:
        if key == "mouse":
            win32api.mouse_event(win32con.MOUSEEVENTF_LEFTUP, 0, 0)
        else:
            win32api.keybd_event(key, 0, win32con.KEYEVENTF_KEYUP, 0)
    ime = state.get("ime")
    if ime and n.is_window(ime["control"]):
        pid = n.w.DWORD()
        n.get_pid(ime["control"], n.c.byref(pid))
        if pid.value == ime["pid"] and ime_window(ime["control"]) == ime["window"]:
            n.send(ime["window"], win32con.WM_IME_CONTROL, IMC_SETOPENSTATUS, ime["open"])
            n.wait(lambda: n.send(ime["window"], win32con.WM_IME_CONTROL, IMC_GETOPENSTATUS) == ime["open"],
                   "IME state restored")
    win32api.SetCursorPos(tuple(state["cursor"]))
    foreground = state["foreground"]
    if foreground and win32gui.IsWindow(foreground):
        win32gui.SetForegroundWindow(foreground)
    path.unlink()


class InputState:
    def __init__(self):
        root = os.environ.get("WINLEX_TEST_LEDGER")
        if not root:
            raise RuntimeError("Actual-input tests require tests/gui/run.py cleanup ownership")
        self.path = Path(root) / "input.json"
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.state = {"held": [], "cursor": win32api.GetCursorPos(), "foreground": win32gui.GetForegroundWindow()}

    def save(self):
        temporary = self.path.with_suffix(".new")
        temporary.write_text(json.dumps(self.state), encoding="utf-8")
        temporary.replace(self.path)

    def __enter__(self):
        current = win32service.GetThreadDesktop(win32api.GetCurrentThreadId())
        active = win32service.OpenInputDesktop(0, False, win32con.DESKTOP_READOBJECTS)
        try:
            assert win32service.GetUserObjectInformation(int(current), win32con.UOI_NAME) == win32service.GetUserObjectInformation(int(active), win32con.UOI_NAME), "actual input requires the input desktop"
        finally:
            active.CloseDesktop()
        self.save()
        return self

    def click(self, x, y):
        win32api.SetCursorPos((x, y))
        self.state["held"].append("mouse")
        self.save()
        win32api.mouse_event(win32con.MOUSEEVENTF_LEFTDOWN, 0, 0)
        win32api.mouse_event(win32con.MOUSEEVENTF_LEFTUP, 0, 0)
        self.state["held"].remove("mouse")
        self.save()

    def key(self, code):
        self.state["held"].append(code)
        self.save()
        win32api.keybd_event(code, 0, 0, 0)
        win32api.keybd_event(code, 0, win32con.KEYEVENTF_KEYUP, 0)
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

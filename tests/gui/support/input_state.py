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
from . import atomic, deadline, native as n

IMC_GETOPENSTATUS = 5
IMC_SETOPENSTATUS = 6
GA_ROOT = 2
get_ancestor = n.api(n.user, "GetAncestor", n.w.HWND, n.w.HWND, n.w.UINT)


@deadline.operation('check global hotkey availability')
def require_available_hotkey(key, modifiers, description):
    """Probe registered conflicts without sending input or retaining ownership."""
    register = n.api(n.user, 'RegisterHotKey', n.w.BOOL,
                     n.w.HWND, n.c.c_int, n.w.UINT, n.w.UINT)
    unregister = n.api(n.user, 'UnregisterHotKey', n.w.BOOL, n.w.HWND, n.c.c_int)
    identifier = 0x574C  # Private to this controller thread's short-lived probe.
    if not register(None, identifier, modifiers, key):
        error = n.c.get_last_error()
        if error == 1409:  # ERROR_HOTKEY_ALREADY_REGISTERED
            raise RuntimeError('Input environment conflict: %s is registered as a global hotkey'
                               % description)
        raise n.c.WinError(error)
    try:
        return
    finally:
        if not unregister(None, identifier):
            raise n.c.WinError(n.c.get_last_error())


@contextmanager
def foreground(app, inputs):
    with n.physical_dpi_context():
        with deadline.operation('activate physical input target via UIA'):
            window = auto.ControlFromHandle(app.frame)
            window.Restore(waitTime=0)
            window.SetActive(waitTime=0)
        left, top, right, bottom = n.titlebar_rectangle(app.frame)
        point = ((left + right) // 2, (top + bottom) // 2)
        hit = win32gui.WindowFromPoint(point)
        assert get_ancestor(hit, GA_ROOT) == app.frame, "foreground title bar is obscured"
        if win32gui.GetForegroundWindow() != app.frame:
            inputs.click(*point, wait_time=0)
        n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "foreground input")
    yield lambda: win32gui.GetForegroundWindow() == app.frame


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
            # Windows may deny activation; never inject keys or retry for
            # permission. Cross-queue activation is asynchronous, however:
            # acknowledge its notification before judging the final foreground.
            win32gui.SetForegroundWindow(foreground)
            with deadline.operation('acknowledge foreground restoration'):
                acknowledged = n.send_timeout(foreground, win32con.WM_NULL,
                                              0, 0, 3, 1000, None)
            if not acknowledged or win32gui.GetForegroundWindow() != foreground:
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
        if raw_foreground and not win32gui.IsWindow(raw_foreground):
            raw_foreground = 0
        foreground = get_ancestor(raw_foreground, GA_ROOT) if raw_foreground else 0
        assert not raw_foreground or foreground, "Cannot resolve foreground root"
        # Recovery releases only inputs explicitly recorded before delivery.
        self.state = {"held": [], "cursor": auto.GetCursorPos(),
                      "foreground": foreground, "raw_foreground": raw_foreground}

    def save(self):
        atomic.write_json(self.path, self.state, indent=None)

    def hold(self, code):
        """Record ownership before delivery so the outer runner can undo it."""
        self.state["held"].append(code)
        self.save()

    def release(self, code):
        self.state["held"].remove(code)
        self.save()

    def holding(self, code):
        return code in self.state["held"]

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

    def click(self, x, y, *, wait_time=auto.OPERATION_WAIT_TIME):
        self.hold("mouse")
        auto.Click(x, y, waitTime=wait_time)
        self.release("mouse")

    def double_click(self, x, y):
        self.hold("mouse")
        try:
            auto.Click(x, y, waitTime=0)
            auto.Click(x, y, waitTime=0)
        except BaseException:
            auto.ReleaseMouse(waitTime=0)
            raise
        finally:
            self.release("mouse")

    def key(self, code):
        # A key() is a tap, not a held key. The provider's default 500 ms pause
        # occurs before key-up and can cross the Windows auto-repeat threshold.
        self.hold(code)
        auto.PressKey(code, waitTime=0)
        # SendInput preserves event order. Callers observe focus/results after
        # their gesture; the provider's post-key-up pause is not synchronization.
        auto.ReleaseKey(code, waitTime=0)
        self.release(code)

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

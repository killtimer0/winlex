"""Persist owned input changes before delivery so the outer runner can undo them."""
import json
import os
from pathlib import Path
import win32api
import win32con
import win32gui


def recover(path):
    if not path.exists():
        return
    state = json.loads(path.read_text(encoding="utf-8"))
    for key in state["held"]:
        if key == "mouse":
            win32api.mouse_event(win32con.MOUSEEVENTF_LEFTUP, 0, 0)
        else:
            win32api.keybd_event(key, 0, win32con.KEYEVENTF_KEYUP, 0)
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

    def __exit__(self, *args):
        recover(self.path)

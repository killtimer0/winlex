"""Capture displayed pixels through UIAutomation without forcing a repaint."""
import struct
from contextlib import contextmanager

import uiautomation as auto
import win32gui


@contextmanager
def capture(hwnd, client=False):
    if client:
        left, top = win32gui.ClientToScreen(hwnd, (0, 0))
        _, _, width, height = win32gui.GetClientRect(hwnd)
    else:
        left, top, right, bottom = win32gui.GetWindowRect(hwnd)
        width, height = right - left, bottom - top
    desktop = auto.GetRootControl()
    bounds = desktop.BoundingRectangle
    with auto.Bitmap.FromControl(desktop, left - bounds.left, top - bounds.top, width, height) as bitmap:
        assert bitmap, "UIAutomation capture failed"
        raw = bitmap.ToBytes("BGRA")
        stride = bitmap.Width * 4
        pixels = b"".join(raw[y:y + stride] for y in range(len(raw) - stride, -1, -stride))
        header = struct.pack("<IiiHHIIiiII", 40, bitmap.Width, bitmap.Height,
                             1, 32, 0, len(pixels), 0, 0, 0, 0)
        yield bitmap, header, pixels


def screenshot(hwnd, path):
    with capture(hwnd) as (_, header, pixels):
        path.write_bytes(struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54) + header + pixels)

"""Capture displayed pixels, including layered popups, without forcing a repaint."""
import struct
from contextlib import contextmanager

import uiautomation as auto
import win32con
import win32gui

CAPTUREBLT = 0x40000000


def screen_pixels(rect):
    """Include layered popups on XP, where ordinary screen copies omit them."""
    from . import native as n
    left, top, right, bottom = rect
    width, height = right - left, bottom - top
    gdi = n.c.WinDLL('gdi32')
    screen = win32gui.GetDC(0)
    memory = bitmap = old = None
    select = n.api(gdi, 'SelectObject', n.w.HANDLE, n.w.HDC, n.w.HANDLE)
    try:
        memory = n.api(gdi, 'CreateCompatibleDC', n.w.HDC, n.w.HDC)(screen)
        assert memory, 'Cannot create capture DC'
        bitmap = n.api(gdi, 'CreateCompatibleBitmap', n.w.HANDLE, n.w.HDC,
                       n.c.c_int, n.c.c_int)(screen, width, height)
        assert bitmap, 'Cannot create capture bitmap'
        old = select(memory, bitmap)
        assert old, 'Cannot select capture bitmap'
        copy = n.api(gdi, 'BitBlt', n.w.BOOL, n.w.HDC, n.c.c_int, n.c.c_int,
                     n.c.c_int, n.c.c_int, n.w.HDC, n.c.c_int, n.c.c_int, n.w.DWORD)
        assert copy(memory, 0, 0, width, height, screen, left, top,
                    win32con.SRCCOPY | CAPTUREBLT), 'Screen capture failed'
        select(memory, old)
        old = None
        pixels = n.c.create_string_buffer(width * height * 4)
        header = struct.pack('<IiiHHIIiiII', 40, width, height, 1, 32, 0, len(pixels), 0, 0, 0, 0)
        info = n.c.create_string_buffer(header)
        read = n.api(gdi, 'GetDIBits', n.c.c_int, n.w.HDC, n.w.HANDLE, n.w.UINT,
                     n.w.UINT, n.c.c_void_p, n.c.c_void_p, n.w.UINT)
        assert read(screen, bitmap, 0, height, pixels, info, 0) == height, 'Cannot read captured pixels'
        return header, pixels.raw
    finally:
        if old:
            select(memory, old)
        if bitmap:
            n.api(gdi, 'DeleteObject', n.w.BOOL, n.w.HANDLE)(bitmap)
        if memory:
            n.api(gdi, 'DeleteDC', n.w.BOOL, n.w.HDC)(memory)
        win32gui.ReleaseDC(0, screen)


def capture_pixels(hwnd, client=False, rect=None):
    """Capture physical pixels; an optional crop uses target-relative coordinates."""
    from . import native as n
    with n.window_dpi_context(hwnd):
        if client:
            left, top = win32gui.ClientToScreen(hwnd, (0, 0))
            _, _, width, height = win32gui.GetClientRect(hwnd)
        else:
            left, top, right, bottom = win32gui.GetWindowRect(hwnd)
            width, height = right - left, bottom - top
        crop = rect if rect is not None else (0, 0, width, height)
        x1, y1, x2, y2 = crop
        assert 0 <= x1 < x2 <= width and 0 <= y1 < y2 <= height, crop
        right, bottom = n.physical_screen_point(hwnd, (left + x2, top + y2))
        left, top = n.physical_screen_point(hwnd, (left + x1, top + y1))
    with n.physical_dpi_context():
        return screen_pixels((left, top, right, bottom))


@contextmanager
def capture(hwnd, client=False, rect=None):
    header, pixels = capture_pixels(hwnd, client, rect)
    with auto.Bitmap.FromBytes(bitmap_bytes(header, pixels)) as bitmap:
        assert bitmap, "Captured bitmap decoding failed"
        yield bitmap, header, pixels


def screenshot(hwnd, path):
    with capture(hwnd) as (bitmap, _, _):
        data = bitmap.ToBytes("png")
        assert data, "UIAutomation PNG encoding failed"
        path.write_bytes(data)


def assert_unobscured(hwnd):
    """Check popup coverage in Z order; hit testing skips transparent tooltips."""
    from . import native as n
    assert n.is_visible(hwnd), "popup is not visible"
    with n.physical_dpi_context():
        left, top, right, bottom = win32gui.GetWindowRect(hwnd)
        assert left < right and top < bottom, "popup has no displayed bounds"
        points = [(left + (right - left) * x // 4, top + (bottom - top) * y // 4)
                  for x, y in ((1, 1), (3, 1), (2, 2), (1, 3), (3, 3))]
        above = win32gui.GetWindow(hwnd, win32con.GW_HWNDPREV)
        while above:
            if n.is_visible(above) and not win32gui.IsIconic(above):
                l, t, r, b = win32gui.GetWindowRect(above)
                covered = [(x, y) for x, y in points if l <= x < r and t <= y < b]
                style = win32gui.GetWindowLong(above, win32con.GWL_EXSTYLE)
                if style & win32con.WS_EX_LAYERED and not style & win32con.WS_EX_TRANSPARENT:
                    # Per-pixel alpha holes in layered windows do not occlude
                    # the popup; native hit testing respects those holes.
                    covered = [point for point in covered if win32gui.WindowFromPoint(point) == above]
                assert not covered, (
                    "popup is obscured", hwnd, above, n.class_name(above))
            above = win32gui.GetWindow(above, win32con.GW_HWNDPREV)


def bitmap_bytes(header, pixels):
    return struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54) + header + pixels


def save_pixels(path, header, pixels):
    with auto.Bitmap.FromBytes(bitmap_bytes(header, pixels)) as bitmap:
        assert bitmap, "UIAutomation bitmap decoding failed"
        data = bitmap.ToBytes("png")
        assert data, "UIAutomation PNG encoding failed"
        path.write_bytes(data)

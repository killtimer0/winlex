"""Best-effort window capture; failure is recorded, never treated as crash evidence."""
import win32con
import win32gui
import struct
from contextlib import contextmanager
from . import native as n


@contextmanager
def capture(hwnd, client=False, repaint=True):
    if repaint:
        win32gui.RedrawWindow(hwnd, None, None,
                             win32con.RDW_INVALIDATE | win32con.RDW_UPDATENOW | win32con.RDW_ALLCHILDREN)
    gdi = n.c.WinDLL("gdi32", use_last_error=True)
    create_dc = n.api(gdi, "CreateCompatibleDC", n.w.HDC, n.w.HDC)
    delete_dc = n.api(gdi, "DeleteDC", n.w.BOOL, n.w.HDC)
    select = n.api(gdi, "SelectObject", n.w.HANDLE, n.w.HDC, n.w.HANDLE)
    delete = n.api(gdi, "DeleteObject", n.w.BOOL, n.w.HANDLE)
    create_bitmap = n.api(gdi, "CreateCompatibleBitmap", n.w.HANDLE, n.w.HDC, n.c.c_int, n.c.c_int)
    get_bits = n.api(gdi, "GetDIBits", n.c.c_int, n.w.HDC, n.w.HANDLE, n.w.UINT,
                     n.w.UINT, n.c.c_void_p, n.c.c_void_p, n.w.UINT)
    left, top, right, bottom = n.rectangle(hwnd, client=client)
    width, height = right - left, bottom - top
    header = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 32, 0, width * height * 4, 0, 0, 0, 0)
    source = win32gui.GetDC(hwnd) if client else win32gui.GetWindowDC(hwnd)
    target = bitmap = previous = None
    try:
        target = create_dc(source)
        assert target, "CreateCompatibleDC failed"
        info = n.c.create_string_buffer(header)
        bitmap = create_bitmap(source, width, height)
        assert bitmap, "CreateCompatibleBitmap failed"
        previous = select(target, bitmap)
        assert previous, "SelectObject failed"
        if repaint:
            assert n.api(n.user, "PrintWindow", n.w.BOOL, n.w.HWND, n.w.HDC,
                         n.w.UINT)(hwnd, target, int(client)), "PrintWindow failed"
        else:
            # Preserve the currently displayed pixels, including stale painting.
            assert n.api(gdi, "BitBlt", n.w.BOOL, n.w.HDC, n.c.c_int, n.c.c_int,
                         n.c.c_int, n.c.c_int, n.w.HDC, n.c.c_int, n.c.c_int,
                         n.w.DWORD)(target, 0, 0, width, height, source, 0, 0,
                                    win32con.SRCCOPY), "BitBlt failed"
        select(target, previous)
        pixels = n.c.create_string_buffer(width * height * 4)
        assert get_bits(source, bitmap, 0, height, pixels, info, 0) == height, "GetDIBits failed"
        select(target, bitmap)
        yield target, header, pixels.raw
    finally:
        if previous:
            select(target, previous)
        if bitmap:
            delete(bitmap)
        if target:
            delete_dc(target)
        win32gui.ReleaseDC(hwnd, source)


def screenshot(hwnd, path):
    with capture(hwnd) as (_, header, pixels):
        path.write_bytes(struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54) + header + pixels)

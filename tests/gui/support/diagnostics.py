"""Best-effort window capture; failure is recorded, never treated as crash evidence."""
import win32con
import win32gui
import win32ui


def screenshot(hwnd, path):
    left, top, right, bottom = win32gui.GetWindowRect(hwnd)
    handle = win32gui.GetWindowDC(hwnd)
    source = win32ui.CreateDCFromHandle(handle)
    target = source.CreateCompatibleDC()
    bitmap = win32ui.CreateBitmap()
    try:
        bitmap.CreateCompatibleBitmap(source, right-left, bottom-top)
        target.SelectObject(bitmap)
        target.BitBlt((0, 0), (right-left, bottom-top), source, (0, 0), win32con.SRCCOPY)
        bitmap.SaveBitmapFile(target, str(path))
    finally:
        target.DeleteDC()
        source.DeleteDC()
        win32gui.DeleteObject(bitmap.GetHandle())
        win32gui.ReleaseDC(hwnd, handle)

"""Displayed pixel/contrast evidence and protected real-input acquisition."""
from collections import Counter
from contextlib import contextmanager
import pytest
import win32con
import win32gui
import winreg
import uiautomation as auto
from support import deadline
from support.diagnostics import capture_pixels
from support.input_state import foreground as acquire_foreground
from driver.appearance import THEME_SUPPORTED


@contextmanager
def foreground(app, inputs):
    for attempt in range(3):
        owner = acquire_foreground(app, inputs)
        try:
            owner.__enter__()
        except pytest.skip.Exception:
            if attempt == 2:
                raise AssertionError('Cannot acquire current desktop for dark-surface contract')
            continue
        try:
            yield
        finally:
            owner.__exit__(None, None, None)
        return


def dark_app(launch, profile, arguments=()):
    if not THEME_SUPPORTED:
        pytest.skip('requires Windows 10 build 17763 or later')
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'dark')
    app = launch(arguments)
    return app


@deadline.operation('repaint and sample control pixels')
def colors(hwnd, rect, client=True):
    win32gui.RedrawWindow(hwnd, None, None,
                         win32con.RDW_INVALIDATE | win32con.RDW_UPDATENOW |
                         win32con.RDW_ALLCHILDREN | win32con.RDW_FRAME)
    return observed_colors(hwnd, rect, client)


@deadline.operation('sample displayed control pixels')
def observed_colors(hwnd, rect, client=True):
    """Observe native theme transitions without synchronously forcing repaint."""
    _header, pixels = capture_pixels(hwnd, client=client, rect=rect)
    # Count native BGRA words in C instead of allocating a tuple per pixel.
    # Alpha is not part of the displayed BGR color, so merge it afterwards.
    packed = Counter(memoryview(pixels).cast('I'))
    result = Counter()
    for color, count in packed.items():
        result[(color & 255, (color >> 8) & 255, (color >> 16) & 255)] += count
    return result


def background(hwnd, rect, client=True):
    return colors(hwnd, rect, client).most_common(1)[0][0]


def dark_color(color):
    """Theme contract: a dark native surface stays below the midpoint."""
    return max(color) < 128


def contrast_ratio(first, second):
    """WCAG contrast ratio for native BGR palette entries."""
    def luminance(color):
        def linear(value):
            value /= 255
            return value / 12.92 if value <= .03928 else ((value + .055) / 1.055) ** 2.4
        red, green, blue = color[2], color[1], color[0]
        return .2126 * linear(red) + .7152 * linear(green) + .0722 * linear(blue)
    high, low = sorted((luminance(first), luminance(second)), reverse=True)
    return (high + .05) / (low + .05)


def text_contrast(palette, minimum=4.5, minimum_count=4):
    """Check that the displayed palette has readable text against its fill."""
    rows = list(palette.items()) if hasattr(palette, 'items') else list(palette)
    if not rows:
        return False
    background = max(rows, key=lambda row: row[1])[0]
    return any(color != background and count >= minimum_count
               and contrast_ratio(color, background) >= minimum
               for color, count in rows)


def center(rect):
    left, top, right, bottom = rect
    return (left + right) // 2, (top + bottom) // 2


def inset(rect, amount=3):
    left, top, right, bottom = rect
    return left + amount, top + amount, right - amount, bottom - amount


def move(hwnd, point):
    auto.MoveTo(*win32gui.ClientToScreen(hwnd, point))

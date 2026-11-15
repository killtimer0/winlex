"""Choose appearance and observe displayed fills without a palette oracle."""
import sys
import time
import pytest
import uiautomation as auto
from support import deadline, native as n
from support.session import select_combo
from support.diagnostics import capture


THEME_SUPPORTED = tuple(sys.getwindowsversion()[:3]) >= (10, 0, 17763)


modern_theme = pytest.mark.skipif(
    not THEME_SUPPORTED, reason='requires Windows 10 build 17763 or later')


@deadline.operation('enumerate and select theme option via UIA')
def select_theme(page, index):
    combo = auto.ControlFromHandle(n.get_item(page, 1301))
    expansion = combo.GetExpandCollapsePattern()
    expansion.Expand(waitTime=0)
    try:
        options = [item.Name for item, _ in auto.WalkControl(combo)
                   if item.ControlType == auto.ControlType.ListItemControl]
    finally:
        expansion.Collapse(waitTime=0)
    select_combo(combo, options[index])


@deadline.operation('read selected combo option via UIA')
def selected_index(app, hwnd):
    selected = auto.ControlFromHandle(hwnd).GetValuePattern().Value
    return app.choices(hwnd).index(selected)


def background_pixels(hwnd):
    """A blank client patch for relative theme comparisons, not a palette oracle."""
    with n.window_dpi_context(hwnd):
        _, _, width, height = n.rectangle(hwnd, client=True)
        # Stay inside the client fill, away from scaled/rounded Edit borders.
        x, y = width - 8, height // 2
        with capture(hwnd, client=True, rect=(x, y, x + 1, y + 1)) as (_, _header, pixels):
            return pixels[:3]


def dark_background(hwnd):
    """Require a blank displayed client patch to use the dark theme fill."""
    return max(background_pixels(hwnd)) < 128


def settled_colors(sample, description):
    """Do not retain an animation's transient frame as an equality baseline."""
    last = None
    unchanged_since = time.monotonic()

    def settled():
        nonlocal last, unchanged_since
        color = sample()
        now = time.monotonic()
        if color != last:
            last, unchanged_since = color, now
        return color if now - unchanged_since >= .25 else None

    return n.wait(settled, description)


def settled_background_pixels(hwnd):
    return settled_colors(lambda: background_pixels(hwnd),
                          'displayed background settles before baseline capture')

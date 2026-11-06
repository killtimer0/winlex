"""Visible native-control contracts shared by Windows 10 and Windows 11."""
from collections import Counter
from contextlib import contextmanager
import json
import struct

import pytest
import win32api
import win32con
import win32gui
import winreg
import uiautomation as auto

from support import messages
from support import native as n
from support.diagnostics import capture, screenshot
from support.input_state import InputState, foreground as acquire_foreground
from test_context import popup_window
from test_tooltips import infotip
from test_theme import THEME_SUPPORTED
from test_tools import visual_state

pytestmark = pytest.mark.interactive


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
        pytest.skip('requires Windows 10 build 19045 or later')
    profile.set('Language', winreg.REG_SZ, 'en-US')
    profile.set('Theme', winreg.REG_SZ, 'dark')
    app = launch(arguments)
    return app


def colors(hwnd, rect, client=True):
    win32gui.RedrawWindow(hwnd, None, None,
                         win32con.RDW_INVALIDATE | win32con.RDW_UPDATENOW |
                         win32con.RDW_ALLCHILDREN | win32con.RDW_FRAME)
    with capture(hwnd, client=client) as (_, header, pixels):
        width, height = struct.unpack_from('<ii', header, 4)
        left, top, right, bottom = rect
        assert 0 <= left < right <= width and 0 <= top < bottom <= height, rect
        return Counter(tuple(pixels[offset:offset + 3])
                       for y in range(top, bottom) for x in range(left, right)
                       for offset in [((height - y - 1) * width + x) * 4])


def background(hwnd, rect, client=True):
    palette = colors(hwnd, rect, client)
    color, count = palette.most_common(1)[0]
    assert count > sum(palette.values()) // 3, palette.most_common(5)
    blue, green, red = color
    assert .0722 * blue + .7152 * green + .2126 * red < 128, ('light native surface', color)
    return color


class ScrollbarInfo(n.c.Structure):
    _fields_ = [('size', n.w.DWORD), ('rect', n.w.RECT), ('line', n.c.c_int),
                ('top', n.c.c_int), ('bottom', n.c.c_int), ('reserved', n.c.c_int),
                ('state', n.w.DWORD * 6)]


def scrollbar_track(hwnd, required):
    if not win32gui.GetWindowLong(hwnd, win32con.GWL_STYLE) & win32con.WS_VSCROLL:
        assert not required, 'results must have a native vertical scrollbar'
        return {'present': False}
    info = ScrollbarInfo()
    info.size = n.c.sizeof(info)
    assert n.api(n.user, 'GetScrollBarInfo', n.w.BOOL, n.w.HWND, n.w.LONG,
                 n.c.POINTER(ScrollbarInfo))(hwnd, -5, n.c.byref(info))
    assert not info.state[0] & 0x18000, 'native scrollbar must be visible and on screen'
    x = (info.rect.left + info.rect.right) // 2
    assert info.rect.left <= x - 2 < x + 2 <= info.rect.right
    assert 0 < info.line <= info.top < info.bottom <= info.rect.bottom - info.rect.top - info.line
    ml, mt, mr, mb = win32api.GetMonitorInfo(win32api.MonitorFromWindow(hwnd))['Monitor']
    assert ml <= x - 2 < x + 2 <= mr
    gaps = [(info.line, info.top), (info.bottom, info.rect.bottom - info.rect.top - info.line)]
    gaps = [(max(info.rect.top + start + 2, mt), min(info.rect.top + end - 2, mb))
            for start, end in gaps]
    top, bottom = max(gaps, key=lambda gap: gap[1] - gap[0])
    assert bottom - top >= 4, 'native scrollbar needs a visible track sample outside thumb/arrows'
    y = (top + bottom) // 2
    assert all(win32gui.WindowFromPoint(point) == hwnd
               for point in [(x - 2, y - 2), (x + 1, y + 1)]), 'scrollbar sample is obscured'
    left, top, _, _ = n.rectangle(hwnd)
    sample = (x - left - 2, y - top - 2, x - left + 2, y - top + 2)
    palette = colors(hwnd, sample, client=False)
    color, count = palette.most_common(1)[0]
    assert sum(palette.values()) == 16 and count > 16 // 3, palette.most_common(5)
    return {'present': True, 'sample': sample, 'colors_bgr': palette.most_common(5),
            'luminance': sum(a * b for a, b in zip(color, (.0722, .7152, .2126)))}


def button_rect(app, toolbar, index):
    rect = auto.ControlFromHandle(toolbar).GetChildren()[index].BoundingRectangle
    left, top, _, _ = n.rectangle(toolbar)
    return rect.left - left, rect.top - top, rect.right - left, rect.bottom - top


def center(rect):
    left, top, right, bottom = rect
    return (left + right) // 2, (top + bottom) // 2


def inset(rect, amount=3):
    left, top, right, bottom = rect
    return left + amount, top + amount, right - amount, bottom - amount


def screen_image(hwnd, path):
    with capture(hwnd) as (_, header, pixels):
        path.write_bytes(struct.pack('<2sIHHI', b'BM', 54 + len(pixels), 0, 0, 54) + header + pixels)


def move(hwnd, point):
    auto.MoveTo(*win32gui.ClientToScreen(hwnd, point))


def test_dark_toolbar_checked_hot_pixels_and_activation(launch, profile, settings):
    app = dark_app(launch, profile)
    toolbar = n.get_item(app.frame, 1113)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        first, second = [button_rect(app, toolbar, i) for i in range(2)]
        move(app.search, (8, 8))
        n.wait(lambda: n.c.c_int32(n.send(toolbar, messages.TB_GETHOTITEM)).value == -1, 'toolbar not hot')
        checked = background(toolbar, inset(first))
        idle = background(toolbar, inset(second))
        assert checked != idle, 'checked toolbar button must remain distinguishable'
        move(toolbar, center(second))
        n.wait(lambda: n.send(toolbar, messages.TB_GETHOTITEM) == 1, 'native toolbar hover')
        n.wait(lambda: background(toolbar, inset(second)) != idle, 'dark toolbar hover pixels')
        assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
        screenshot(app.frame, settings[2] / 'dark-toolbar-hot.bmp')
        inputs.click(*win32gui.ClientToScreen(toolbar, center(second)))
        n.wait(lambda: n.is_visible(app.sources), 'checked button opens metadata')
        assert n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1021)
        assert not n.send(toolbar, messages.TB_ISBUTTONCHECKED, 1040)
        move(app.status, (8, 3))
        n.wait(lambda: n.c.c_int32(n.send(toolbar, messages.TB_GETHOTITEM)).value == -1, 'toolbar hover cleared')
        assert background(toolbar, inset(second)) != background(toolbar, inset(first))


@pytest.mark.parametrize('sources', [False, True])
def test_dark_list_native_selection_hover_and_focus(launch, profile, settings, sources):
    app = dark_app(launch, profile, [settings[1]])
    app.query('CreateFile')
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        if sources:
            app.command(1021)
        control = app.sources if sources else app.results
        app.select(0, control)
        app.focus(control)
        n.wait(lambda: app.focused() == control, 'native list focus')
        move(app.status, (8, 3))
        left, top, right, bottom = app.item_rect(0, control)
        width = n.rectangle(control, client=True)[2]
        sample = (min(right, width) - 16, top + 4, min(right, width) - 5, bottom - 4)
        selected = background(control, sample)
        assert n.send(control, messages.LVM_GETITEMSTATE, 0, 3) == 3
        app.focus(n.get_item(app.frame, 1020) if sources else app.search)
        unfocused = background(control, sample)
        record_path = settings[2] / ('dark-list-%s-colors.json' % sources)
        evidence = {'selected': selected, 'unfocused': unfocused, 'sample': sample}
        record_path.write_text(json.dumps(evidence), encoding='utf-8')
        n.wait(lambda: background(control, sample) != selected, 'list focus painting')
        idle = background(control, sample)
        assert app.selected_row(control) == 0
        assert n.send(control, messages.LVM_GETITEMSTATE, 0, 3) == 3
        # Hover an unselected row: selected+unfocused hover is intentionally unchanged
        # by some Windows 10 common-control themes.
        app.row(0, control).GetSelectionItemPattern().RemoveFromSelection()
        assert app.selected_row(control) == -1
        idle = background(control, sample)
        move(control, (left + 8, (top + bottom) // 2))
        n.wait(lambda: n.send(control, messages.LVM_GETHOTITEM) == 0, 'native list hot item')
        def hovered():
            value = background(control, sample)
            evidence.update({'idle_unselected': idle, 'hot_unselected': value,
                             'hot_item': n.send(control, messages.LVM_GETHOTITEM)})
            record_path.write_text(json.dumps(evidence), encoding='utf-8')
            return value if value != idle else None

        screen_image(app.frame, settings[2] / ('dark-list-%s-native-screen.bmp' % sources))
        hot = n.wait(hovered, 'dark list hover pixels')
        assert .0722 * hot[0] + .7152 * hot[1] + .2126 * hot[2] < 128
        screenshot(app.frame, settings[2] / ('dark-list-%s-hot.bmp' % sources))
        move(app.status, (8, 3))
        app.select(0, control)
        app.focus(control)
        n.wait(lambda: background(control, sample) == selected, 'list selected focus restored')
        assert app.selected_row(control) == 0
        track = scrollbar_track(control, required=not sources)
        evidence['scrollbar_track'] = track
        record_path.write_text(json.dumps(evidence), encoding='utf-8')
        screen_image(control, settings[2] / ('dark-list-%s-scrollbar.bmp' % sources))
        if track['present']:
            assert track['luminance'] < 128, ('light native scrollbar track', track)


@pytest.mark.parametrize('sources', [False, True])
def test_dark_list_native_tooltip_pixels(launch, profile, settings, sources):
    app = dark_app(launch, profile, [settings[1]])
    app.query('CreateFileW')
    # The existing helper owns cursor restoration; capture the tip while it is visible.
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        if sources:
            app.command(1021)
        control = app.sources if sources else app.results
        text = infotip(app, control, 0, settings[2] / ('dark-infotip-%s.bmp' % sources))
        assert text
        tooltip = n.send(control, messages.LVM_GETTOOLTIPS)
        # infotip restores the cursor on return, so show it again before sampling.
        rect = app.item_rect(0, control)
        move(control, (min(rect[0] + 120, rect[2] - 8), rect[1] + (rect[3] - rect[1]) // 4))
        n.wait(lambda: n.is_visible(tooltip), 'dark native infotip visible')
        _, _, width, height = n.rectangle(tooltip, client=True)
        palette = colors(tooltip, (3, 3, width - 3, height - 3))
        background(tooltip, (3, 3, width - 3, height - 3))
        assert any(min(color) > 150 for color in palette), 'tooltip text must remain readable'


def menu_items(app):
    menu = win32gui.GetMenu(app.frame)
    return menu, [(i, win32gui.GetMenuItemID(menu, i))
                  for i in range(win32gui.GetMenuItemCount(menu))]


def menu_rect(app, menu, index):
    rect = n.w.RECT()
    assert n.api(n.user, 'GetMenuItemRect', n.w.BOOL, n.w.HWND, n.w.HMENU,
                 n.w.UINT, n.c.POINTER(n.w.RECT))(app.frame, menu, index, n.c.byref(rect))
    return rect.left, rect.top, rect.right, rect.bottom


def test_dark_tool_menu_icon_is_visible_on_creation(launch, profile, settings):
    from test_tools import tool

    app = dark_app(launch, profile, [settings[1]])
    app.query('CreateFileW')
    doc = app.open_row()
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        visual_state(doc, auto.WindowVisualState.Maximized)
        for new in (False, True):
            doc = tool(app, new=new)
            assert n.is_zoomed(doc)
            menu, _ = menu_items(app)
            assert win32gui.GetSubMenu(menu, 0)
            left, top, right, bottom = menu_rect(app, menu, 0)
            x, y, _, _ = n.rectangle(app.frame)
            palette = colors(app.frame, inset((left - x, top - y, right - x, bottom - y), 2),
                             client=False)
            bg = palette.most_common(1)[0][0]
            assert max(bg) < 128
            assert any(max(color) - max(bg) > 60 for color in palette), palette


def test_dark_mdi_merged_menu_glyph_pixels_and_native_actions(launch, profile, settings):
    app = dark_app(launch, profile, [settings[1]])
    app.query('CreateFileW')
    doc = app.open_row()
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        visual_state(doc, auto.WindowVisualState.Maximized)
        n.wait(lambda: n.is_zoomed(doc), 'maximized MDI document')
        menu, items = menu_items(app)
        commands = (win32con.SC_MINIMIZE, win32con.SC_RESTORE, win32con.SC_CLOSE)
        positions = {command: next(i for i, value in items if value == command)
                     for command in commands}
        assert win32gui.GetSubMenu(menu, 0), 'merged document system icon menu'
        frame_left, frame_top, _, _ = n.rectangle(app.frame)
        evidence = {}
        for name, index in [('icon', 0), *[(str(command), positions[command]) for command in commands]]:
            left, top, right, bottom = menu_rect(app, menu, index)
            rect = (left - frame_left, top - frame_top, right - frame_left, bottom - frame_top)
            palette = colors(app.frame, inset(rect, 2), client=False)
            bg = palette.most_common(1)[0][0]
            evidence[name] = {'rect': rect, 'background': bg,
                              'colors': palette.most_common(10),
                              'dark': max(bg) < 128,
                              'visible': any(max(color) - max(bg) > 60 for color in palette)}
        (settings[2] / 'dark-mdi-menu-colors.json').write_text(json.dumps(evidence, indent=2), encoding='utf-8')
        screen_image(app.frame, settings[2] / 'dark-mdi-native-screen.bmp')
        screenshot(app.frame, settings[2] / 'dark-mdi-merged-menu.bmp')
        assert all(item['dark'] and item['visible'] for item in evidence.values()), evidence
        # Open the actual merged icon popup, then dismiss without changing the document.
        inputs.click(*center(menu_rect(app, menu, 0)))
        popup = n.wait(lambda: popup_window(app.process.pid), 'merged document system popup')
        assert n.send(popup, messages.MN_GETHMENU) == win32gui.GetSystemMenu(doc, False)
        inputs.key(win32con.VK_ESCAPE)
        n.wait(lambda: not popup_window(app.process.pid), 'merged system menu dismissed')
        assert app.active_document() == doc
        inputs.click(*center(menu_rect(app, menu, positions[win32con.SC_RESTORE])))
        n.wait(lambda: not n.is_zoomed(doc), 'merged restore button')
        visual_state(doc, auto.WindowVisualState.Maximized)
        n.wait(lambda: n.is_zoomed(doc), 'MDI maximized after restore')
        menu, items = menu_items(app)
        inputs.click(*center(menu_rect(app, menu, next(i for i, command in items
                                                      if command == win32con.SC_MINIMIZE))))
        n.wait(lambda: win32gui.IsIconic(doc), 'merged minimize button')
        visual_state(doc, auto.WindowVisualState.Maximized)
        n.wait(lambda: n.is_zoomed(doc), 'MDI remaximized')
        menu, items = menu_items(app)
        inputs.click(*center(menu_rect(app, menu, next(i for i, command in items
                                                      if command == win32con.SC_CLOSE))))
        n.wait(lambda: not n.is_window(doc), 'merged close button')
        assert not app.documents()

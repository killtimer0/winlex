import shutil
import struct
import time
import pytest
import win32api
import win32con
import win32gui
from support import native as n
from support.input_state import InputState
from support.diagnostics import screenshot
from support import messages


def test_search_tooltip_registers_and_tracks_mode_without_hover(launch, settings):
    app = launch()
    pointer = "<Q" if app.is64 else "<I"

    def tool_info():
        data = bytearray(128)
        struct.pack_into("<I", data, 0, 64 if app.is64 else 44)
        struct.pack_into(pointer, data, 8, app.search)
        struct.pack_into(pointer, data, 16 if app.is64 else 12, app.search)
        struct.pack_into(pointer, data, 48 if app.is64 else 36, app.memory + 128)
        assert n.write_memory(app.handle, app.memory, bytes(data), len(data), None)

    tooltip = None
    for hwnd in n.windows():
        if (n.class_name(hwnd) == "tooltips_class32"
                and win32gui.GetWindow(hwnd, win32con.GW_OWNER) == app.frame):
            tool_info()
            if n.send(hwnd, win32con.WM_USER + 53, 0, app.memory):  # TTM_GETTOOLINFOW
                tooltip = hwnd
                break
    assert tooltip, "search input has a registered tooltip"

    def text():
        tool_info()
        assert n.write_memory(app.handle, app.memory + 128, bytes(4096), 4096, None)
        n.send(tooltip, win32con.WM_USER + 56, 0, app.memory)  # TTM_GETTEXTW
        return app.remote_text(app.memory + 128, 4096)

    initial = text()
    assert initial
    selector = n.get_item(app.frame, 1114)
    for index in [1, 0]:
        n.send(selector, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)
        actual = text()
        assert actual and (actual == initial) == (index == 0)


def test_toolbar_names_on_hover(launch, settings):
    app = launch()
    if not app.real_input:
        pytest.skip('requires --real-input')
    toolbar = n.get_item(app.frame, 1113)
    tooltip = n.send(toolbar, win32con.WM_USER + 35)
    with InputState() as inputs, n.window_dpi_context(app.frame):
        foreground = n.api(n.user, 'SetForegroundWindow', n.w.BOOL, n.w.HWND)
        n.wait(lambda: win32gui.GetForegroundWindow() == app.frame or foreground(app.frame),
               'toolbar foreground')
        for index, names in enumerate((('Search', '搜索'), ('Metadata', '元数据'))):
            win32api.SetCursorPos(win32gui.ClientToScreen(app.search, (8, 8)))
            n.wait(lambda: not n.is_visible(tooltip), 'toolbar tooltip hidden')
            time.sleep(.1)
            assert n.send(toolbar, messages.TB_GETITEMRECT, index, app.memory)
            buffer = n.c.create_string_buffer(16)
            assert n.read_memory(app.handle, app.memory, buffer, 16, None)
            left, top, right, bottom = struct.unpack('<4i', buffer.raw)
            win32api.SetCursorPos(win32gui.ClientToScreen(toolbar,
                ((left + right) // 2, (top + bottom) // 2)))
            n.wait(lambda: n.is_visible(tooltip), 'toolbar tooltip')
            assert n.window_text(tooltip) in names
            screenshot(app.frame, settings[2] / ('toolbar-%d.bmp' % index))
        inputs.click(*win32gui.ClientToScreen(toolbar,
            ((left + right) // 2, (top + bottom) // 2)))
        n.wait(lambda: n.is_visible(app.sources), 'toolbar opens metadata')


def infotip(app, control, row, image):
    if not app.real_input:
        pytest.skip('requires --real-input')
    if not n.is_visible(control):
        app.command(1021 if control == app.sources else 1040)
    n.send(control, 0x1013, row, 0)
    tooltip = n.send(control, 0x104E)
    with InputState():
        win32gui.SetForegroundWindow(app.frame)
        _, _, width, height = n.rectangle(control, client=True)
        win32api.SetCursorPos(win32gui.ClientToScreen(control, (width - 8, height - 8)))
        n.wait(lambda: not n.is_visible(tooltip), 'infotip hidden')
        time.sleep(.1)
        left, top, right, bottom = app.item_rect(row, control)
        win32api.SetCursorPos(win32gui.ClientToScreen(control,
            (min(left + 120, right - 8), top + (bottom - top) // 4)))
        n.wait(lambda: n.is_visible(tooltip), 'native infotip')
        time.sleep(.1)
        text = n.window_text(tooltip)
        assert n.is_visible(tooltip)
        screenshot(tooltip, image)
        return text


def test_structured_infotips_preserve_paths_and_report_reload_errors(launch, settings):
    from support.paths import extended
    directory = settings[2] / ('长路径' * 40) / ('metadata-' * 12)
    extended(directory).mkdir(parents=True, exist_ok=True)
    path = directory / '来源-😀.winmd'
    shutil.copyfile(settings[1], extended(path))
    app = launch([path])
    app.query('CreateFileW')
    tip = infotip(app, app.results, 0, settings[2] / 'search-tooltip.bmp')
    assert tip.startswith(app.cell(0).splitlines()[0] + '\r\n\r\n')
    assert 'Namespace: Windows.Win32.' in tip or '命名空间：Windows.Win32.' in tip
    assert str(path) in tip and path.name in tip
    source = infotip(app, app.sources, 0, settings[2] / 'source-tooltip.bmp')
    assert source.startswith(path.name + '\r\n\r\n')
    assert str(path) in source and str(extended(path).stat().st_size) in source
    extended(path).unlink()
    app.select(0, app.sources)
    app.command(1044)
    n.wait(lambda: app.cell(0, app.sources).splitlines()[1].startswith(('Reload failed', '重载失败')),
           'reload failure', 45)
    tip = infotip(app, app.sources, 0, settings[2] / 'source-error-tooltip.bmp')
    assert 'Error: ' in tip or '错误：' in tip
    assert 'Previously loaded metadata is still available.' in tip or '先前加载的元数据仍可使用。' in tip


def test_infotips_include_values_and_full_guids(launch, settings):
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, labels in [
        (1, 'value = 5', ('Value: 5', '值：5')),
        (1, 'guid = "0000000c-0000-0000-c000-000000000046"', ('GUID: ', 'GUID：')),
    ]:
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)
        app.query(query)
        assert app.count() > 0
        tip = infotip(app, app.results, 0, settings[2] / ('value-tooltip-%d.bmp' % index))
        assert any(label in tip for label in labels), tip
        prefixes = [
            ('Type: ', '类型：'), ('Namespace: ', '命名空间：'),
            ('Owner: ', '所属对象：'), ('Value: ', '值：'),
            ('GUID: ', 'GUID：'), ('Architecture: ', '架构：'),
            ('Path: ', '路径：'),
        ]
        lines = tip.split('\r\n')
        assert lines[1] == '', tip
        fields = [next(i for i, names in enumerate(prefixes) if line.startswith(names))
                  for line in lines[2:]]
        assert fields == sorted(set(fields)), tip
        assert fields[0] == 0 and fields[-1] == 6, tip
        if query.startswith('guid'):
            assert '0000000c-0000-0000-c000-000000000046' in tip.lower()

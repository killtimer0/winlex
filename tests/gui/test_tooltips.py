import shutil
import struct
import time
import pytest
import win32api
import win32con
import win32gui
import win32process
import uiautomation as auto
from support import native as n
from support.input_state import InputState
from support.diagnostics import screenshot
from support.pe import is_64_bit


def registered_tip(app, target, bits):
    pointer = '<Q' if bits == 64 else '<I'
    size = 64 if bits == 64 else 44
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ
                                  | win32con.PROCESS_VM_WRITE, False, app.process.pid)
    memory = None
    try:
        memory = win32process.VirtualAllocEx(process, 0, 8192,
                                            win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        info = bytearray(size)
        struct.pack_into('<I', info, 0, size)
        struct.pack_into(pointer, info, 8, target)
        struct.pack_into(pointer, info, 16 if bits == 64 else 12, target)
        struct.pack_into(pointer, info, 48 if bits == 64 else 36, memory + 128)
        info_buffer = n.c.create_string_buffer(bytes(info))
        tooltip = win32gui.FindWindowEx(0, 0, 'tooltips_class32', None)
        while tooltip:
            if win32gui.GetWindow(tooltip, win32con.GW_OWNER) == app.frame:
                if not n.write_process_memory(int(process), memory, info_buffer, len(info), None):
                    raise n.c.WinError(n.c.get_last_error())
                if n.send(tooltip, win32con.WM_USER + 53, 0, memory):
                    if not n.write_process_memory(int(process), memory, info_buffer, len(info), None):
                        raise n.c.WinError(n.c.get_last_error())
                    n.send(tooltip, win32con.WM_USER + 56, 0, memory)
                    data = n.c.create_string_buffer(4096)
                    size_read = n.c.c_size_t()
                    if not n.read_process_memory(int(process), memory + 128, data,
                                                 len(data), n.c.byref(size_read)):
                        raise n.c.WinError(n.c.get_last_error())
                    text = data.raw[:size_read.value].decode(
                        'utf-16-le', 'surrogatepass').split('\0', 1)[0]
                    return tooltip, text
            tooltip = win32gui.FindWindowEx(0, tooltip, 'tooltips_class32', None)
        raise AssertionError('control has no registered tooltip')
    finally:
        try:
            if memory is not None:
                win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        finally:
            win32api.CloseHandle(process)


def test_search_tooltip_registers_and_tracks_mode_without_hover(launch, settings):
    app = launch()
    selector = n.get_item(app.frame, 1114)
    choices = app.choices(selector)
    from support.session import select_combo
    bits = 64 if is_64_bit(settings[0]) else 32
    _, initial = registered_tip(app, app.search, bits)
    assert initial, "search input has a registered tooltip"
    for index in (1, 0):
        select_combo(selector, choices[index])
        _, actual = registered_tip(app, app.search, bits)
        assert actual and (actual == initial) == (index == 0)


@pytest.mark.interactive
def test_toolbar_names_on_hover(launch, settings):
    app = launch()
    toolbar = n.get_item(app.frame, 1113)
    tooltip = auto.ToolTipControl(searchDepth=3, Compare=lambda item, depth: item.ProcessId == app.process.pid)
    with InputState() as inputs, n.window_dpi_context(app.frame):
        auto.ControlFromHandle(app.frame).SetActive()
        assert win32gui.GetForegroundWindow() == app.frame, 'toolbar foreground'
        for index, command in enumerate((1040, 1021)):
            bounds = auto.ControlFromHandle(app.search).BoundingRectangle
            auto.MoveTo(bounds.left + 8, bounds.top + 8)
            n.wait(lambda: not tooltip.Exists(0, 0) or not n.is_visible(tooltip.NativeWindowHandle),
                   'toolbar tooltip hidden')
            time.sleep(.1)
            button = auto.ControlFromHandle(toolbar).ButtonControl(Name=app.toolbar_text(command))
            bounds = button.BoundingRectangle
            auto.MoveTo(bounds.xcenter(), bounds.ycenter())
            n.wait(lambda: tooltip.Exists(0, 0) and n.is_visible(tooltip.NativeWindowHandle), 'toolbar tooltip')
            assert tooltip.Name == app.toolbar_text(command)
            screenshot(app.frame, settings[2] / ('toolbar-%d.bmp' % index))
        button.Click()
        n.wait(lambda: n.is_visible(app.sources), 'toolbar opens metadata')


def infotip(app, control, row, image):
    assert control and n.is_visible(control)
    item = app.rows(control)[row]
    item.GetScrollItemPattern().ScrollIntoView()
    tooltip = auto.ToolTipControl(searchDepth=3, Compare=lambda item, depth: item.ProcessId == app.process.pid)
    with InputState():
        auto.ControlFromHandle(app.frame).SetActive()
        assert win32gui.GetForegroundWindow() == app.frame, 'infotip foreground'
        _, _, width, height = n.rectangle(control, client=True)
        x, y = win32gui.ClientToScreen(control, (width - 8, height - 8))
        auto.MoveTo(x, y)
        n.wait(lambda: not tooltip.Exists(0, 0) or not n.is_visible(tooltip.NativeWindowHandle), 'infotip hidden')
        time.sleep(.1)
        bounds = item.BoundingRectangle
        auto.MoveTo(min(bounds.left + 120, bounds.right - 8), bounds.top + bounds.height() // 4)
        n.wait(lambda: tooltip.Exists(0, 0) and n.is_visible(tooltip.NativeWindowHandle), 'native infotip')
        time.sleep(.1)
        text = tooltip.Name
        assert n.is_visible(tooltip.NativeWindowHandle)
        screenshot(tooltip.NativeWindowHandle, image)
        return text


@pytest.mark.interactive
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
    app.command(1021)
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


@pytest.mark.interactive
def test_infotips_include_values_and_full_guids(launch, settings):
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    for index, query, labels in [
        (1, 'value = 5', ('Value: 5', '值：5')),
        (1, 'guid = "0000000c-0000-0000-c000-000000000046"', ('GUID: ', 'GUID：')),
    ]:
        assert auto.ControlFromHandle(mode).Select(app.choices(mode)[index])
        app.query(query)
        assert app.count() > 0
        tip = infotip(app, app.results, 0, settings[2] / ('value-tooltip-%d.bmp' % index))
        assert any(label in tip for label in labels), tip
        if query.startswith('guid'):
            assert '0000000c-0000-0000-c000-000000000046' in tip.lower()

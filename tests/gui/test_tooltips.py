import shutil
import time
import pytest
import win32api
import win32con
import win32gui
from support import native as n
from support.input_state import InputState
from support.diagnostics import screenshot


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
        (2, '5', ('Value: 5', '值：5')),
        (3, '0000000c00000000c000000000000046', ('GUID: ', 'GUID：')),
    ]:
        n.send(mode, win32con.CB_SETCURSEL, index)
        app.command(1114, code=win32con.CBN_SELENDOK)
        app.query(query)
        assert app.count() > 0
        tip = infotip(app, app.results, 0, settings[2] / ('value-tooltip-%d.bmp' % index))
        assert any(label in tip for label in labels), tip
        if index == 3:
            assert '0000000c-0000-0000-c000-000000000046' in tip.lower()

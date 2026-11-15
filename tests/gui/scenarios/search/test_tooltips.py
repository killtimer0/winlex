"""Accessible names and infotips expose complete paths, values and failures."""
import shutil
import pytest
import win32gui
import uiautomation as auto
from support import deadline, messages, native as n
from support.input_state import InputState
from support.diagnostics import screenshot
from support.pe import is_64_bit
from support.session import select_combo
from driver.tooltips import infotip, registered_tip


@pytest.fixture
def tooltip_pointer():
    with InputState():
        yield


def test_search_tooltip_registers_and_tracks_mode_without_hover(launch, settings):
    app = launch()
    selector = n.get_item(app.frame, 1114)
    choices = app.choices(selector)
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
    tooltip = n.send(toolbar, messages.TB_GETTOOLTIPS)
    assert tooltip and n.is_window(tooltip), 'toolbar has a native tooltip'
    initial = n.send(tooltip, messages.TTM_GETDELAYTIME, 3) / 1000
    autopop = n.send(tooltip, messages.TTM_GETDELAYTIME, 2) / 1000
    assert initial > 0 and autopop > 0, 'native tooltip must have finite display delays'
    with InputState(), n.window_dpi_context(app.frame):
        with deadline.operation('activate toolbar tooltip frame'):
            auto.ControlFromHandle(app.frame).SetActive()
            assert win32gui.GetForegroundWindow() == app.frame, 'toolbar foreground'
        for index, command in enumerate((1040, 1021)):
            with deadline.operation('leave toolbar tooltip target'), n.physical_dpi_context():
                left, top, right, bottom = n.titlebar_rectangle(app.frame)
                auto.MoveTo((left + right) // 2, (top + bottom) // 2, waitTime=0)
            n.wait(lambda: not n.is_visible(tooltip),
                   'toolbar tooltip hidden', autopop + 1)
            with deadline.operation('hover toolbar tooltip target'):
                expected = app.toolbar_text(command)
                button = auto.ControlFromHandle(toolbar).Control(Name=expected)
                bounds = button.BoundingRectangle
                auto.MoveTo(bounds.xcenter(), bounds.ycenter(), waitTime=0)
            n.wait(lambda: n.is_visible(tooltip) and n.window_text(tooltip) == expected,
                   'toolbar tooltip content', initial + autopop)
            screenshot(app.frame, settings[2] / ('toolbar-%d.png' % index))
        with deadline.operation('open metadata through toolbar'):
            button.Click(waitTime=0)
            n.wait(lambda: n.is_visible(app.sources), 'toolbar opens metadata')


@pytest.mark.interactive
@pytest.mark.usefixtures('tooltip_pointer')
def test_structured_infotips_preserve_paths_and_report_reload_errors(launch, settings):
    from support.paths import extended
    from driver.metadata import complete_list_text
    directory = settings[2] / ('长路径' * 40) / ('metadata-' * 12)
    extended(directory).mkdir(parents=True, exist_ok=True)
    path = directory / '来源-😀.winmd'
    shutil.copyfile(settings[1], extended(path))
    app = launch([path])
    app.query('CreateFileW')
    tip = infotip(app, app.results, 0, settings[2] / 'search-tooltip.png')
    assert tip.startswith(app.cell(0).splitlines()[0] + '\r\n\r\n')
    assert 'Namespace: Windows.Win32.' in tip or '命名空间：Windows.Win32.' in tip
    assert str(path) in tip
    app.command(1021)
    source = infotip(app, app.sources, 0, settings[2] / 'source-tooltip.png')
    assert source.startswith(path.name + '\r\n\r\n')
    assert str(path) in source and str(extended(path).stat().st_size) in source
    extended(path).unlink()
    app.select(0, app.sources)
    app.command(1044)
    bits = 64 if is_64_bit(settings[0]) else 32
    n.wait(lambda: (lambda lines: len(lines) > 1 and lines[1].startswith(('Reload failed', '重载失败')))(
        complete_list_text(app, app.sources, 0, bits).splitlines()),
           'reload failure', 45)
    tip = infotip(app, app.sources, 0, settings[2] / 'source-error-tooltip.png')
    assert 'Error: ' in tip or '错误：' in tip
    assert 'Previously loaded metadata is still available.' in tip or '先前加载的元数据仍可使用。' in tip


@pytest.mark.interactive
@pytest.mark.usefixtures('tooltip_pointer')
def test_infotips_include_values_and_full_guids(launch, settings):
    app = launch([settings[1]])
    mode = n.get_item(app.frame, 1114)
    select_combo(mode, app.choices(mode)[1])
    for index, query, labels in [
        (1, 'value = 5', ('Value: 5', '值：5')),
        (2, 'guid = "0000000c-0000-0000-c000-000000000046"', ('GUID: ', 'GUID：')),
    ]:
        app.query(query)
        tip = infotip(app, app.results, 0, settings[2] / ('value-tooltip-%d.png' % index))
        assert any(label in tip for label in labels), tip
        if query.startswith('guid'):
            assert '0000000c-0000-0000-c000-000000000046' in tip.lower()

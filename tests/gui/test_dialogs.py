import pytest
import uiautomation as auto
from support import native as n
from support.session import invoke
from test_tools import tool, choose_scope, body


@pytest.mark.parametrize('loaded', [False, True])
def test_native_picker_cancel_keeps_application_state(launch, profile, settings, loaded):
    app = launch([settings[1]] if loaded else [])
    if loaded:
        app.query('CreateFileW')
        assert app.count() > 0
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == int(loaded), 'source state before picker', 45)
    source_count = app.count(app.sources)
    app.command(1040)
    before = (app.status_text(), app.count())
    first = app.cell(0) if loaded else None
    enabled = n.api(n.user, "IsWindowEnabled", n.w.BOOL, n.w.HWND)
    app.command(1020)
    picker = auto.WindowControl(searchDepth=3, ClassName='#32770',
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    assert picker.Exists(5)
    dialog = picker.NativeWindowHandle
    assert not enabled(app.frame)
    invoke(picker.ButtonControl(AutomationId='2'))
    n.wait(lambda: not n.is_window(dialog) and enabled(app.frame), "picker cancellation")
    assert not app.documents()
    assert (app.status_text(), app.count()) == before
    if loaded:
        assert app.cell(0) == first
    assert profile.get("AutoLoad") is None
    app.command(1021)
    assert app.count(app.sources) == source_count


def test_module_picker_uses_system_dialog_and_cancel_preserves_request(launch):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    code, module = n.get_item(doc, 1810), n.get_item(doc, 1813)
    auto.ControlFromHandle(code).GetValuePattern().SetValue('0x20000001')
    auto.ControlFromHandle(module).GetValuePattern().SetValue(r'C:\unchanged\messages.dll')
    app.focus(module)
    pending = app.status_text()
    app.focus(code)
    n.wait(lambda: app.status_text() != pending, 'module language enumeration failed')
    before = (n.window_text(code), n.window_text(module), body(doc), app.status_text())
    invoke(n.get_item(doc, 1814))
    picker = auto.WindowControl(searchDepth=3, ClassName='#32770',
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    assert picker.Exists(5)
    dialog = picker.NativeWindowHandle
    invoke(picker.ButtonControl(AutomationId='2'))
    n.wait(lambda: not n.is_window(dialog), 'module picker cancellation')
    assert (n.window_text(code), n.window_text(module), body(doc), app.status_text()) == before


def test_module_picker_accepts_resource_path(launch, settings):
    from test_tools import query

    module = settings[0].with_name('messages-x64.dll')
    if not module.exists():
        pytest.skip('bilingual message resource fixture was not deployed')
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    invoke(n.get_item(doc, 1814))
    picker = auto.WindowControl(searchDepth=3, ClassName='#32770',
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    assert picker.Exists(5)
    dialog = picker.NativeWindowHandle
    edit = picker.ComboBoxControl(AutomationId='1148').EditControl()
    assert edit.Exists(5), 'native filename edit is available'
    edit.GetValuePattern().SetValue(str(module))
    invoke(picker.ButtonControl(AutomationId='1'))
    n.wait(lambda: not n.is_window(dialog), 'module picker accepted the selected file')
    n.wait(lambda: n.window_text(n.get_item(doc, 1813)) == str(module), 'selected module path')
    language = n.get_item(doc, 1812)
    n.wait(lambda: len(app.choices(language)) == 3, 'selected module languages')
    auto.ControlFromHandle(language).GetValuePattern().SetValue('0x0409')
    result = query(app, doc, '0x20000001')
    assert 'Fixture' in result and '%1' in result and str(module) in result

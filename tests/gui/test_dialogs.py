import win32con
import pytest
from support import native as n
from test_tools import tool, choose_scope, body


@pytest.mark.parametrize('loaded', [False, True])
def test_native_picker_cancel_keeps_application_state(launch, profile, settings, loaded):
    app = launch([settings[1]] if loaded else [])
    if loaded:
        app.query('CreateFileW')
        assert app.count() > 0
    before = (app.status_text(), app.count(), app.count(app.sources))
    first = app.cell(0) if loaded else None
    owner = n.api(n.user, "GetWindow", n.w.HWND, n.w.HWND, n.w.UINT)
    enabled = n.api(n.user, "IsWindowEnabled", n.w.BOOL, n.w.HWND)

    def dialogs():
        return [h for h in n.windows() if n.class_name(h) == "#32770"
                and owner(h, 4) == app.frame and n.is_visible(h)]

    assert n.post(app.frame, win32con.WM_COMMAND, 1020, 0)
    dialog = n.wait(lambda: next(iter(dialogs()), None), "owned system picker")
    assert not enabled(app.frame)
    assert n.post(dialog, win32con.WM_COMMAND, win32con.IDCANCEL, 0)
    n.wait(lambda: not dialogs() and enabled(app.frame), "picker cancellation")
    assert not app.documents()
    assert (app.status_text(), app.count(), app.count(app.sources)) == before
    if loaded:
        assert app.cell(0) == first
    assert profile.get("AutoLoad") is None
    n.send(app.frame, win32con.WM_NULL)  # the owner remains responsive; no error or fallback dialog


def test_module_picker_uses_system_dialog_and_cancel_preserves_request(launch):
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    code, module = n.get_item(doc, 1810), n.get_item(doc, 1813)
    n.set_text(code, '0x20000001')
    n.set_text(module, r'C:\unchanged\messages.dll')
    before = (n.window_text(code), n.window_text(module), body(doc), app.status_text())
    assert n.post(doc, win32con.WM_COMMAND, 1814, 0)
    dialog = n.wait(lambda: next((h for h in n.windows()
                                  if n.class_name(h) == '#32770' and n.is_visible(h)
                                  and n.get_pid(h, None) == n.get_pid(doc, None)), None),
                    'module file picker')
    assert n.post(dialog, win32con.WM_COMMAND, win32con.IDCANCEL, 0)
    n.wait(lambda: not n.is_window(dialog), 'module picker cancellation')
    n.send(doc, win32con.WM_NULL)
    assert (n.window_text(code), n.window_text(module), body(doc), app.status_text()) == before


def test_module_picker_accepts_resource_path(launch, settings):
    from test_tools import query

    module = settings[0].with_name('messages-x64.dll')
    if not module.exists():
        pytest.skip('bilingual message resource fixture was not deployed')
    app = launch()
    doc = tool(app)
    choose_scope(app, doc, 5)
    assert n.post(doc, win32con.WM_COMMAND, 1814, 0)
    dialog = n.wait(lambda: next((h for h in n.windows()
                                  if n.class_name(h) == '#32770' and n.is_visible(h)
                                  and n.get_pid(h, None) == n.get_pid(doc, None)), None),
                    'module file picker')
    filename = n.get_item(dialog, 1148)
    edit = next((h for h in n.windows(filename) if n.class_name(h) == 'Edit'), None) if filename else None
    edit = edit or n.get_item(dialog, 1152)
    assert edit and n.is_visible(edit), 'native filename edit is available'
    n.set_text(edit, str(module))
    assert n.post(dialog, win32con.WM_COMMAND, win32con.IDOK, 0)
    n.wait(lambda: not n.is_window(dialog), 'module picker accepted the selected file')
    n.wait(lambda: n.window_text(n.get_item(doc, 1813)) == str(module), 'selected module path')
    language = n.get_item(doc, 1812)
    n.wait(lambda: n.send(language, win32con.CB_GETCOUNT) == 3, 'selected module languages')
    n.set_text(language, '0x0409')
    app.command(1812, doc, win32con.CBN_EDITCHANGE)
    result = query(app, doc, '0x20000001')
    assert 'Fixture' in result and '%1' in result and str(module) in result

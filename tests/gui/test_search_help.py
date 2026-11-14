import pytest
import win32con
import win32gui
import winreg
import uiautomation as auto
from support import deadline, native as n
from support.session import invoke
from test_document import resize


def open_help(app):
    app.command(1053)
    return n.wait(lambda: n.find_window(app.process.pid, '#32770', app.frame), 'search help dialog', 15)


@pytest.mark.parametrize('language,mode', [
    ('en-US', 'pmv1'), ('zh-CN', 'pmv2'), ('en-US', 'system'), ('zh-CN', 'unaware')])
def test_search_help_readonly_resize_and_dpi(launch, profile, language, mode):
    profile.set('Language', winreg.REG_SZ, language)
    app = launch(dpi=mode)
    dialog = open_help(app)
    edit, button = n.get_item(dialog, 1410), n.get_item(dialog, 1)
    assert not win32gui.IsWindowEnabled(app.frame)
    assert win32gui.IsWindowEnabled(edit)
    text = n.window_text(edit)
    assert win32gui.GetWindowLong(edit, win32con.GWL_STYLE) & win32con.ES_READONLY
    app.focus(edit)
    n.send(edit, win32con.EM_SETSEL, 0, 4)
    assert n.send(edit, win32con.EM_GETSEL) == 4 << 16
    n.send(edit, win32con.WM_CHAR, ord('X'), 0)
    assert n.window_text(edit) == text
    with n.window_dpi_context(dialog):
        outer, before_edit, before_button = [n.rectangle(h) for h in (dialog, edit, button)]
        resize(dialog, outer[2] - outer[0] + 120, outer[3] - outer[1] + 80)
        after_edit, after_button = n.rectangle(edit), n.rectangle(button)
        assert after_edit[:2] == before_edit[:2]
        assert (after_edit[2] - before_edit[2], after_edit[3] - before_edit[3]) == (120, 80)
        assert (after_button[0] - before_button[0], after_button[1] - before_button[1]) == (120, 80)
    with n.window_dpi_context(dialog):
        monitors = n.monitors()
    for monitor in monitors * (2 if len(monitors) > 1 else 1):
        with n.window_dpi_context(dialog):
            with deadline.operation('move search help across monitors via UIA'):
                control = auto.ControlFromHandle(dialog)
                target = (monitor[0] + 40, monitor[1] + 40)
                assert control.GetPattern(auto.PatternId.TransformPattern).Move(*target, waitTime=0)

                def moved():
                    bounds = control.BoundingRectangle
                    return (bounds.left, bounds.top) == target

                n.wait(moved, 'search help reached requested screen position', 15)
            left, top = win32gui.ClientToScreen(dialog, (0, 0))
            _, _, width, height = win32gui.GetClientRect(dialog)
            for control in (edit, button):
                r = n.rectangle(control)
                assert left <= r[0] < r[2] <= left + width
                assert top <= r[1] < r[3] <= top + height
            assert n.rectangle(edit)[3] <= n.rectangle(button)[1]
    assert n.window_text(edit) == text
    key = win32con.VK_RETURN if language == 'en-US' else win32con.VK_ESCAPE
    win32gui.PostMessage(edit, win32con.WM_KEYDOWN, key, 0)
    n.wait(lambda: not n.is_window(dialog), 'close help from edit keyboard focus')
    assert win32gui.IsWindowEnabled(app.frame)
    dialog = open_help(app)
    invoke(n.get_item(dialog, 1))
    n.wait(lambda: not n.is_window(dialog), 'help close button')

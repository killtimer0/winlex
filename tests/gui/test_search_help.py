import pytest
import win32con
import win32gui
import winreg
from support import native as n


def open_help(app):
    assert n.post(app.frame, win32con.WM_COMMAND, 1053, 0)
    return n.wait(lambda: next((h for h in n.windows()
        if n.class_name(h) == '#32770' and n.is_visible(h)
        and win32gui.GetWindow(h, win32con.GW_OWNER) == app.frame
        and n.get_item(h, 1410)), None), 'search syntax dialog')


@pytest.mark.parametrize('language,title,close', [
    ('en-US', 'Search syntax', 'Close'), ('zh-CN', '搜索语法', '关闭')], ids=['english', 'chinese'])
@pytest.mark.parametrize('mode', ['pmv1', 'pmv2', 'system', 'unaware'])
def test_search_help_readonly_resize_and_dpi(launch, profile, language, title, close, mode):
    profile.set('Preferences', winreg.REG_SZ, language + ':7')
    app = launch(dpi=mode)
    dialog = open_help(app)
    edit, button = n.get_item(dialog, 1410), n.get_item(dialog, 1)
    assert n.window_text(dialog) == title
    assert n.window_text(button) == close
    assert not win32gui.IsWindowEnabled(app.frame)
    assert win32gui.IsWindowEnabled(edit)
    text = n.window_text(edit)
    assert 'name contains "access" and value = 5' in text
    assert '\r\n\r\n' in text
    assert win32gui.GetWindowLong(edit, win32con.GWL_STYLE) & win32con.ES_READONLY
    n.send(dialog, win32con.WM_NEXTDLGCTL, edit, 1)
    n.send(edit, win32con.EM_SETSEL, 0, 4)
    assert n.send(edit, win32con.EM_GETSEL) == 4 << 16
    n.send(edit, win32con.WM_CHAR, ord('X'), 0)
    assert n.window_text(edit) == text
    with n.window_dpi_context(dialog):
        outer, before_edit, before_button = [n.rectangle(h) for h in (dialog, edit, button)]
        n.set_pos(dialog, None, 0, 0, outer[2] - outer[0] + 120, outer[3] - outer[1] + 80,
                  win32con.SWP_NOMOVE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
        after_edit, after_button = n.rectangle(edit), n.rectangle(button)
        assert after_edit[:2] == before_edit[:2]
        assert (after_edit[2] - before_edit[2], after_edit[3] - before_edit[3]) == (120, 80)
        assert (after_button[0] - before_button[0], after_button[1] - before_button[1]) == (120, 80)
    for monitor in n.monitors() * 2:
        with n.window_dpi_context(dialog):
            n.set_pos(dialog, None, monitor[0] + 40, monitor[1] + 40, 0, 0,
                      win32con.SWP_NOSIZE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
            n.send(dialog, win32con.WM_NULL)
            left, top = win32gui.ClientToScreen(dialog, (0, 0))
            _, _, width, height = win32gui.GetClientRect(dialog)
            for control in (edit, button):
                r = n.rectangle(control)
                assert left <= r[0] < r[2] <= left + width
                assert top <= r[1] < r[3] <= top + height
            assert n.rectangle(edit)[3] <= n.rectangle(button)[1]
    assert n.window_text(edit) == text
    key = win32con.VK_RETURN if language == 'en-US' else win32con.VK_ESCAPE
    n.post(edit, win32con.WM_KEYDOWN, key, 0)
    n.wait(lambda: not n.is_window(dialog), 'close help from edit keyboard focus')
    assert win32gui.IsWindowEnabled(app.frame)
    dialog = open_help(app)
    n.post(n.get_item(dialog, 1), win32con.BM_CLICK, 0, 0)
    n.wait(lambda: not n.is_window(dialog), 'help close button')

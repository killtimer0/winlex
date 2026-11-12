import pytest
import win32con
import win32gui
import winreg
import uiautomation as auto
from support import native as n
from support.session import invoke


def open_help(app):
    app.command(1053)
    dialog = auto.WindowControl(searchDepth=3, RegexName='^(Search syntax|搜索语法)$',
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    assert dialog.Exists(5)
    return dialog.NativeWindowHandle


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
    editor = auto.ControlFromHandle(edit)
    app.focus(edit)
    win32gui.SendMessage(edit, win32con.EM_SETSEL, 0, 4)
    assert n.send(edit, win32con.EM_GETSEL) == 4 << 16
    win32gui.SendMessage(edit, win32con.WM_CHAR, ord('X'), 0)
    assert n.window_text(edit) == text
    with n.window_dpi_context(dialog):
        outer, before_edit, before_button = [n.rectangle(h) for h in (dialog, edit, button)]
        auto.ControlFromHandle(dialog).GetPattern(auto.PatternId.TransformPattern).Resize(
            outer[2] - outer[0] + 120, outer[3] - outer[1] + 80)
        after_edit, after_button = n.rectangle(edit), n.rectangle(button)
        assert after_edit[:2] == before_edit[:2]
        assert (after_edit[2] - before_edit[2], after_edit[3] - before_edit[3]) == (120, 80)
        assert (after_button[0] - before_button[0], after_button[1] - before_button[1]) == (120, 80)
    monitors = n.monitors()
    for monitor in monitors * (2 if len(monitors) > 1 else 1):
        with n.window_dpi_context(dialog):
            auto.ControlFromHandle(dialog).GetPattern(auto.PatternId.TransformPattern).Move(
                monitor[0] + 40, monitor[1] + 40)
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

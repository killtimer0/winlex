import json
import pytest
import win32api
import win32con
import win32gui
import winreg
import uiautomation as auto
from support import native as n
from support.diagnostics import screenshot
from support.input_state import InputState, foreground
from support.session import invoke


def open_about(app):
    app.command(1054)
    dialog = auto.WindowControl(searchDepth=3, RegexName='^(About Winlex|关于 Winlex)$',
                                Compare=lambda item, depth: item.ProcessId == app.process.pid)
    assert dialog.Exists(5)
    return dialog.NativeWindowHandle


def link_item(app, control, index=0):
    links = [child for child in auto.ControlFromHandle(control).GetChildren()
             if child.ControlType == auto.ControlType.HyperlinkControl]
    link = links[index]
    legacy = link.GetLegacyIAccessiblePattern()
    state = (1 if legacy.State & 4 else 0) | (2 if link.IsEnabled else 0)
    return legacy.Value, state


@pytest.mark.parametrize('language,mode', [
    ('en-US', 'pmv2'), ('zh-CN', 'pmv1'), ('en-US', 'system'), ('zh-CN', 'unaware')])
def test_about_content_modality_and_dpi(launch, profile, settings, language, mode):
    profile.set('Language', winreg.REG_SZ, language)
    app = launch(dpi=mode)
    dialog = open_about(app)
    assert not win32gui.IsWindowEnabled(app.frame)
    info = n.get_item(dialog, 1403)
    text = n.window_text(info)
    assert 'Winlex' in text
    version = win32api.GetFileVersionInfo(str(settings[0]), '\\')
    ms, ls = version['FileVersionMS'], version['FileVersionLS']
    assert '.'.join(map(str, [ms >> 16, ms & 65535, ls >> 16, ls & 65535])) in text
    assert n.send(n.get_item(dialog, 1400), win32con.STM_GETICON)
    for index, url in enumerate(('https://github.com/killtimer0/winlex',
                                 'https://github.com/killtimer0/winlex/blob/main/LICENSE')):
        actual, state = link_item(app, info, index)
        assert actual == url and state & 2
    evidence = []
    for cycle in range(3):
        for monitor in n.monitors():
            with n.window_dpi_context(dialog):
                auto.ControlFromHandle(dialog).GetPattern(auto.PatternId.TransformPattern).Move(
                    monitor[0] + 40, monitor[1] + 40)
                left, top = win32gui.ClientToScreen(dialog, (0, 0))
                _, _, width, height = win32gui.GetClientRect(dialog)
                for child, _ in auto.WalkControl(auto.ControlFromHandle(dialog)):
                    if not child.NativeWindowHandle or child.IsOffscreen:
                        continue
                    r = child.BoundingRectangle
                    assert left <= r.left < r.right <= left + width, (child.Name, r)
                    assert top <= r.top < r.bottom <= top + height, (child.Name, r)
                evidence.append({'cycle': cycle, 'monitor': monitor, 'bounds': n.rectangle(dialog)})
                if cycle == 2:
                    screenshot(dialog, settings[2] / ('about-%s-%s-%d.bmp' % (language, mode, monitor[0])))
    for index in range(len(n.monitors())):
        sizes = [(e['bounds'][2] - e['bounds'][0], e['bounds'][3] - e['bounds'][1])
                 for e in evidence[index::len(n.monitors())]]
        assert all(abs(w-sizes[-1][0]) <= 1 and abs(h-sizes[-1][1]) <= 1 for w, h in sizes[1:])
    (settings[2] / ('about-%s-%s.json' % (language, mode))).write_text(json.dumps(evidence), encoding='utf-8')
    key = win32con.VK_RETURN if language == 'en-US' else win32con.VK_ESCAPE
    win32gui.PostMessage(n.get_item(dialog, 1), win32con.WM_KEYDOWN, key, 0)
    n.wait(lambda: not n.is_window(dialog), 'about keyboard close')
    assert win32gui.IsWindowEnabled(app.frame)
    dialog = open_about(app)
    invoke(n.get_item(dialog, 1))
    n.wait(lambda: not n.is_window(dialog), 'about button close')


@pytest.mark.interactive
def test_about_real_keyboard(launch):
    app = launch()
    with InputState() as inputs, foreground(app, inputs):
        dialog = open_about(app)
        n.wait(lambda: win32gui.GetForegroundWindow() == dialog, 'about foreground')
        assert app.focused() == n.get_item(dialog, 1)
        inputs.key(win32con.VK_TAB)
        info = n.get_item(dialog, 1403)
        n.wait(lambda: app.focused() == info and link_item(app, info, 0)[1] & 1,
               'homepage keyboard focus')
        inputs.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == info and link_item(app, info, 1)[1] & 1,
               'license keyboard focus')
        inputs.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1), 'return to OK')
        inputs.key(win32con.VK_ESCAPE)
        n.wait(lambda: not n.is_window(dialog), 'real Escape closes about')
        dialog = open_about(app)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1), 'default OK focus')
        inputs.key(win32con.VK_RETURN)
        n.wait(lambda: not n.is_window(dialog), 'real Enter closes about')

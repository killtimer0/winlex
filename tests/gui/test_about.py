import ctypes as c
import json
import struct
import pytest
import win32api
import win32con
import win32gui
import winreg
from support import native as n
from support import messages
from support.diagnostics import screenshot
from support.input_state import InputState, foreground


def open_about(app):
    assert n.post(app.frame, win32con.WM_COMMAND, 1054, 0)
    return n.wait(lambda: next((h for h in n.windows()
        if n.class_name(h) == '#32770' and win32gui.GetWindow(h, win32con.GW_OWNER) == app.frame
        and n.is_visible(h) and n.get_item(h, 1400)), None), 'about dialog')


def link_item(app, control):
    # commctrl.h: LITEM contains four 32-bit fields, a 48-WCHAR ID and a 2084-WCHAR URL.
    item = c.create_string_buffer(16 + (48 + 2084) * 2)
    struct.pack_into('<4I', item, 0, 1 | 2 | 8, 0, 0, 2)  # LIF_ITEMINDEX | LIF_STATE | LIF_URL
    assert n.write_memory(app.handle, app.memory, item, len(item), None)
    assert n.send(control, messages.LM_GETITEM, 0, app.memory)
    assert n.read_memory(app.handle, app.memory, item, len(item), None)
    return app.remote_text(app.memory + 16 + 48 * 2, 2084 * 2), bool(struct.unpack_from('<I', item, 8)[0] & 2)


@pytest.mark.parametrize('language,title,ok', [('en-US', 'About Winlex', 'OK'), ('zh-CN', '关于 Winlex', '确定')], ids=['english', 'chinese'])
@pytest.mark.parametrize('mode', ['pmv2', 'pmv1', 'system', 'unaware'])
def test_about_content_modality_and_dpi(launch, profile, settings, language, title, ok, mode):
    profile.set('Preferences', winreg.REG_SZ, language + ':7')
    app = launch(dpi=mode)
    dialog = open_about(app)
    assert not win32gui.IsWindowEnabled(app.frame)
    assert n.window_text(dialog) == title
    labels = [n.window_text(h) for h in n.windows(dialog) if n.class_name(h) == 'Static']
    assert 'Winlex' in labels
    version = win32api.GetFileVersionInfo(str(settings[0]), '\\')
    ms, ls = version['FileVersionMS'], version['FileVersionLS']
    assert '.'.join(map(str, [ms >> 16, ms & 65535, ls >> 16, ls & 65535])) in labels
    assert n.window_text(n.get_item(dialog, 1)) == ok
    assert n.send(n.get_item(dialog, 1400), win32con.STM_GETICON)
    assert link_item(app, n.get_item(dialog, 1403)) == ('https://github.com/killtimer0/winlex', True)
    assert link_item(app, n.get_item(dialog, 1404)) == ('https://github.com/killtimer0/winlex/blob/main/LICENSE', True)
    evidence = []
    for cycle in range(3):
        for monitor in n.monitors():
            with n.window_dpi_context(dialog):
                n.set_pos(dialog, None, monitor[0] + 40, monitor[1] + 40, 0, 0,
                          win32con.SWP_NOSIZE | win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
                n.send(dialog, win32con.WM_NULL)
                left, top = win32gui.ClientToScreen(dialog, (0, 0))
                _, _, width, height = win32gui.GetClientRect(dialog)
                for child in n.windows(dialog):
                    r = n.rectangle(child)
                    assert left <= r[0] < r[2] <= left + width, (n.get_id(child), r)
                    assert top <= r[1] < r[3] <= top + height, (n.get_id(child), r)
                evidence.append({'cycle': cycle, 'monitor': monitor, 'bounds': n.rectangle(dialog)})
                if cycle == 2:
                    screenshot(dialog, settings[2] / ('about-%s-%s-%d.bmp' % (language, mode, monitor[0])))
    for index in range(len(n.monitors())):
        sizes = [(e['bounds'][2] - e['bounds'][0], e['bounds'][3] - e['bounds'][1])
                 for e in evidence[index::len(n.monitors())]]
        assert all(abs(w-sizes[-1][0]) <= 1 and abs(h-sizes[-1][1]) <= 1 for w, h in sizes[1:])
    (settings[2] / ('about-%s-%s.json' % (language, mode))).write_text(json.dumps(evidence), encoding='utf-8')
    key = win32con.VK_RETURN if language == 'en-US' else win32con.VK_ESCAPE
    assert n.post(n.get_item(dialog, 1), win32con.WM_KEYDOWN, key, 0)
    n.wait(lambda: not n.is_window(dialog), 'about keyboard close')
    assert win32gui.IsWindowEnabled(app.frame)
    dialog = open_about(app)
    assert n.post(n.get_item(dialog, 1), win32con.BM_CLICK, 0, 0)
    n.wait(lambda: not n.is_window(dialog), 'about button close')


@pytest.mark.interactive
def test_about_real_keyboard(launch):
    app = launch()
    with InputState() as inputs, foreground(app, inputs):
        dialog = open_about(app)
        n.wait(lambda: win32gui.GetForegroundWindow() == dialog, 'about foreground')
        assert app.focused() == n.get_item(dialog, 1)
        inputs.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1403), 'homepage keyboard focus')
        inputs.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1404), 'license keyboard focus')
        inputs.key(win32con.VK_TAB)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1), 'return to OK')
        inputs.key(win32con.VK_ESCAPE)
        n.wait(lambda: not n.is_window(dialog), 'real Escape closes about')
        dialog = open_about(app)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1), 'default OK focus')
        inputs.key(win32con.VK_RETURN)
        n.wait(lambda: not n.is_window(dialog), 'real Enter closes about')

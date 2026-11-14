import json
import pytest
import win32api
import win32con
import win32gui
import win32process
import winreg
import uiautomation as auto
from support import deadline, native as n
from support.diagnostics import capture, screenshot
from support.input_state import InputState, foreground
from support.session import invoke


ABOUT_URLS = {
    'https://github.com/killtimer0/winlex',
    'https://github.com/killtimer0/winlex/blob/main/CHANGELOG.md',
    'https://github.com/killtimer0/winlex/blob/main/LICENSE',
}


def open_about(app):
    app.command(1054)
    return n.wait(lambda: n.find_window(app.process.pid, '#32770', app.frame), 'About dialog', 15)


def link_item(app, control, index=0):
    class LinkItem(n.c.Structure):
        _fields_ = [('mask', n.w.UINT), ('index', n.c.c_int), ('state', n.w.UINT),
                    ('state_mask', n.w.UINT), ('id', n.w.WCHAR * 48), ('url', n.w.WCHAR * 2084)]

    item = LinkItem(mask=11, index=index, state_mask=3)
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ
                                   | win32con.PROCESS_VM_WRITE, False, app.process.pid)
    memory = None
    try:
        memory = win32process.VirtualAllocEx(process, 0, n.c.sizeof(item),
                                             win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                             win32con.PAGE_READWRITE)
        assert n.write_process_memory(int(process), memory, n.c.byref(item), n.c.sizeof(item), None)
        if not n.send(control, 0x0703, 0, memory):  # LM_GETITEM
            return None
        assert n.read_process_memory(int(process), memory, n.c.byref(item), n.c.sizeof(item), None)
        return item.url, item.state
    finally:
        if memory is not None:
            win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        win32api.CloseHandle(process)


def link_items(app, control):
    items = []
    for index in range(64):
        item = link_item(app, control, index)
        if item is None:
            return items
        items.append(item)
    raise AssertionError('About link enumeration exceeded safety limit')


@deadline.operation('move and measure About child windows via UIA')
def measure_children(dialog, monitor):
    auto.ControlFromHandle(dialog).GetPattern(auto.PatternId.TransformPattern).Move(
        monitor[0] + 40, monitor[1] + 40, waitTime=0)
    left, top = win32gui.ClientToScreen(dialog, (0, 0))
    _, _, width, height = win32gui.GetClientRect(dialog)
    children = []
    win32gui.EnumChildWindows(dialog, lambda hwnd, _: children.append(hwnd), None)
    for hwnd in children:
        if not n.is_visible(hwnd):
            continue
        # Only HWND-backed controls were checked by the original geometry
        # contract. Do not traverse the SysLink's virtual UIA descendants.
        child = auto.ControlFromHandle(hwnd)
        r = child.BoundingRectangle
        assert left <= r.left < r.right <= left + width, (hwnd, r)
        assert top <= r.top < r.bottom <= top + height, (hwnd, r)


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
    enabled_urls = {url for url, state in link_items(app, info) if state & 2}
    assert ABOUT_URLS <= enabled_urls, ABOUT_URLS - enabled_urls
    evidence = []
    for cycle in range(3):
        for monitor in n.monitors():
            with n.window_dpi_context(dialog):
                measure_children(dialog, monitor)
                evidence.append({'cycle': cycle, 'monitor': monitor, 'bounds': n.rectangle(dialog)})
    for index in range(len(n.monitors())):
        sizes = [(e['bounds'][2] - e['bounds'][0], e['bounds'][3] - e['bounds'][1])
                 for e in evidence[index::len(n.monitors())]]
        assert all(abs(w-sizes[-1][0]) <= 1 and abs(h-sizes[-1][1]) <= 1 for w, h in sizes[1:-1])
    (settings[2] / ('about-%s-%s.json' % (language, mode))).write_text(json.dumps(evidence), encoding='utf-8')
    key = win32con.VK_RETURN if language == 'en-US' else win32con.VK_ESCAPE
    win32gui.PostMessage(n.get_item(dialog, 1), win32con.WM_KEYDOWN, key, 0)
    n.wait(lambda: not n.is_window(dialog), 'about keyboard close')
    assert win32gui.IsWindowEnabled(app.frame)
    dialog = open_about(app)
    invoke(n.get_item(dialog, 1))
    n.wait(lambda: not n.is_window(dialog), 'about button close')


@pytest.mark.interactive
@pytest.mark.parametrize('theme', ['light', 'dark'])
def test_about_real_keyboard(launch, profile, settings, theme):
    profile.set('Theme', winreg.REG_SZ, theme)
    app = launch()
    with InputState() as inputs, foreground(app, inputs):
        dialog = open_about(app)
        n.wait(lambda: win32gui.GetForegroundWindow() == dialog, 'about foreground')
        assert app.focused() == n.get_item(dialog, 1)
        info = n.get_item(dialog, 1403)

        def focus_target():
            hwnd = app.focused()
            url = None
            if hwnd == info:
                url = next((url for url, state in link_items(app, info) if state & 1), None)
            return hwnd, url

        visited = set()
        previous = focus_target()
        for _ in range(64):
            inputs.key(win32con.VK_TAB)

            def changed_focus():
                current = focus_target()
                return current if current != previous else None

            current = n.wait(changed_focus, 'Tab moves About focus')
            if current[0] == n.get_item(dialog, 1):
                break
            if current[1] is not None:
                visited.add(current[1])
            previous = current
        else:
            pytest.fail('About Tab traversal did not return to OK within safety limit')
        assert ABOUT_URLS <= visited, ABOUT_URLS - visited

        def painted():
            with n.window_dpi_context(dialog), capture(dialog) as (_, _, pixels):
                return any(pixels[index:index + 4] != pixels[:4]
                           for index in range(4, len(pixels), 4))

        n.wait(painted, 'about pixels presented')
        with n.window_dpi_context(dialog):
            screenshot(dialog, settings[2] / ('about-keyboard-' + theme + '.png'))
        inputs.key(win32con.VK_ESCAPE)
        n.wait(lambda: not n.is_window(dialog), 'real Escape closes about')
        dialog = open_about(app)
        n.wait(lambda: app.focused() == n.get_item(dialog, 1), 'default OK focus')
        inputs.key(win32con.VK_RETURN)
        n.wait(lambda: not n.is_window(dialog), 'real Enter closes about')

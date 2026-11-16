"""Representative public input/appearance wiring, not a visual matrix."""
import ctypes as c
from ctypes import wintypes as w
import json
from pathlib import Path
import shutil
import struct

import pytest
import win32api
import win32con
import win32gui
import win32process

from driver import auto, until


def capture(app, name):
    destination = app.output / name
    app.evidence(destination)
    assert (destination / 'screen.bmp').is_file(), 'Missing appearance evidence: ' + str(destination)


def focused(app):
    class GUIThreadInfo(c.Structure):
        _fields_ = [('cbSize', w.DWORD), ('flags', w.DWORD),
                    ('hwndActive', w.HWND), ('hwndFocus', w.HWND),
                    ('hwndCapture', w.HWND), ('hwndMenuOwner', w.HWND),
                    ('hwndMoveSize', w.HWND), ('hwndCaret', w.HWND), ('rcCaret', w.RECT)]
    info = GUIThreadInfo(cbSize=c.sizeof(GUIThreadInfo))
    get_info = c.windll.user32.GetGUIThreadInfo
    get_info.argtypes, get_info.restype = [w.DWORD, c.POINTER(GUIThreadInfo)], w.BOOL
    assert get_info(win32process.GetWindowThreadProcessId(app.frame)[0], c.byref(info))
    return info.hwndFocus if win32gui.GetForegroundWindow() == app.frame else None


@pytest.mark.real_input
def test_keyboard_regions_find_escape_history_and_close(app):
    app.keyboard_search('CreateFileW')
    first = app.editor()
    until('Enter enters declaration', lambda: focused(app) == first.handle)
    auto.AU3_Send('^l', 0)
    until('Ctrl+L focuses search', lambda: focused(app) == app.child(1102))
    auto.AU3_Send('{F6}', 0)
    until('F6 leaves sidebar', lambda: focused(app) not in (None, app.child(1102)))
    auto.AU3_Send('{ENTER}', 0)
    until('tab Enter returns to declaration', lambda: focused(app) == first.handle)

    auto.AU3_Send('^f', 0)
    edit = until('find input focused', lambda: app.child(1220, first.window()))
    until('Ctrl+F focuses find', lambda: focused(app) == edit)
    auto.AU3_Send('lpFileName', 1)
    auto.AU3_Send('{ENTER}', 0)  # Commit possible IME composition, as in keyboard_search.
    until('typed find query', lambda: app.text(edit) == 'lpFileName')
    until('keyboard find selects parameter', lambda: first.selected_text() == 'lpFileName')
    auto.AU3_Send('{ESC}', 0)
    until('Escape returns to body', lambda: focused(app) == first.handle
          and not win32gui.IsWindowVisible(edit))

    app.keyboard_search('CloseHandle')
    second_window = app.editor().window()
    auto.AU3_Send('!{LEFT}', 0)
    app.wait_declaration('CreateFileW')
    auto.AU3_Send('!{RIGHT}', 0)
    app.wait_declaration('CloseHandle')
    auto.AU3_Send('^{F4}', 0)
    until('Ctrl+F4 closes document', lambda: not win32gui.IsWindow(second_window))
    app.wait_declaration('CreateFileW')
    capture(app, 'keyboard-reading')


def test_unicode_multi_file_shell_drop_adds_searchable_sources(app):
    fixtures = Path(__file__).with_name('fixtures')
    paths = [app.output / '拖放 one.winmd', app.output / '拖放 two.winmd']
    for source, destination in zip(('before.winmd', 'after.winmd'), paths):
        shutil.copyfile(fixtures / source, destination)
    # Documented DROPFILES layout (20 bytes on both x86/x64), wide double-NUL list.
    payload = struct.pack('<IiiII', 20, 0, 0, 0, 1)
    payload += ('\0'.join(map(str, paths)) + '\0\0').encode('utf-16-le')
    kernel = c.windll.kernel32
    kernel.GlobalAlloc.argtypes, kernel.GlobalAlloc.restype = [w.UINT, c.c_size_t], w.HGLOBAL
    kernel.GlobalLock.argtypes, kernel.GlobalLock.restype = [w.HGLOBAL], c.c_void_p
    kernel.GlobalUnlock.argtypes, kernel.GlobalUnlock.restype = [w.HGLOBAL], w.BOOL
    kernel.GlobalFree.argtypes, kernel.GlobalFree.restype = [w.HGLOBAL], w.HGLOBAL
    handle = kernel.GlobalAlloc(0x42, len(payload))  # GMEM_MOVEABLE | GMEM_ZEROINIT
    assert handle
    transferred = False
    try:
        memory = kernel.GlobalLock(handle)
        assert memory
        c.memmove(memory, payload, len(payload))
        kernel.GlobalUnlock(handle)
        app.record('shell drop two Unicode metadata paths')
        win32gui.PostMessage(app.frame, win32con.WM_DROPFILES, handle, 0)
        transferred = True  # Receiver owns HDROP and releases it with DragFinish.
    finally:
        if not transferred:
            kernel.GlobalFree(handle)
    for path in paths:
        app.source_index(path)
    assert 'ReloadVersionTwo' in app.open_symbol('ReloadVersionTwo').text_block()
    app.close_source(paths[1])
    until('second dropped source removed', lambda: app.source_count() == 2)
    assert 'OriginalField' in app.open_symbol('ReloadSnapshot').text_block()


def test_language_applies_only_after_restart_and_can_return_to_english(app, ui_text, request):
    library = win32api.LoadLibraryEx(request.config.getoption('--exe'), 0, 2)
    try:
        block = win32api.LoadResource(library, 6, 2080 // 16 + 1, 0x804)
        size = struct.unpack_from('<H', block)[0] * 2
        chinese_caption = block[2:2 + size].decode('utf-16-le')
    finally:
        win32api.FreeLibrary(library)
    assert chinese_caption and chinese_caption != ui_text(2080)
    sheet = app.settings()
    sheet.select(1300, ui_text(2084))
    sheet.apply()
    assert app.text(sheet.handle) == ui_text(2080)
    sheet.cancel()
    sheet = app.settings()
    assert app.text(sheet.handle) == ui_text(2080)
    sheet.cancel()
    app.restart()
    sheet = app.settings()
    assert app.text(sheet.handle) == chinese_caption
    sheet.select(1300, ui_text(2083))  # Language self-names are shared resources.
    sheet.ok()
    assert 'CreateFileW' in app.open_symbol('CreateFileW').text_block()
    app.restart()
    sheet = app.settings()
    assert app.text(sheet.handle) == ui_text(2080)
    sheet.cancel()


def test_theme_apply_preserves_reading_and_search_at_target_dpi(app, ui_text):
    class HighContrast(c.Structure):
        _fields_ = [('cbSize', w.UINT), ('dwFlags', w.DWORD), ('scheme', w.LPWSTR)]
    contrast = HighContrast(cbSize=c.sizeof(HighContrast))
    spi = c.windll.user32.SystemParametersInfoW
    spi.argtypes, spi.restype = [w.UINT, w.UINT, c.c_void_p, w.UINT], w.BOOL
    assert spi(0x0042, contrast.cbSize, c.byref(contrast), 0)  # SPI_GETHIGHCONTRAST only.
    dc = win32gui.GetDC(app.frame)
    try:
        get_caps = c.windll.gdi32.GetDeviceCaps
        get_caps.argtypes, get_caps.restype = [w.HDC, c.c_int], c.c_int
        dpi = get_caps(dc, 88)  # LOGPIXELSX; also available on XP.
    finally:
        win32gui.ReleaseDC(app.frame, dc)
    get_dpi = getattr(c.windll.user32, 'GetDpiForWindow', None)
    if get_dpi is not None:
        get_dpi.argtypes, get_dpi.restype = [w.HWND], w.UINT
        dpi = get_dpi(app.frame)
    (app.output / 'appearance-environment.json').write_text(json.dumps(dict(
        window_dpi=dpi, high_contrast=bool(contrast.dwFlags & 1))), encoding='utf-8')
    document = app.open_symbol('IStream')
    original = document.text_block()
    sheet = app.settings()
    if not win32gui.IsWindowEnabled(sheet.control(1301)):
        sheet.cancel()
        capture(app, 'native-appearance')
        pytest.skip('Target uses native system appearance; theme preference unavailable')
    sheet.cancel()
    for resource in (2089, 2088, 2087):  # Dark, light, system: no OS-global mutation.
        sheet = app.settings()
        sheet.select(1301, ui_text(resource))
        sheet.ok()
        document.activate()
        assert document.text_block() == original
        app.find('Read')
        assert document.selection_indices()[1] > document.selection_indices()[0]
        capture(app, 'theme-' + str(resource))
        assert 'CloseHandle' in app.open_symbol('CloseHandle').text_block()
        sheet = app.settings()
        assert sheet.value(1301) == ui_text(resource)
        sheet.cancel()

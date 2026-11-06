import os
import shutil
import struct
import pytest
import winreg
import win32con
import win32api
import win32gui
import win32process
import uiautomation as auto
from support import native as n
from support.messages import LVM_GETITEMTEXTW
from support.pe import is_64_bit
from test_context import selection_packed
from test_metadata_guide import resource_text


def complete_list_text(app, hwnd, row, bits):
    process = win32api.OpenProcess(win32con.PROCESS_VM_OPERATION | win32con.PROCESS_VM_READ
                                  | win32con.PROCESS_VM_WRITE, False, app.process.pid)
    memory = None
    capacity = 32768
    try:
        memory = win32process.VirtualAllocEx(process, 0, 128 + capacity * 2,
                                            win32con.MEM_COMMIT | win32con.MEM_RESERVE,
                                            win32con.PAGE_READWRITE)
        item = bytearray(128)
        struct.pack_into('<III', item, 0, 1, row, 0)
        struct.pack_into('<Q' if bits == 64 else '<I', item, 24 if bits == 64 else 20, memory + 128)
        struct.pack_into('<I', item, 32 if bits == 64 else 24, capacity)
        item_buffer = n.c.create_string_buffer(bytes(item))
        if not n.write_process_memory(int(process), memory, item_buffer, len(item), None):
            raise n.c.WinError(n.c.get_last_error())
        count = n.send(hwnd, LVM_GETITEMTEXTW, row, memory)
        assert 0 <= count < capacity - 1, 'Native list text exceeds measurement buffer'
        data = n.c.create_string_buffer(count * 2)
        size_read = n.c.c_size_t()
        if not n.read_process_memory(int(process), memory + 128, data, len(data), n.c.byref(size_read)):
            raise n.c.WinError(n.c.get_last_error())
        return data.raw[:size_read.value].decode('utf-16-le', 'surrogatepass')
    finally:
        try:
            if memory is not None:
                win32process.VirtualFreeEx(process, memory, 0, win32con.MEM_RELEASE)
        finally:
            win32api.CloseHandle(process)


@pytest.mark.parametrize("sidebar_width", [140, 480])
def test_source_actions_remain_visible_and_disjoint(launch, profile, sidebar_width):
    profile.set("SidebarWidth", winreg.REG_DWORD, sidebar_width)
    app = launch()
    app.command(1021)
    controls = [n.get_item(app.frame, ident) for ident in (1020, 1044, 1022)]
    with n.window_dpi_context(app.frame):
        bounds = n.rectangle(app.sources)
        rectangles = [n.rectangle(control) for control in controls]
        for index, (x1, y1, x2, y2) in enumerate(rectangles):
            assert n.is_visible(controls[index])
            assert bounds[0] <= x1 < x2 <= bounds[2]
            assert y1 < y2 < bounds[1]
            for other in rectangles[index + 1:]:
                assert x2 <= other[0] or other[2] <= x1 or y2 <= other[1] or other[3] <= y1


def test_source_actions_preserve_identity_and_recover_failed_reload(launch, profile, settings, ui_language):
    paths = [settings[2] / name / "same.winmd" for name in ("first", "second", "third")]
    for path in paths:
        path.parent.mkdir(exist_ok=True)
        shutil.copyfile(settings[1], path)
    app = launch()
    app.pick(paths)
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 3, "sources loaded", 45)
    reload_button, remove_button = (n.get_item(app.frame, i) for i in (1044, 1022))
    assert not win32gui.IsWindowEnabled(reload_button)
    assert not win32gui.IsWindowEnabled(remove_button)
    app.select(1, app.sources)
    assert win32gui.IsWindowEnabled(reload_button) and win32gui.IsWindowEnabled(remove_button)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 2, "middle source removed", 45)
    assert app.selected_row(app.sources) == 1
    assert app.cell(1, app.sources).splitlines()[0] == str(paths[2])
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 1, "last source removed", 45)
    assert app.selected_row(app.sources) == 0
    saved = profile.get("AutoLoad")
    assert saved == [str(paths[0])]
    failed = resource_text(settings[0], 2102, ui_language.langid)
    loaded = resource_text(settings[0], 2050, ui_language.langid)
    paths[0].write_bytes(b"invalid replacement")
    app.command(1044)
    n.wait(lambda: app.cell(0, app.sources).splitlines()[1].startswith(failed),
           "reload failure on retained row", 45)
    assert app.selected_row(app.sources) == 0
    app.query("CreateFileW")
    assert app.count() > 0
    assert profile.get("AutoLoad") == saved
    app.command(1021)
    assert app.cell(0, app.sources).splitlines()[1].startswith(failed)
    shutil.copyfile(settings[1], paths[0])
    app.command(1044)
    n.wait(lambda: app.cell(0, app.sources).splitlines()[1].startswith(loaded), "reload recovered", 45)
    assert app.selected_row(app.sources) == 0
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0, "all removed", 45)
    assert not win32gui.IsWindowEnabled(reload_button) and not win32gui.IsWindowEnabled(remove_button)


@pytest.mark.parametrize('mouse', [False, pytest.param(True, marks=pytest.mark.interactive)],
                         ids=['menu-contract', 'clipboard'])
def test_source_context_menu_copies_full_native_path(launch, settings, mouse):
    from test_context import find_popup, popup, wait_closed, clipboard_text, wait_clipboard_available
    second = settings[2] / "右键来源.winmd"
    shutil.copyfile(settings[1], second)
    app = launch([settings[1], second])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 2, "sources loaded", 45)
    app.select(0, app.sources)
    app.focus(app.sources)
    if mouse:
        wait_clipboard_available()
    if mouse:
        with n.window_dpi_context(app.frame):
            left, top, right, bottom = app.item_rect(1, app.sources)
            x, y = win32gui.ClientToScreen(app.sources, (left + 8, (top + bottom) // 2))
            _, ids = popup(app, app.sources, location=(x & 0xffff) | ((y & 0xffff) << 16), mouse=True)
    else:
        win32gui.PostMessage(app.sources, win32con.WM_CONTEXTMENU, app.sources, -1)
        _, ids = find_popup(app)
    assert {1044, 1022, 1052} <= set(ids)
    if mouse:
        actions = [ident for ident in ids if ident and ident != 0xffffffff]
        auto.SendKeys('{Down %d}{Enter}' % (actions.index(1052) + 1))
    else:
        menu = auto.MenuControl(searchDepth=3, ClassName='#32768', ProcessId=app.process.pid)
        win32gui.PostMessage(menu.NativeWindowHandle, win32con.WM_KEYDOWN, win32con.VK_ESCAPE, 1)
    wait_closed(app.sources)
    if mouse:
        n.wait(lambda: clipboard_text() == str(second), "source path copied")
    assert app.selected_row(app.sources) == int(mouse)


def test_open_loads_and_close_keeps_document_snapshot(launch, profile, settings):
    app = launch()
    app.pick([settings[1]])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "file loaded", 45)
    n.wait(lambda: profile.get("AutoLoad") == [str(settings[1])], "successful files saved")
    app.query("CreateFileW")
    entity = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(entity), "declaration", 45)
    old = app.content(entity)
    editor = n.get_item(entity, 1203)
    n.send(editor, win32con.EM_SETSEL, 2, 8)
    assert selection_packed(editor) == (8 << 16) | 2
    app.command(1021)
    app.select(0, app.sources)
    app.command(1022)
    n.wait(lambda: app.count(app.sources) == 0 and profile.get("AutoLoad") == [], "source closed")
    app.command(1040)
    assert app.count() == 0
    assert app.content(entity) == old
    assert selection_packed(editor) == (8 << 16) | 2
    n.send(editor, win32con.EM_SETSEL, 0, 2)
    assert selection_packed(editor) == 2 << 16
    app.choose(entity, "arm64")
    n.wait(lambda: 'CreateFileW(' in app.content(entity), "query from closed snapshot", 45)
    assert "arm64" in n.window_text(entity)
    assert app.documents() == [entity]
    app.close()
    restored = launch()
    restored.command(1021)
    assert restored.count(restored.sources) == 0


def test_restore_successful_files(launch, profile, settings):
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch()
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "saved source loaded", 45)
    assert str(settings[1]) in app.cell(0, app.sources)
    app.query("CreateFileW")
    assert app.count() > 0
    app.close()
    restored = launch()
    restored.command(1021)
    n.wait(lambda: restored.count(restored.sources) == 1, "source restored again", 45)


def test_temporary_inputs_preserve_saved_files(launch, profile, settings, ui_language):
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch([str(settings[2] / "missing.winmd")])
    if not n.is_visible(app.sources):
        app.command(1021)
    failed = resource_text(settings[0], 2022, ui_language.langid).split('%1!s!')[0]
    n.wait(lambda: failed in app.status_text(), "load error")
    assert app.count(app.sources) == 0
    assert profile.get("AutoLoad") == [str(settings[1])]
    app.close()
    temporary = launch([settings[1]])
    if not n.is_visible(temporary.sources):
        temporary.command(1021)
    n.wait(lambda: temporary.count(temporary.sources) == 1, "temporary input loaded", 45)
    temporary.select(0, temporary.sources)
    temporary.command(1022)
    n.wait(lambda: temporary.count(temporary.sources) == 0, "temporary source closed")
    assert profile.get("AutoLoad") == [str(settings[1])]
    temporary.close()
    restored = launch()
    if not n.is_visible(restored.sources):
        restored.command(1021)
    n.wait(lambda: restored.count(restored.sources) == 1, "saved input restored after temporary session", 45)
    restored.query("CreateFileW")
    assert restored.count() > 0


def test_failed_restore_preserves_saved_files(launch, profile, settings, ui_language):
    missing = str(settings[2] / "missing-restored.winmd")
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [missing])
    app = launch()
    if not n.is_visible(app.sources):
        app.command(1021)
    failed = resource_text(settings[0], 2022, ui_language.langid).split('%1!s!')[0]
    n.wait(lambda: failed in app.status_text(), "restore error")
    assert profile.get("AutoLoad") == [missing]
    assert app.count(app.sources) == 0


def test_multiple_positional_inputs_preserve_order(launch, profile, settings):
    first = settings[2] / "first source.winmd"
    second = settings[2] / "第二个来源.winmd"
    for path in (first, second):
        shutil.copyfile(settings[1], path)
    profile.set("AutoLoad", winreg.REG_MULTI_SZ, [str(settings[1])])
    app = launch([first, second])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 2, "both inputs loaded", 45)
    assert str(first) in app.cell(0, app.sources)
    assert str(second) in app.cell(1, app.sources)
    assert profile.get("AutoLoad") == [str(settings[1])]
    app.query("CreateFileW")
    assert len(app.named_rows("CreateFileW")) == 2


def test_long_unicode_input(launch, settings):
    from support.paths import extended
    directory = settings[2] / ("长路径" * 40) / ("metadata-" * 12)
    extended(directory).mkdir(parents=True, exist_ok=True)
    path = directory / "来源.winmd"
    assert len(str(path)) > 260
    shutil.copyfile(settings[1], extended(path))
    app = launch([path])
    app.command(1021)
    n.wait(lambda: app.count(app.sources) == 1, "long path loaded", 45)
    bits = 64 if is_64_bit(settings[0]) else 32
    assert complete_list_text(app, app.sources, 0, bits).splitlines()[0] == str(path)
    app.query("CreateFileW")
    assert complete_list_text(app, app.results, 0, bits).splitlines()[3] == str(path)
    doc = app.open_row()
    n.wait(lambda: 'CreateFileW(' in app.content(doc), "declaration", 45)
    assert str(path) not in app.content(doc)
    assert path.name in n.window_text(n.get_item(doc, 1205))


@pytest.mark.parametrize('unc', [False, True], ids=['local', 'unc'])
def test_list_text_preserves_native_utf16_path(launch, settings, unc):
    path = settings[2] / "source-\ud800.winmd"
    if unc:
        from pathlib import Path
        path = Path('\\\\' + os.environ['COMPUTERNAME'] + '\\' + path.drive[0] + '$' + str(path)[2:])
        if not os.path.isdir(path.parent):
            pytest.skip('worker cannot access its local administrative share')
    shutil.copyfile(settings[1], path)
    app = launch([path])
    try:
        app.command(1021)
        n.wait(lambda: app.count(app.sources) == 1, "source loaded", 45)
        assert app.cell(0, app.sources).splitlines()[0] == str(path)
        app.query("CreateFileW")
        lines = app.cell(0).splitlines()
        assert lines[1].endswith(path.name)
        assert lines[3] == str(path)
        doc = app.open_row()
        n.wait(lambda: "CreateFileW(" in app.content(doc), "declaration", 45)
        assert path.name in n.window_text(n.get_item(doc, 1205))
    finally:
        app.close()
        path.unlink()

"""Representative public tool workflows with Windows/ABI reference values."""
import ctypes
from ctypes import wintypes
import re
import uuid
import winreg

import pytest
import win32clipboard
import win32con
import win32gui

from driver import auto, select_combo, until


class Tool:
    """Test-local wrapper for a tool's standard public controls."""

    def __init__(self, app, ui_text, title_id, input_id, result_id):
        self.app = app
        app.command(1800)  # PANEL_TOOLS
        listing = until('tools launcher', lambda: app.child(1801))
        title = ui_text(title_id)

        def row():
            for index in range(int(app.list_command(listing, 'GetItemCount'))):
                if app.list_command(listing, 'GetText', str(index)).split('\r\n', 1)[0] == title:
                    return index + 1
            return None

        index = until('tool launcher row ' + title, row) - 1
        app.record('open tool ' + title)
        app.list_command(listing, 'SelectClear')
        app.list_command(listing, 'Select', str(index))
        app.command(1802)  # TOOL_NEW

        def opened():
            document = app.document()
            app.child(input_id, document)
            app.child(result_id, document)
            return document

        self.handle = until('tool document ' + title, opened)
        self.input_id, self.result_id = input_id, result_id

    def control(self, identifier):
        return self.app.child(identifier, self.handle)

    def enter(self, value):
        self.app.record('tool input ' + value)
        self.app.set_text(self.control(self.input_id), value)
        if self.input_id == 1820:
            # Multiline Edit WM_SETTEXT does not generate EN_CHANGE. Deliver
            # the standard edit notification after setting its public text.
            win32gui.SendMessage(self.handle, win32con.WM_COMMAND,
                                 self.input_id | (win32con.EN_CHANGE << 16),
                                 self.control(self.input_id))

    def select(self, identifier, value):
        self.app.record('tool select {} {}'.format(identifier, value))
        select_combo(self.control(identifier), value, win32con.CBN_SELENDOK)

    def click(self, identifier):
        self.app.record('tool click {}'.format(identifier))
        win32gui.SendMessage(self.control(identifier), win32con.BM_CLICK, 0, 0)

    def body(self):
        return self.app.text(self.control(self.result_id))


def system_message(code):
    """Independent Windows message lookup, in the target's default language."""
    buffer = ctypes.create_unicode_buffer(4096)
    format_message = ctypes.WinDLL('kernel32', use_last_error=True).FormatMessageW
    format_message.argtypes = [wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD,
                              wintypes.DWORD, wintypes.LPWSTR, wintypes.DWORD,
                              ctypes.c_void_p]
    format_message.restype = wintypes.DWORD
    count = format_message(0x1000 | 0x200, None, code, 0, buffer, len(buffer), None)
    assert count, 'Independent FormatMessageW({}) failed: {}'.format(code, ctypes.get_last_error())
    message = buffer.value.rstrip('\r\n')
    assert message, 'Independent system message is empty'
    return message


@pytest.mark.parametrize('title,input_id,result_id,options', [
    (2303, 1810, 1816, (1811,)),
    (2350, 1820, 1823, (1821,)),
    (2400, 1830, 1835, (1831, 1832)),
])
def test_fixed_options_stay_compact_while_input_grows(app, ui_text, title, input_id, result_id, options):
    tool = Tool(app, ui_text, title, input_id, result_id)
    win32gui.ShowWindow(tool.handle, win32con.SW_RESTORE)
    available = win32gui.GetClientRect(win32gui.GetParent(tool.handle))

    def widths(width):
        win32gui.SetWindowPos(tool.handle, 0, 0, 0, width, available[3],
                              win32con.SWP_NOZORDER | win32con.SWP_NOACTIVATE)
        return [win32gui.GetClientRect(tool.control(identifier))[2]
                for identifier in (*options, input_id)]

    narrow = widths(available[2] * 4 // 5)
    wide = widths(available[2])
    # Fixed choices do not absorb extra space; free-form input does.
    assert all(narrow) and all(wide)
    assert wide[:-1] == narrow[:-1]
    assert wide[-1] > narrow[-1]


def test_error_code_system_message_invalid_and_recovery(app, ui_text):
    denied, invalid_handle = system_message(5), system_message(6)
    tool = Tool(app, ui_text, 2303, 1810, 1816)
    tool.select(1811, ui_text(2315))  # Win32
    tool.select(1812, ui_text(2319))  # Automatic language

    def query(code, message):
        tool.enter(str(code))
        tool.click(1815)

        def complete():
            body = tool.body()
            assert win32gui.IsWindowEnabled(tool.control(1815))
            assert '0x{:08X}'.format(code) in body and message in body
            return body

        return until('system error {} result'.format(code), complete)

    previous = query(5, denied)  # ERROR_ACCESS_DENIED, independently defined by Win32
    tool.enter('6')
    assert tool.body() == previous
    assert app.text(app.child(1105)) == ui_text(2312)
    tool.select(1811, ui_text(2314))  # Auto; option changes also require Query.
    assert tool.body() == previous
    assert app.text(app.child(1105)) == ui_text(2312)
    tool.enter('0xGG')
    tool.click(1815)

    def rejected():
        assert app.text(app.child(1105)).startswith(ui_text(2334))
        assert tool.body() == previous
        return True

    until('invalid code preserves successful result', rejected)
    recovered = query(6, invalid_handle)  # ERROR_INVALID_HANDLE
    assert denied not in recovered


def test_demangle_known_cpp_symbols_and_live_updates(app, ui_text):
    tool = Tool(app, ui_text, 2350, 1820, 1823)

    def restored(patterns, absent=()):
        def complete():
            body = tool.body()
            for pattern in patterns:
                assert re.search(pattern, body, re.MULTILINE), body
            for text in absent:
                assert text not in body
            return True
        until('independently known demangled signatures', complete)

    # Itanium grammar: 3foo is the name, i is its int parameter.
    tool.select(1821, ui_text(2356))
    tool.enter('_Z3fooi')
    foo = r'^\s*foo\(int\)\s*$'
    restored([foo])

    # MSVC free function: int __cdecl func(int).
    tool.enter('?func@@YAHH@Z')
    tool.select(1821, ui_text(2355))
    func = r'^\s*int\s+__cdecl\s+func\(int\)\s*$'
    restored([func], ('foo(int)',))

    tool.select(1821, ui_text(2314))  # Auto detects both independent ABIs.
    tool.enter('_Z3fooi\r\n?func@@YAHH@Z')
    restored([foo, func])
    tool.enter('_Z3bari')  # EN_CHANGE alone must replace the old result.
    restored([r'^\s*bar\(int\)\s*$'], ('foo(int)', 'func(int)'))


REGISTRY_ROOTS = (winreg.HKEY_CURRENT_USER, winreg.HKEY_LOCAL_MACHINE)
CLASSES = 'Software\\Classes\\'


def registry_default(root, path):
    """Read the same explicit 32-bit OS view independently of Winlex."""
    try:
        with winreg.OpenKey(root, path, 0, winreg.KEY_READ | winreg.KEY_WOW64_32KEY) as key:
            try:
                value, kind = winreg.QueryValueEx(key, '')
            except FileNotFoundError:
                return None
    except FileNotFoundError:
        return None
    assert kind in (winreg.REG_SZ, winreg.REG_EXPAND_SZ), 'Unexpected registry string type: ' + path
    return winreg.ExpandEnvironmentStrings(value) if kind == winreg.REG_EXPAND_SZ else value


def registry_key_exists(root, path):
    try:
        with winreg.OpenKey(root, path, 0, winreg.KEY_READ | winreg.KEY_WOW64_32KEY):
            return True
    except FileNotFoundError:
        return False


def test_com_builtin_registration_missing_and_recovery(app, ui_text):
    progid = 'Shell.Application'
    clsid = '{13709620-C279-11CE-A49E-444553540000}'
    mappings = [registry_default(root, CLASSES + progid + '\\CLSID')
                for root in REGISTRY_ROOTS]
    mappings = [value for value in mappings if value]
    assert mappings, 'Prerequisite missing: Shell.Application registration in explicit 32-bit registry view'
    assert all(value.upper() == clsid for value in mappings), 'Unexpected Shell.Application CLSID: ' + repr(mappings)

    descriptions, registered_progids = set(), set()
    for root in REGISTRY_ROOTS:
        path = CLASSES + 'CLSID\\' + clsid
        description = registry_default(root, path)
        if description:
            descriptions.add(description)
        for subkey in ('ProgID', 'VersionIndependentProgID'):
            value = registry_default(root, path + '\\' + subkey)
            if value:
                registered_progids.add(value)
    assert descriptions and registered_progids, (
        'Prerequisite missing: Shell.Application class description/ProgID in 32-bit registry view')

    # Verify absence in both real roots before asking Winlex; never write keys.
    missing = '{' + str(uuid.uuid4()).upper() + '}'
    assert not any(registry_key_exists(root, CLASSES + 'CLSID\\' + missing)
                   for root in REGISTRY_ROOTS), 'Generated missing CLSID unexpectedly registered'

    tool = Tool(app, ui_text, 2400, 1830, 1835)
    tool.select(1831, ui_text(2407))  # ProgID
    tool.select(1832, ui_text(2411))  # Explicit x86 registry view, not process default

    def positive():
        body = tool.body()
        assert clsid in body.upper()
        for value in descriptions | registered_progids:
            assert value in body
        return True

    tool.enter(progid)
    tool.click(1833)
    until('built-in Shell.Application registration', positive)

    tool.select(1831, ui_text(2408))  # GUID; changing options requires Query.
    tool.enter(missing)
    tool.click(1833)
    diagnostic = ui_text(2434).replace('%1!s!', missing)

    def not_found():
        body = tool.body()
        assert diagnostic in body
        assert clsid not in body.upper()
        for value in descriptions | registered_progids:
            assert value not in body
        return True

    until('verified absent CLSID replaces positive report', not_found)
    tool.enter(clsid)
    tool.click(1833)
    until('COM registration recovers after absent CLSID', positive)
    # Deliberately never invoke COM_TYPELIB (1836) or COM_ACTIVATE (1837).


def test_com_conditions_require_explicit_query(app, ui_text):
    tool = Tool(app, ui_text, 2400, 1830, 1835)
    tool.enter('Shell.Application')
    tool.select(1832, ui_text(2411))  # Explicit x86 view.
    tool.click(1833)
    clsid = '{13709620-C279-11CE-A49E-444553540000}'

    def queried():
        assert clsid in tool.body().upper()
        assert win32gui.IsWindowEnabled(tool.control(1837))
        return tool.body()

    previous = until('COM result ready for activation', queried)

    def stale():
        assert tool.body() == previous, 'Conditions must not implicitly replace results'
        assert not win32gui.IsWindowEnabled(tool.control(1836))
        assert not win32gui.IsWindowEnabled(tool.control(1837))
        assert app.text(app.child(1105)) == ui_text(2312)
        return True

    tool.enter(clsid)
    until('edited COM input invalidates old actions', stale)
    tool.click(1833)
    previous = until('COM input submitted explicitly', queried)
    tool.select(1831, ui_text(2408))  # GUID.
    until('COM kind change preserves stale result', stale)
    tool.click(1833)
    previous = until('COM kind submitted explicitly', queried)
    tool.select(1832, ui_text(2314))  # Auto view.
    until('COM view change preserves stale result', stale)
    tool.click(1833)
    until('explicit COM query restores actions', queried)


@pytest.mark.real_input
@pytest.mark.parametrize('title,input_id,result_id,copy_id,value,expected', [
    (2303, 1810, 1816, 1818, '5', '0x00000005'),
    (2350, 1820, 1823, 1822, '_Z3fooi', 'foo(int)'),
    (2400, 1830, 1835, 1834, 'Shell.Application',
     '{13709620-C279-11CE-A49E-444553540000}'),
])
def test_tool_keyboard_and_copy(app, ui_text, title, input_id, result_id,
                               copy_id, value, expected):
    tool = Tool(app, ui_text, title, input_id, result_id)
    tool.enter(value)
    assert auto.AU3_WinActivateByHandle(app.frame)
    assert auto.AU3_ControlFocusByHandle(app.frame, tool.control(input_id))
    if input_id != 1820:  # Multiline symbol input remains live; Enter is a newline.
        auto.AU3_Send('{ENTER}', 0)
    until('keyboard tool result', lambda: expected in tool.body())
    win32clipboard.OpenClipboard()
    try:
        win32clipboard.EmptyClipboard()
    finally:
        win32clipboard.CloseClipboard()
    tool.click(copy_id)

    def copied():
        win32clipboard.OpenClipboard()
        try:
            return expected in win32clipboard.GetClipboardData(win32con.CF_UNICODETEXT)
        finally:
            win32clipboard.CloseClipboard()

    until('copy tool result', copied)
    app.evidence(app.output / ('tool-' + str(input_id)))

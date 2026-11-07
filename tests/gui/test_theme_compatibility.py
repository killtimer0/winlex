"""Native fallback on legacy Windows and real high-contrast transitions."""
from contextlib import contextmanager
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
import uuid


SPI_GETHIGHCONTRAST = 0x0042
SPI_SETHIGHCONTRAST = 0x0043
HCF_HIGHCONTRASTON = 0x0001
HCF_AVAILABLE = 0x0002


class ObscuredSample(AssertionError):
    pass


class HighContrast(c.Structure):
    _fields_ = [('size', w.UINT), ('flags', w.DWORD), ('scheme', c.c_void_p)]


def spi_high_contrast(action, state, flags=0):
    spi = c.WinDLL('user32', use_last_error=True).SystemParametersInfoW
    spi.restype, spi.argtypes = w.BOOL, (w.UINT, w.UINT, c.c_void_p, w.UINT)
    if not spi(action, state.size, c.byref(state), flags):
        raise c.WinError(c.get_last_error())


def high_contrast():
    state = HighContrast()
    state.size = c.sizeof(state)
    spi_high_contrast(SPI_GETHIGHCONTRAST, state)
    # Borrowed pointer: the 19045 native probe returned the same address for
    # four GETs, ignoring caller buffers and leaving their sentinels untouched.
    # Copy immediately; LocalFree of the cached address caused 0xc0000374.
    return state.flags, c.wstring_at(state.scheme) if state.scheme else None


def set_high_contrast(snapshot):
    flags, scheme = snapshot
    buffer = c.create_unicode_buffer(scheme) if scheme is not None else None
    state = HighContrast(c.sizeof(HighContrast), flags,
                         c.addressof(buffer) if buffer is not None else None)
    # Broadcast the real setting change, but never SPIF_UPDATEINIFILE. Do not
    # use HCF_OPTION_NOTHEMECHANGE: Windows forbids it for an ON/OFF transition.
    spi_high_contrast(SPI_SETHIGHCONTRAST, state, 0x0002)


def probe_high_contrast():
    """Read-only native probe; never free an unproven-ownership pointer."""
    states, buffers, rows = [], [], []
    for index in range(4):
        buffer = c.create_unicode_buffer('caller-buffer-sentinel', 32768)
        buffers.append(buffer)
        state = HighContrast(c.sizeof(HighContrast), 0,
                             c.addressof(buffer) if index % 2 else None)
        supplied = state.scheme
        spi_high_contrast(SPI_GETHIGHCONTRAST, state)
        states.append(state)
        rows.append({'supplied': supplied, 'returned': state.scheme,
                     'caller_buffer': buffer.value, 'flags': state.flags,
                     'scheme': c.wstring_at(state.scheme) if state.scheme else None})
    print(json.dumps({'windows': list(sys.getwindowsversion()[:3]), 'reads': rows}, indent=2))


def task_scheduler():
    import support  # Initialize XP/Python 3.8 pywin32 DLL loading, as remote.py does.
    import pythoncom
    from win32com.taskscheduler import taskscheduler as t
    return pythoncom.CoCreateInstance(t.CLSID_CTaskScheduler, None,
                                     pythoncom.CLSCTX_INPROC_SERVER, t.IID_ITaskScheduler)


def delete_watchdog_task(name):
    prefix = 'Winlex.Tests.HighContrast.'
    if not name.startswith(prefix) or uuid.UUID(hex=name[len(prefix):]).hex != name[len(prefix):]:
        raise ValueError('Invalid owned watchdog task name')
    scheduler = task_scheduler()
    if (name + '.job').casefold() in {entry.casefold() for entry in scheduler.Enum()}:
        scheduler.Delete(name)


def watchdog_path(artifact, task_name, suffix):
    return artifact.with_suffix('.' + task_name.rsplit('.', 1)[1] + suffix)


def watchdog_record(path, text):
    temporary = path.with_suffix(path.suffix + '.new')
    temporary.write_text(text, encoding='utf-8')
    temporary.replace(path)


def watchdog(parent_pid, artifact, task_name):
    """Scheduler-launched supervisor, independent of the pytest process/job."""
    kernel = c.WinDLL('kernel32', use_last_error=True)
    kernel.OpenProcess.restype, kernel.OpenProcess.argtypes = w.HANDLE, (w.DWORD, w.BOOL, w.DWORD)
    kernel.WaitForSingleObject.restype, kernel.WaitForSingleObject.argtypes = w.DWORD, (w.HANDLE, w.DWORD)
    kernel.CloseHandle.restype, kernel.CloseHandle.argtypes = w.BOOL, (w.HANDLE,)
    handle = None
    ready = watchdog_path(artifact, task_name, '.ready')
    try:
        handle = kernel.OpenProcess(0x00100000, False, parent_pid)
        if not handle:
            raise c.WinError(c.get_last_error())
        saved = json.loads(artifact.read_text(encoding='utf-8'))
        snapshot = saved['flags'], saved['scheme']
        watchdog_record(ready, str(os.getpid()))
        deadline = time.monotonic() + 120
        while artifact.exists():
            result = kernel.WaitForSingleObject(handle, 100)
            if result == 0 or time.monotonic() >= deadline:
                set_high_contrast(snapshot)
                if high_contrast() != snapshot:
                    raise RuntimeError('Watchdog could not restore entire HIGHCONTRAST snapshot')
                artifact.unlink()
                return
            if result != 0x102:
                raise RuntimeError('Watchdog parent wait failed: %#x' % result)
    finally:
        try:
            if handle:
                kernel.CloseHandle(handle)
            if ready.exists():
                ready.unlink()
        finally:
            delete_watchdog_task(task_name)


if __name__ == '__main__':
    if sys.argv[1:] == ['--probe-high-contrast']:
        probe_high_contrast()
    elif len(sys.argv) == 5 and sys.argv[1] == '--watch-high-contrast':
        if os.environ.get('PYTHONMALLOC') != 'malloc':
            # pywin32 312's Task Scheduler wrappers free Python memory while
            # the GIL is released. The scheduler does not inherit the test
            # runner's environment, so start the watchdog with the same safe
            # allocator before it uses those bindings for task cleanup.
            raise SystemExit(subprocess.call(
                [sys.executable, *sys.argv],
                env={**os.environ, 'PYTHONMALLOC': 'malloc'}))
        artifact, task_name = Path(sys.argv[3]), sys.argv[4]
        result = watchdog_path(artifact, task_name, '.result.json')
        try:
            watchdog(int(sys.argv[2]), artifact, task_name)
        except BaseException:
            watchdog_record(result, json.dumps({'ok': False, 'error': traceback.format_exc()}))
            raise SystemExit(1)
        watchdog_record(result, json.dumps({'ok': True}))
    else:
        raise SystemExit('Use --probe-high-contrast or --watch-high-contrast PID SNAPSHOT TASK')
    raise SystemExit(0)


import pytest
import win32con
import win32gui
import winreg

from support import native as n
from support.diagnostics import screenshot
from support.input_state import InputState
from test_theme import THEME_SUPPORTED, dark_background, modern_theme, selected_index
from test_theme_surfaces import colors, foreground
from test_theme_inputs import combo_info


pytestmark = pytest.mark.interactive


@contextmanager
def temporary_high_contrast(snapshot, artifact):
    # Preserve the whole structure (including the owned copy of the scheme),
    # not just the ON bit. Keep recovery evidence if the outer runner is killed.
    artifact.write_text(json.dumps({'flags': snapshot[0], 'scheme': snapshot[1]}),
                        encoding='utf-8')
    # Match remote.py's Task Scheduler v1 launch: scheduler provenance keeps
    # this supervisor independent without weakening the runner's job limits.
    import pythoncom
    import win32api
    from win32com.taskscheduler import taskscheduler as t
    name = 'Winlex.Tests.HighContrast.' + uuid.uuid4().hex
    ready = watchdog_path(artifact, name, '.ready')
    result = watchdog_path(artifact, name, '.result.json')
    created = False
    try:
        scheduler = task_scheduler()
        task = scheduler.NewWorkItem(name)
        created = True
        # An independent console can take the foreground and cover the native
        # dialog whose displayed pixels the test is measuring.
        interpreter = Path(sys.executable).with_name('pythonw.exe')
        if not interpreter.is_file():
            raise RuntimeError('High-contrast watchdog requires pythonw.exe')
        task.SetApplicationName(str(interpreter))
        task.SetParameters(subprocess.list2cmdline(
            [str(Path(__file__).resolve()), '--watch-high-contrast',
             str(os.getpid()), str(artifact), name]))
        task.SetWorkingDirectory(str(Path(__file__).resolve().parent))
        task.SetFlags(t.TASK_FLAG_RUN_ONLY_IF_LOGGED_ON)
        task.SetAccountInformation(win32api.GetUserName(), None)
        task.SetMaxRunTime(-1)
        task.QueryInterface(pythoncom.IID_IPersistFile).Save(None, True)
        task.Run()
        n.wait(lambda: ready.exists() or result.exists(), 'scheduler watchdog ready', 15)
        assert ready.exists() and not result.exists(), 'Watchdog startup failed; see %s' % result
        assert int(ready.read_text(encoding='ascii')) > 0
    except BaseException:
        artifact.unlink()
        if created:
            delete_watchdog_task(name)
        raise
    try:
        set_high_contrast((snapshot[0] | HCF_HIGHCONTRASTON, snapshot[1]))
        n.wait(lambda: high_contrast()[0] & HCF_HIGHCONTRASTON,
               'real system high contrast is enabled', 10)
        yield
    finally:
        set_high_contrast(snapshot)
        n.wait(lambda: high_contrast() == snapshot,
               'entire original HIGHCONTRAST state restored', 10)
        artifact.unlink()
        try:
            n.wait(result.exists, 'scheduler watchdog cleanup complete', 10)
            outcome = json.loads(result.read_text(encoding='utf-8'))
            assert outcome['ok'], outcome.get('error', 'High-contrast watchdog failed')
        finally:
            delete_watchdog_task(name)


def system_color(index):
    value = n.api(n.user, 'GetSysColor', n.w.DWORD, n.c.c_int)(index)
    return (value >> 16) & 255, (value >> 8) & 255, value & 255


def edit_background(hwnd):
    # Blank lower-right client area excludes text, caret and native border.
    _, _, width, height = n.rectangle(hwnd, client=True)
    return colors(hwnd, (width - 10, height - 8, width - 5, height - 4)).most_common(1)[0][0]


def set_edit_text(hwnd, text):
    value = n.c.create_unicode_buffer(text)
    assert n.send(hwnd, win32con.WM_SETTEXT, 0, n.c.addressof(value))
    assert n.window_text(hwnd) == text


def assert_dark_preference(app, profile, expected, native=False):
    sheet = app.settings()
    try:
        page = app.settings_page(sheet, 0)
        combo = n.get_item(page, 1301)
        if THEME_SUPPORTED:
            assert selected_index(app, combo) == 2
        else:
            assert not n.api(n.user, 'IsWindowEnabled', n.w.BOOL, n.w.HWND)(combo), (
                'unsupported Windows must disable the native-only theme choice')
            assert selected_index(app, combo) == 0
        if native:
            n.wait(lambda: colors(page, (3, 3, 8, 8)).most_common(1)[0][0] ==
                   system_color(win32con.COLOR_BTNFACE),
                   'settings page uses native COLOR_BTNFACE', 10)
    finally:
        app.close_settings(sheet, accept=False)
    assert profile.preferences() == expected, 'native fallback must not rewrite preferences'


@pytest.mark.skipif(THEME_SUPPORTED, reason='requires Windows before dark-mode support')
def test_legacy_dark_preference_keeps_native_edit(launch, profile, settings):
    profile.set('Theme', winreg.REG_SZ, 'dark')
    expected = profile.preferences()
    app = launch()
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        text = 'native fallback text'
        set_edit_text(app.search, text)
        assert_dark_preference(app, profile, expected, native=True)
        n.wait(lambda: edit_background(app.search) == system_color(win32con.COLOR_WINDOW),
               'legacy Edit uses native COLOR_WINDOW', 10)
        assert n.window_text(app.search) == text
        screenshot(app.frame, settings[2] / 'legacy-dark-preference-native.png')


@modern_theme
def test_actual_high_contrast_overrides_dark_and_restores(launch, profile, settings):
    snapshot = high_contrast()
    if snapshot[0] & HCF_HIGHCONTRASTON:
        pytest.skip('high contrast already enabled; do not alter the existing desktop')
    if not snapshot[0] & HCF_AVAILABLE:
        pytest.skip('system high contrast is unavailable')
    profile.set('Theme', winreg.REG_SZ, 'dark')
    expected = profile.preferences()
    app = launch()
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        text = 'high contrast preserves this text'
        set_edit_text(app.search, text)
        n.wait(lambda: dark_background(app.search), 'explicit dark before high contrast', 10)
        dark_fill = edit_background(app.search)
        regions = {}
        unobscured = set()

        def fill(hwnd):
            _, _, width, height = n.rectangle(hwnd, client=True)
            region = regions.get(hwnd, (2, 2, width - 2, height - 2))
            if callable(region):
                region = region()
            if hwnd in unobscured:
                x = (region[0] + region[2]) // 2
                y = (region[1] + region[3]) // 2
                point = win32gui.ClientToScreen(hwnd, (x, y))
                actual = win32gui.WindowFromPoint(point)
                if actual != hwnd:
                    raise ObscuredSample(str((
                        'native control sample is obscured', hwnd, n.class_name(hwnd),
                        region, point, actual, n.class_name(actual) if actual else None)))
            return colors(hwnd, region).most_common(1)[0][0]

        def wait_fill(hwnd, expected_fills, description):
            observed = []
            obstruction = []

            def sample():
                try:
                    value = fill(hwnd)
                except ObscuredSample as error:
                    obstruction[:] = [str(error)]
                    return None
                observed[:] = [value]
                expected = expected_fills() if callable(expected_fills) else expected_fills
                return value if value in expected else None

            try:
                return n.wait(sample, description, 10)
            except AssertionError as error:
                expected = expected_fills() if callable(expected_fills) else expected_fills
                raise AssertionError('%s: observed=%r, expected=%r' %
                                     (description, observed, expected) +
                                     '; cause=' + str(error) +
                                     '; obstruction=' + repr(obstruction)) from error

        dark_status = fill(app.status)
        sheet = app.settings()
        evidence = {'before': {'high_contrast': list(snapshot)}}
        evidence_path = settings[2] / 'high-contrast-controls.json'

        def record(stage, values):
            evidence[stage] = values
            evidence_path.write_text(json.dumps(evidence, indent=2), encoding='utf-8')

        try:
            page = app.settings_page(sheet, 0)
            combo = n.get_item(page, 1301)
            tabs = []
            win32gui.EnumChildWindows(sheet, lambda hwnd, _: tabs.append(hwnd)
                                     if n.class_name(hwnd) == 'SysTabControl32' else None, None)
            tab = next(iter(tabs), None)
            assert tab, 'settings exposes a native tab control'
            selected = n.send(tab, 0x130B)  # TCM_GETCURSEL; no remote pointers.
            selection = n.send(combo, win32con.CB_GETCURSEL)
            assert selection == 2
            assert not win32gui.FindWindowEx(tab, 0, 'msctls_updown32', None)
            # Property sheets use multiline tabs. Exercise the engine's supported
            # single-line overflow configuration on this already-themed HWND.
            style = win32gui.GetWindowLong(tab, win32con.GWL_STYLE)
            win32gui.SetWindowLong(tab, win32con.GWL_STYLE, style & ~0x0200)  # TCS_MULTILINE
            bounds = win32gui.GetWindowRect(tab)
            origin = win32gui.ScreenToClient(win32gui.GetParent(tab), bounds[:2])
            dpi = n.api(n.user, 'GetDpiForWindow', w.UINT, w.HWND)(tab)
            win32gui.MoveWindow(tab, origin[0], origin[1], 80 * dpi // 96,
                                bounds[3] - bounds[1], True)
            overflow = n.wait(lambda: win32gui.FindWindowEx(tab, 0, 'msctls_updown32', None),
                              'native overflow created after initial theme application')
            n.wait(lambda: n.is_visible(overflow), 'late overflow visible')
            info = combo_info(combo)
            def combo_region():
                item = combo_info(combo).item
                return (item.right - 12, item.top + 3, item.right - 4, item.bottom - 3)

            def tab_region():
                overflow_left = win32gui.ScreenToClient(tab, n.rectangle(overflow)[:2])[0]
                return (4, 4, overflow_left - 4, 8)

            regions[combo] = combo_region
            # The display area lies behind the property page. Sample only the
            # exposed header above its text, excluding the overflow child.
            regions[tab] = tab_region
            unobscured.update((combo, tab, overflow, info.list))

            def idle_pointer():
                left, top, _, _ = n.rectangle(sheet)
                assert n.api(n.user, 'SetCursorPos', w.BOOL, c.c_int, c.c_int)(left + 40, top + 10)

            idle_pointer()

            def stable_fill(hwnd):
                previous = [None, time.monotonic()]

                def sample():
                    value = fill(hwnd)
                    if value != previous[0]:
                        previous[:] = [value, time.monotonic()]
                    return value if time.monotonic() - previous[1] >= 0.3 else None

                return n.wait(sample, 'idle native background settles', 10)

            before_controls = {name: stable_fill(hwnd) for name, hwnd in
                               [('combo', combo), ('tab', tab), ('overflow', overflow)]}
            assert all(max(value) < 128 for value in before_controls.values()), (
                'all sampled controls must initially render dark', before_controls)
            record('dark_controls', before_controls)
            with temporary_high_contrast(snapshot, settings[2] / 'high-contrast-restore.json'):
                assert high_contrast()[0] & HCF_HIGHCONTRASTON
                idle_pointer()
                observed = {}
                failures = []

                def observe(name, sample):
                    try:
                        observed[name] = {'color': sample()}
                    except AssertionError as error:
                        observed[name] = {'error': str(error)}
                        failures.append(str(error))
                    record('high_contrast_observations', dict(
                        observed, high_contrast=list(high_contrast()),
                        system_window=system_color(win32con.COLOR_WINDOW),
                        system_face=system_color(win32con.COLOR_BTNFACE)))

                for name, hwnd in [('combo', combo), ('tab', tab), ('overflow', overflow)]:
                    color_index = win32con.COLOR_WINDOW if name == 'combo' else win32con.COLOR_BTNFACE
                    observe(name, lambda hwnd=hwnd, name=name, color_index=color_index:
                            wait_fill(hwnd, lambda: {system_color(color_index)},
                                      name + ' restores native high-contrast background'))
                n.send(combo, win32con.CB_SHOWDROPDOWN, 1)
                try:
                    n.wait(lambda: n.is_visible(info.list), 'high-contrast combo popup visible')
                    observe('combo_list', lambda: wait_fill(info.list,
                                                           lambda: {system_color(win32con.COLOR_WINDOW)},
                                                           'combo popup uses system colors'))
                finally:
                    n.send(combo, win32con.CB_SHOWDROPDOWN, 0)
                assert n.send(combo, win32con.CB_GETCURSEL) == selection
                assert n.send(tab, 0x130B) == selected
                assert profile.preferences() == expected
                # Hide the owned sheet only while sampling the main window so
                # displayed pixels cannot come from an overlapping dialog.
                win32gui.ShowWindow(sheet, win32con.SW_HIDE)
                try:
                    def native_edit_fill():
                        value = edit_background(app.search)
                        return value if value == system_color(win32con.COLOR_WINDOW) else None

                    observe('edit', lambda: n.wait(native_edit_fill,
                            'high contrast restores native Edit background', 10))
                    observe('status', lambda: wait_fill(app.status,
                            lambda: {system_color(win32con.COLOR_BTNFACE)},
                            'status restores native high-contrast background'))
                    assert n.window_text(app.search) == text
                    screenshot(app.frame, settings[2] / 'actual-high-contrast.png')
                finally:
                    win32gui.ShowWindow(sheet, win32con.SW_SHOWNOACTIVATE)
                record('high_contrast_controls', dict(observed,
                        system_window=system_color(win32con.COLOR_WINDOW),
                        system_face=system_color(win32con.COLOR_BTNFACE)))
                screenshot(sheet, settings[2] / 'high-contrast-native-controls.png')
                assert not failures, '\n'.join(failures)
            restored = {}
            idle_pointer()
            for name, hwnd in [('combo', combo), ('tab', tab), ('overflow', overflow)]:
                restored[name] = wait_fill(hwnd, {before_controls[name]},
                                           name + ' returns to its original dark background')
            assert n.send(combo, win32con.CB_GETCURSEL) == selection
            assert n.send(tab, 0x130B) == selected
            assert profile.preferences() == expected
            record('restored_controls', dict(restored, high_contrast=list(high_contrast())))
            screenshot(sheet, settings[2] / 'high-contrast-controls-restored-dark.png')
        finally:
            app.close_settings(sheet, accept=False)
        assert high_contrast() == snapshot
        n.wait(lambda: dark_background(app.search) and edit_background(app.search) == dark_fill,
               'dark palette returns after real high contrast is disabled', 10)
        n.wait(lambda: fill(app.status) == dark_status,
               'status returns to its original dark background', 10)
        assert n.window_text(app.search) == text
        assert_dark_preference(app, profile, expected)
        record('verified', {'high_contrast': list(high_contrast()),
                            'edit': edit_background(app.search),
                            'status': fill(app.status),
                            'preferences': list(profile.preferences())})
        screenshot(app.frame, settings[2] / 'high-contrast-restored-dark.png')

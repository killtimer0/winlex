"""Native fallback on legacy Windows and real high-contrast transitions."""
from contextlib import contextmanager
import hashlib
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

from support import bounded


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


def restored_high_contrast(state, snapshot):
    # Windows materializes an empty scheme name on the first transition and
    # cannot represent that initialized state as an empty string again.
    return state[0] == snapshot[0] and (
        state[1] == snapshot[1] or snapshot[1] == '' and state[1] is not None)


def set_high_contrast(snapshot):
    flags, scheme = snapshot
    buffer = c.create_unicode_buffer(scheme) if scheme is not None else None
    state = HighContrast(c.sizeof(HighContrast), flags,
                         c.addressof(buffer) if buffer is not None else None)
    # Broadcast the real setting change, but never SPIF_UPDATEINIFILE. Do not
    # use HCF_OPTION_NOTHEMECHANGE: Windows forbids it for an ON/OFF transition.
    spi_high_contrast(SPI_SETHIGHCONTRAST, state, 0x0002)


def execution_context():
    """Identify the desktop whose native settings this process can observe."""
    kernel = c.WinDLL('kernel32', use_last_error=True)
    user = c.WinDLL('user32', use_last_error=True)
    kernel.ProcessIdToSessionId.restype = w.BOOL
    kernel.ProcessIdToSessionId.argtypes = (w.DWORD, c.POINTER(w.DWORD))
    kernel.GetCurrentThreadId.restype, kernel.GetCurrentThreadId.argtypes = w.DWORD, ()
    user.GetProcessWindowStation.restype, user.GetProcessWindowStation.argtypes = w.HANDLE, ()
    user.GetThreadDesktop.restype, user.GetThreadDesktop.argtypes = w.HANDLE, (w.DWORD,)
    user.GetUserObjectInformationW.restype = w.BOOL
    user.GetUserObjectInformationW.argtypes = (
        w.HANDLE, c.c_int, c.c_void_p, w.DWORD, c.POINTER(w.DWORD))
    session = w.DWORD()
    if not kernel.ProcessIdToSessionId(os.getpid(), c.byref(session)):
        raise c.WinError(c.get_last_error())

    def name(handle):
        if not handle:
            raise c.WinError(c.get_last_error())
        needed = w.DWORD()
        if not user.GetUserObjectInformationW(handle, 2, None, 0, c.byref(needed)):
            if c.get_last_error() != 122:  # ERROR_INSUFFICIENT_BUFFER
                raise c.WinError(c.get_last_error())
        if not needed.value:
            raise RuntimeError('Native desktop context name has no buffer size')
        buffer = c.create_unicode_buffer((needed.value + c.sizeof(c.c_wchar) - 1) //
                                         c.sizeof(c.c_wchar))
        if not user.GetUserObjectInformationW(handle, 2, buffer, c.sizeof(buffer),
                                             c.byref(needed)):
            raise c.WinError(c.get_last_error())
        return buffer.value

    return {'session_id': session.value,
            'window_station': name(user.GetProcessWindowStation()),
            'desktop': name(user.GetThreadDesktop(kernel.GetCurrentThreadId()))}


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
    def remove():
        initialized = False
        scheduler = None
        try:
            import support
            import pythoncom
            pythoncom.CoInitialize()
            initialized = True
            scheduler = task_scheduler()
            if (name + '.job').casefold() in {entry.casefold() for entry in scheduler.Enum()}:
                from support.task_scheduler import delete_work_item
                delete_work_item(name)
        finally:
            if initialized:
                scheduler = None
                pythoncom.CoUninitialize()

    bounded.call(remove, 10, 'High-contrast watchdog task cleanup')


def watchdog_path(artifact, task_name, suffix):
    return artifact.with_suffix('.' + task_name.rsplit('.', 1)[1] + suffix)


def watchdog_record(path, text):
    temporary = path.with_suffix(path.suffix + '.new')
    temporary.write_text(text, encoding='utf-8')
    temporary.replace(path)


def watchdog(parent_pid, artifact, task_name, receipt):
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
        receipt['expected'] = saved
        receipt['context'] = execution_context()
        if receipt['context'] != saved['context']:
            raise RuntimeError('Watchdog execution context does not match caller')
        watchdog_record(ready, json.dumps({'pid': os.getpid(),
                                         'context': receipt['context']}))
        completed = watchdog_path(artifact, task_name, '.restored')
        deadline = time.monotonic() + 120
        while True:
            result = kernel.WaitForSingleObject(handle, 100)
            emergency = result == 0 or time.monotonic() >= deadline
            if emergency or completed.exists():
                actual = high_contrast()
                receipt['actual'] = {'flags': actual[0], 'scheme': actual[1]}
                receipt['emergency'] = emergency
                receipt['recovery_needed'] = not restored_high_contrast(actual, snapshot)
                if emergency or receipt['recovery_needed']:
                    # Keep native recovery bounded in this standalone supervisor.
                    recovery = {}

                    def recover():
                        recovery['recovery_context'] = execution_context()
                        if recovery['recovery_context'] != saved['context']:
                            raise RuntimeError('Native recovery context does not match caller')
                        set_high_contrast(snapshot)
                        restored = high_contrast()
                        recovery['restored'] = {'flags': restored[0], 'scheme': restored[1]}
                        if not restored_high_contrast(restored, snapshot):
                            raise RuntimeError('Watchdog could not restore entire HIGHCONTRAST snapshot')
                        return recovery

                    task = bounded.start(recover)
                    try:
                        task.wait(10, 'Watchdog native recovery')
                    except BaseException:
                        receipt.update(recovery)
                        raise
                    receipt.update(recovery)
                else:
                    receipt['restored'] = receipt['actual']
                if not emergency and receipt['recovery_needed']:
                    raise RuntimeError('Parent restoration did not stick; watchdog recovery was needed')
                return
            if result != 0x102:
                raise RuntimeError('Watchdog parent wait failed: %#x' % result)
    finally:
        if handle:
            kernel.CloseHandle(handle)
        if ready.exists():
            ready.unlink()


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
        receipt = {'ok': False}
        try:
            watchdog(int(sys.argv[2]), artifact, task_name, receipt)
        except BaseException:
            receipt['error'] = traceback.format_exc()
        else:
            receipt['ok'] = True
        # Native restoration evidence must survive a blocked scheduler call.
        watchdog_record(watchdog_path(artifact, task_name, '.recovery.json'), json.dumps(receipt))
        try:
            delete_watchdog_task(task_name)
        except BaseException:
            receipt['ok'] = False
            receipt['task_cleanup_complete'] = False
            receipt['task_cleanup_error'] = traceback.format_exc()
        else:
            receipt['task_cleanup_complete'] = True
        watchdog_record(result, json.dumps(receipt))
        raise SystemExit(0 if receipt['ok'] else 1)
    else:
        raise SystemExit('Use --probe-high-contrast or --watch-high-contrast PID SNAPSHOT TASK')
    raise SystemExit(0)


import pytest
import win32con
import win32gui
import winreg

from support import deadline, native as n
from support.diagnostics import screenshot
from support.input_state import InputState
from test_theme import THEME_SUPPORTED, modern_theme, selected_index
from test_theme_surfaces import colors, foreground
from test_theme_inputs import combo_info


pytestmark = pytest.mark.interactive


HC_COLOR_ROLES = {
    'Window': win32con.COLOR_WINDOW, 'WindowText': win32con.COLOR_WINDOWTEXT,
    'ButtonFace': win32con.COLOR_BTNFACE, 'ButtonText': win32con.COLOR_BTNTEXT,
    'GrayText': win32con.COLOR_GRAYTEXT, 'Hilight': win32con.COLOR_HIGHLIGHT,
    'HilightText': win32con.COLOR_HIGHLIGHTTEXT,
}


@deadline.operation('read explicit high contrast appearance fixture', 5)
def read_high_contrast_appearance(path):
    path = path.resolve(strict=True)
    with path.open('rb') as stream:
        raw = stream.read(256 * 1024 + 1)
    assert len(raw) <= 256 * 1024, 'HC theme file exceeds 256 KiB'
    profile = n.api(n.kernel, 'GetPrivateProfileStringW', w.DWORD,
                    w.LPCWSTR, w.LPCWSTR, w.LPCWSTR, w.LPWSTR, w.DWORD, w.LPCWSTR)

    def field(section, key):
        buffer = c.create_unicode_buffer(32768)
        count = profile(section, key, '', buffer, len(buffer), str(path))
        assert 0 < count < len(buffer) - 1, ('Missing or truncated HC fixture field', section, key)
        return buffer.value

    colors = {}
    for name in HC_COLOR_ROLES:
        parts = field(r'Control Panel\Colors', name).split()
        assert len(parts) == 3 and all(p.isascii() and p.isdigit() for p in parts), ('Invalid HC RGB field', name, parts)
        rgb = tuple(map(int, parts))
        assert all(0 <= value <= 255 for value in rgb), ('Invalid HC RGB range', name, rgb)
        colors[name] = list(reversed(rgb))
    style = [os.path.normcase(os.path.normpath(os.path.expandvars(field('VisualStyles', 'Path')))),
             field('VisualStyles', 'ColorStyle'), field('VisualStyles', 'Size')]
    with path.open('rb') as stream:
        assert stream.read(len(raw) + 1) == raw, 'HC fixture changed during preflight'
    return {'path': str(path), 'sha256': hashlib.sha256(raw).hexdigest(),
            'style': style, 'colors_bgr': colors}


def high_contrast_appearance_state():
    n.dispatch_pending_messages()
    buffers = [c.create_unicode_buffer(32768) for _ in range(3)]
    get_theme = n.api(c.WinDLL('uxtheme'), 'GetCurrentThemeName', c.c_long,
                      w.LPWSTR, c.c_int, w.LPWSTR, c.c_int, w.LPWSTR, c.c_int)
    status = get_theme(buffers[0], len(buffers[0]), buffers[1], len(buffers[1]),
                       buffers[2], len(buffers[2]))
    assert status >= 0, ('Cannot query applied HC visual style', status)
    return {'high_contrast': list(high_contrast()),
            'style': [os.path.normcase(os.path.normpath(buffers[0].value)),
                      buffers[1].value, buffers[2].value],
            'colors_bgr': {name: list(system_color(index)) for name, index in HC_COLOR_ROLES.items()},
            'covers': visible_transition_covers()}


@contextmanager
def temporary_high_contrast(snapshot, artifact, appearance=None):
    n.wait(lambda: not visible_transition_covers(),
           'previous native high contrast transition cover gone', 15)
    original_palette = native_system_palette()
    assert restored_high_contrast(high_contrast(), snapshot), (
        'High-contrast state changed before the guarded transition')
    # Preserve the whole structure (including the owned copy of the scheme),
    # not just the ON bit. Keep recovery evidence if the outer runner is killed.
    context = execution_context()
    artifact.write_text(json.dumps({'flags': snapshot[0], 'scheme': snapshot[1],
                                    'context': context}),
                        encoding='utf-8')
    # Match remote.py's Task Scheduler v1 launch: scheduler provenance keeps
    # this supervisor independent without weakening the runner's job limits.
    import pythoncom
    import win32api
    from win32com.taskscheduler import taskscheduler as t
    name = 'Winlex.Tests.HighContrast.' + uuid.uuid4().hex
    ready = watchdog_path(artifact, name, '.ready')
    result = watchdog_path(artifact, name, '.result.json')
    completed = watchdog_path(artifact, name, '.restored')
    created = False
    launch_requested = False
    try:
        with deadline.operation('register independent high contrast recovery', 15):
            from support.task_scheduler import new_work_item
            task = new_work_item(name)
            created = True
            # A separate console could cover the dialog under observation.
            interpreter = Path(sys.executable).with_name('pythonw.exe')
            if not interpreter.is_file():
                raise RuntimeError('High-contrast watchdog requires pythonw.exe')
            task.SetApplicationName(str(interpreter))
            task.SetParameters(subprocess.list2cmdline(
                [str(Path(__file__).resolve()), '--watch-high-contrast',
                 str(os.getpid()), str(artifact), name]))
            # Executable, script and artifact paths are absolute. Do not use
            # pywin32 SetWorkingDirectory: it also frees Python memory unlocked.
            task.SetFlags(t.TASK_FLAG_RUN_ONLY_IF_LOGGED_ON)
            task.SetAccountInformation(win32api.GetUserName(), None)
            task.SetMaxRunTime(-1)
            task.QueryInterface(pythoncom.IID_IPersistFile).Save(None, True)
            launch_requested = True
            task.Run()
        n.wait(lambda: ready.exists() or result.exists(), 'scheduler watchdog ready', 15)
        assert ready.exists() and not result.exists(), 'Watchdog startup failed; see %s' % result
        acknowledgement = json.loads(ready.read_text(encoding='utf-8'))
        assert acknowledgement['pid'] > 0 and acknowledgement['context'] == context, (
            'Watchdog did not acknowledge the caller execution context')
    except BaseException as error:
        if created and not launch_requested:
            delete_watchdog_task(name)
        if launch_requested:
            # A launched supervisor must survive until parent exit, even when
            # its readiness acknowledgement failed.
            pytest.exit('High-contrast watchdog startup failed: %r; see %s' %
                        (error, artifact), returncode=1)
        raise
    cleanup = artifact.with_name(artifact.stem + '-hc-cleanup.json')
    evidence = {'cleanup_complete': False,
                'errors': ['Cooperative native palette/cover restoration is unverified'],
                'original_palette': original_palette, 'high_contrast_palettes': []}
    try:
        watchdog_record(cleanup, json.dumps(evidence))
        with deadline.operation('enable native system high contrast', 10):
            set_high_contrast((snapshot[0] | HCF_HIGHCONTRASTON, snapshot[1]))
        n.wait(lambda: high_contrast()[0] & HCF_HIGHCONTRASTON,
               'real system high contrast is enabled', 10)
        evidence['high_contrast_palettes'].append(native_system_palette())
        if appearance is not None:
            appearance_path = artifact.with_name(artifact.stem + '-appearance.json')
            last = []

            def applied():
                actual = high_contrast_appearance_state()
                if last != [actual]:
                    watchdog_record(appearance_path, json.dumps({'expected': appearance, 'actual': actual}))
                    last[:] = [actual]
                return (restored_high_contrast(tuple(actual['high_contrast']),
                                               (snapshot[0] | HCF_HIGHCONTRASTON, snapshot[1]))
                        and actual['style'] == appearance['style']
                        and actual['colors_bgr'] == appearance['colors_bgr']
                        and not actual['covers'])

            # A real OS-application phase, not repeated UIA/capture. Per-capture
            # obstruction limits remain independent and unchanged.
            n.wait(applied, 'system reaches explicit HC fixture appearance (not a product rendering verdict)', 45)
        yield
    finally:
        original_error = sys.exc_info()
        if original_error[0] is not None:
            evidence['original_error'] = ''.join(traceback.format_exception(*original_error))
        restore_samples = []
        try:
            if high_contrast()[0] & HCF_HIGHCONTRASTON:
                evidence['high_contrast_palettes'].append(native_system_palette())
            watchdog_record(cleanup, json.dumps(evidence))
            with deadline.operation('restore native system high contrast', 10):
                until = time.monotonic() + 10
                set_high_contrast(snapshot)
                remaining = until - time.monotonic()
                assert remaining > 0, 'Native high-contrast restoration exhausted its budget'
                def restored():
                    sample = {'entered_elapsed': time.monotonic() - (until - 10)}
                    restore_samples.append(sample)
                    if len(restore_samples) > 64:
                        del restore_samples[0]
                    n.dispatch_pending_messages()
                    sample['pump_completed_elapsed'] = time.monotonic() - (until - 10)
                    state = high_contrast()
                    palette = native_system_palette()
                    sample['palette_read_elapsed'] = time.monotonic() - (until - 10)
                    covers = visible_transition_covers()
                    sample['cover_read_elapsed'] = time.monotonic() - (until - 10)
                    observation = {
                        'high_contrast': list(state), 'palette': palette, 'covers': covers,
                        'structure_matches': restored_high_contrast(state, snapshot),
                        'palette_matches': palette == original_palette,
                        'covers_absent': not covers,
                    }
                    # Record every changed observation, including all components
                    # when the first one fails. Preserve the last sample on timeout.
                    if evidence.get('restore_observation') != observation:
                        evidence['restore_observation'] = observation
                        evidence.setdefault('restore_observations', []).append({
                            'elapsed_seconds': time.monotonic() - (until - 10),
                            **observation,
                        })
                        watchdog_record(cleanup, json.dumps(evidence))
                    sample.update(structure_matches=observation['structure_matches'],
                                  palette_matches=observation['palette_matches'], covers=covers,
                                  completed_elapsed=time.monotonic() - (until - 10))
                    return (observation['structure_matches']
                            and observation['palette_matches']
                            and observation['covers_absent'])

                n.wait(restored,
                       'original HIGHCONTRAST, native palette and transition cover restored',
                       remaining)
            evidence['restore_samples'] = restore_samples
            watchdog_record(completed, str(os.getpid()))
            n.wait(result.exists, 'scheduler watchdog cleanup complete', 15)
            outcome = json.loads(result.read_text(encoding='utf-8'))
            assert outcome['ok'], outcome.get('error', 'High-contrast watchdog failed')
            assert outcome['context'] == context and outcome['expected'] == {
                'flags': snapshot[0], 'scheme': snapshot[1], 'context': context}
            assert not outcome['emergency'] and not outcome['recovery_needed'], outcome
            assert restored_high_contrast(
                (outcome['restored']['flags'], outcome['restored']['scheme']), snapshot), outcome
            artifact.unlink()
            evidence.update(cleanup_complete=True, errors=[])
            watchdog_record(cleanup, json.dumps(evidence))
        except BaseException as error:
            evidence.update(cleanup_complete=False, errors=[traceback.format_exc()],
                            restore_samples=restore_samples)
            try:
                watchdog_record(cleanup, json.dumps(evidence))
            finally:
                # Never start another case on structure-only emergency recovery.
                # Keep the task/artifacts for independent recovery after exit.
                pytest.exit('Cooperative high-contrast restoration failed: %r; see %s' %
                            (error, cleanup), returncode=1)


def visible_transition_covers():
    # Known private Windows transition class; not a universal occlusion test.
    return [hwnd for hwnd in n.top_level_windows()
            if n.is_visible(hwnd) and n.class_name(hwnd) == 'CoverWindowClass']


def native_system_palette():
    return [system_color(index) for index in
            (win32con.COLOR_WINDOW, win32con.COLOR_BTNFACE, win32con.COLOR_HIGHLIGHT)]


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


def assert_dark_preference(app, profile, expected):
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
    finally:
        app.close_settings(sheet, accept=False)
    assert profile.preferences() == expected, 'native fallback must not rewrite preferences'


@pytest.mark.skipif(THEME_SUPPORTED, reason='requires Windows before dark-mode support')
def test_legacy_dark_preference_keeps_native_edit(launch, profile, settings):
    profile.set('Theme', winreg.REG_SZ, 'dark')
    expected = profile.preferences()
    app = launch()
    native = edit_background(app.search)
    with InputState() as inputs, foreground(app, inputs), n.window_dpi_context(app.frame):
        text = 'native fallback text'
        set_edit_text(app.search, text)
        assert_dark_preference(app, profile, expected)
        assert edit_background(app.search) == native, 'opening settings preserves native appearance'
        assert n.window_text(app.search) == text
        screenshot(app.frame, settings[2] / 'legacy-dark-preference-native.png')


@modern_theme
def test_actual_high_contrast_overrides_dark_and_restores(launch, profile, settings, hc_appearance):
    from test_theme_surfaces import observed_colors

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
        evidence = {'before': {'high_contrast': list(snapshot)}}
        evidence_path = settings[2] / 'high-contrast-controls.json'

        def record(stage, values):
            evidence[stage] = values
            evidence_path.write_text(json.dumps(evidence, indent=2), encoding='utf-8')

        set_edit_text(app.search, text)
        regions = {}

        def remember_region(hwnd, region=None):
            _, _, width, height = n.rectangle(hwnd, client=True)
            regions[hwnd] = region or (2, 2, width - 2, height - 2)

        def sample_points(hwnd, region):
            left, top, right, bottom = region
            assert left < right and top < bottom, ('empty native sample', hwnd, region)
            return [win32gui.ClientToScreen(hwnd, point) for point in
                    ((left, top), (right - 1, top), (left, bottom - 1),
                     (right - 1, bottom - 1),
                     ((left + right) // 2, (top + bottom) // 2))]

        def require_ownership(hwnd, points):
            for point in points:
                actual = win32gui.WindowFromPoint(point)
                # WindowFromPoint ignores disabled windows. A hidden modal
                # sheet leaves its owner disabled, so descend from the exact
                # hit root without skipping disabled children. Still reject
                # other top-level windows and overlapping siblings.
                if actual == app.frame and not win32gui.IsWindowEnabled(app.frame):
                    current = actual
                    while current and current != hwnd:
                        child = win32gui.ChildWindowFromPointEx(
                            current, win32gui.ScreenToClient(current, point),
                            0x0001 | 0x0004)  # CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT
                        if not child or child == current:
                            break
                        current = child
                    actual = current
                if actual != hwnd and not (actual and win32gui.IsChild(hwnd, actual)):
                    raise ObscuredSample(str((
                        'native control sample is obscured', hwnd, n.class_name(hwnd),
                        point, actual, n.class_name(actual) if actual else None)))

        def fill(hwnd):
            if hwnd not in regions:
                remember_region(hwnd)
            region = regions[hwnd]
            points = sample_points(hwnd, region)
            obstruction = []

            def sample():
                try:
                    require_ownership(hwnd, points)
                    value = observed_colors(hwnd, region).most_common(1)[0][0]
                    require_ownership(hwnd, points)
                    return value
                except ObscuredSample as error:
                    obstruction[:] = [str(error)]
                    return None

            try:
                return n.wait(sample, 'unobscured native control capture', 15)
            except AssertionError as error:
                message = '%s; obstruction=%r' % (error, obstruction)
                record('failure', {'hwnd': hwnd, 'region': region, 'error': message})
                raise AssertionError(message) from error

        def remember_edit_region():
            _, _, width, height = n.rectangle(app.search, client=True)
            remember_region(app.search, (width - 10, height - 8, width - 5, height - 4))

        remember_edit_region()
        original_edit = fill(app.search)

        def wait_fill(hwnd, expected_fills, description):
            observed = []

            def sample():
                value = fill(hwnd)
                observed[:] = [value]
                expected = expected_fills() if callable(expected_fills) else expected_fills
                return value if value in expected else None

            try:
                return n.wait(sample, description, 15)
            except AssertionError as error:
                expected = expected_fills() if callable(expected_fills) else expected_fills
                raise AssertionError('%s: observed=%r, expected=%r' %
                                     (description, observed, expected) +
                                     '; cause=' + str(error)) from error

        dark_status = fill(app.status)
        sheet = app.settings()

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

            def remember_control_regions():
                current_info = combo_info(combo)
                item = current_info.item
                remember_region(combo, (item.right - 12, item.top + 3,
                                        item.right - 4, item.bottom - 3))
                overflow_left = win32gui.ScreenToClient(tab, n.rectangle(overflow)[:2])[0]
                # Exclude the property page, header text and overflow child.
                remember_region(tab, (4, 4, overflow_left - 4, 8))
                remember_region(overflow)
                return current_info

            info = remember_control_regions()

            def idle_pointer():
                left, top, _, _ = n.rectangle(sheet)
                assert n.api(n.user, 'SetCursorPos', w.BOOL, c.c_int, c.c_int)(left + 40, top + 10)

            idle_pointer()

            initial = []

            def settled_dark_controls():
                current = {name: fill(hwnd) for name, hwnd in
                           [('combo', combo), ('tab', tab), ('overflow', overflow)]}
                if not initial or current != initial[0]:
                    initial[:] = [current, time.monotonic()]
                # Do not make a restoration oracle from an animation frame.
                return current if time.monotonic() - initial[1] >= .1 else None

            before_controls = n.wait(settled_dark_controls, 'initial dark control animations settle', 15)
            record('dark_controls', before_controls)
            with temporary_high_contrast(snapshot, settings[2] / 'high-contrast-restore.json', hc_appearance):
                assert high_contrast()[0] & HCF_HIGHCONTRASTON
                idle_pointer()
                info = remember_control_regions()
                observed = {}

                def observe(name, sample):
                    try:
                        observed[name] = {'color': sample()}
                    except AssertionError as error:
                        observed[name] = {'error': str(error)}
                        raise
                    finally:
                        record('high_contrast_observations', dict(
                            observed, high_contrast=list(high_contrast()),
                            system_window=system_color(win32con.COLOR_WINDOW),
                            system_face=system_color(win32con.COLOR_BTNFACE)))

                for name, hwnd in [('combo', combo), ('tab', tab), ('overflow', overflow)]:
                    # HC uses system roles, not the application's dark palette.
                    # Check each surface independently: one recolored control
                    # must not hide stale dark rendering in another control.
                    role = win32con.COLOR_WINDOW if name == 'combo' else win32con.COLOR_BTNFACE
                    observe(name, lambda hwnd=hwnd, role=role: wait_fill(
                        hwnd, lambda: {system_color(role)}, name + ' uses its high-contrast system role'))
                # Recheck every exposed control before delivering an owned gesture.
                for hwnd in (combo, tab, overflow):
                    require_ownership(hwnd, sample_points(hwnd, regions[hwnd]))
                assert high_contrast()[0] & HCF_HIGHCONTRASTON

                def require_sheet_input():
                    active = win32gui.GetForegroundWindow()
                    assert active in (sheet, info.list) or (
                        active and (win32gui.IsChild(sheet, active) or
                                    win32gui.IsChild(info.list, active))), (
                                        'settings no longer owns foreground input', active)

                n.wait(lambda: win32gui.GetForegroundWindow() == sheet,
                       'settings foreground returns after native high contrast transition', 15)
                require_sheet_input()
                button = info.button
                button_points = sample_points(combo, (button.left, button.top,
                                                     button.right, button.bottom))
                require_ownership(combo, button_points)
                point = n.physical_screen_point(combo, button_points[-1])
                with n.physical_dpi_context():
                    require_sheet_input()
                    require_ownership(combo, [point])
                    inputs.click(*point)
                n.wait(lambda: n.is_visible(info.list), 'high-contrast combo popup visible', 15)
                remember_region(info.list)
                observe('combo_list', lambda: wait_fill(
                    info.list, lambda: {system_color(win32con.COLOR_WINDOW)},
                    'combo list uses the high-contrast window background'))
                # On failed observation, unwind immediately instead of injecting
                # cleanup input into a potentially covered or inactive desktop.
                require_sheet_input()
                require_ownership(info.list, sample_points(info.list, regions[info.list]))
                assert n.is_visible(info.list), 'combo popup remains open before Escape'
                inputs.key(win32con.VK_ESCAPE)
                n.wait(lambda: not n.is_visible(info.list), 'Escape closes high-contrast combo popup', 15)
                assert n.send(combo, win32con.CB_GETCURSEL) == selection
                assert n.send(tab, 0x130B) == selected
                assert profile.preferences() == expected
                # Hide the owned sheet only while sampling the main window so
                # displayed pixels cannot come from an overlapping dialog.
                win32gui.ShowWindow(sheet, win32con.SW_HIDE)
                try:
                    remember_edit_region()
                    remember_region(app.status)
                    observe('edit', lambda: wait_fill(
                        app.search, lambda: {system_color(win32con.COLOR_WINDOW)},
                        'edit uses the high-contrast window background'))
                    observe('status', lambda: wait_fill(
                        app.status, lambda: {system_color(win32con.COLOR_BTNFACE)},
                        'status uses the high-contrast face background'))
                    assert n.window_text(app.search) == text
                    screenshot(app.frame, settings[2] / 'actual-high-contrast.png')
                finally:
                    win32gui.ShowWindow(sheet, win32con.SW_SHOWNOACTIVATE)
                record('high_contrast_controls', dict(observed,
                        system_window=system_color(win32con.COLOR_WINDOW),
                        system_face=system_color(win32con.COLOR_BTNFACE)))
                screenshot(sheet, settings[2] / 'high-contrast-native-controls.png')
            restored = {}
            idle_pointer()
            remember_control_regions()
            for name, hwnd in [('combo', combo), ('tab', tab), ('overflow', overflow)]:
                restored[name] = wait_fill(hwnd, {before_controls[name]},
                                           name + ' returns to its original dark background')
            assert n.send(combo, win32con.CB_GETCURSEL) == selection
            assert n.send(tab, 0x130B) == selected
            assert profile.preferences() == expected
            record('restored_controls', dict(restored, high_contrast=list(high_contrast())))
            screenshot(sheet, settings[2] / 'high-contrast-controls-restored-dark.png')
        except AssertionError as error:
            if 'failure' not in evidence:
                record('failure', {'error': str(error)})
            raise
        else:
            # Failure teardown belongs to the launch fixture's native Session
            # cleanup, not another UIA operation in the broken dialog state.
            app.close_settings(sheet, accept=False)
        assert restored_high_contrast(high_contrast(), snapshot)
        remember_edit_region()
        remember_region(app.status)
        wait_fill(app.search, {original_edit},
                  'dark palette returns after real high contrast is disabled')
        wait_fill(app.status, {dark_status}, 'status returns to its original dark background')
        assert n.window_text(app.search) == text
        assert_dark_preference(app, profile, expected)
        record('verified', {'high_contrast': list(high_contrast()),
                            'edit': fill(app.search),
                            'status': fill(app.status),
                            'preferences': list(profile.preferences())})
        screenshot(app.frame, settings[2] / 'high-contrast-restored-dark.png')

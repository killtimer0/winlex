"""Guarded real high-contrast transitions with independent native recovery."""
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
import pytest
import win32con
from support import deadline, native as n
from driver.appearance import THEME_SUPPORTED, selected_index
from driver.surfaces import colors
from driver.high_contrast_watchdog import (
    HCF_HIGHCONTRASTON, delete_watchdog_task, execution_context, high_contrast,
    restored_high_contrast, set_high_contrast, watchdog_path, watchdog_record,
)


HC_COLOR_ROLES = {
    'Window': win32con.COLOR_WINDOW, 'WindowText': win32con.COLOR_WINDOWTEXT,
    'ButtonFace': win32con.COLOR_BTNFACE, 'ButtonText': win32con.COLOR_BTNTEXT,
    'GrayText': win32con.COLOR_GRAYTEXT, 'Hilight': win32con.COLOR_HIGHLIGHT,
    'HilightText': win32con.COLOR_HIGHLIGHTTEXT,
}


@deadline.operation('read explicit high contrast appearance fixture')
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


def register_recovery(artifact, task_name):
    """Declare custody before any recovery task or native setting can change."""
    obligation = {'kind': 'high_contrast', 'snapshot': artifact.name,
                  'receipt': watchdog_path(artifact, task_name, '.result.json').name}
    with (artifact.parent / 'recoveries.jsonl').open('a', encoding='utf-8', newline='\n') as stream:
        stream.write(json.dumps(obligation) + '\n')
        stream.flush()
        os.fsync(stream.fileno())


@contextmanager
def temporary_high_contrast(snapshot, artifact, appearance):
    if appearance is None:
        raise ValueError('Native high-contrast transitions require an explicit appearance fixture')
    n.wait(lambda: not visible_transition_covers(),
           'previous native high contrast transition cover gone', 15)
    original_palette = native_system_palette()
    assert restored_high_contrast(high_contrast(), snapshot), (
        'High-contrast state changed before the guarded transition')
    # Preserve the whole structure (including the owned copy of the scheme),
    # not just the ON bit. Keep recovery evidence if the outer runner is killed.
    context = execution_context()
    name = 'Winlex.Tests.HighContrast.' + uuid.uuid4().hex
    register_recovery(artifact, name)
    saved = {'flags': snapshot[0], 'scheme': snapshot[1], 'context': context,
             'original_palette': [list(color) for color in original_palette],
             'application_verified': False}
    artifact.write_text(json.dumps(saved),
                        encoding='utf-8')
    # Match remote.py's Task Scheduler v1 launch: scheduler provenance keeps
    # this supervisor independent without weakening the runner's job limits.
    import pythoncom
    import win32api
    from win32com.taskscheduler import taskscheduler as t
    ready = watchdog_path(artifact, name, '.ready')
    result = watchdog_path(artifact, name, '.result.json')
    completed = watchdog_path(artifact, name, '.restored')
    created = False
    launch_requested = False
    try:
        with deadline.operation('register independent high contrast recovery'):
            from support.task_scheduler import new_work_item
            task = new_work_item(name)
            created = True
            # A separate console could cover the dialog under observation.
            interpreter = Path(sys.executable).with_name('pythonw.exe')
            if not interpreter.is_file():
                raise RuntimeError('High-contrast watchdog requires pythonw.exe')
            task.SetApplicationName(str(interpreter))
            task.SetParameters(subprocess.list2cmdline(
                [str(Path(__file__).resolve().with_name('high_contrast_watchdog.py')), '--watch-high-contrast',
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
            # No native setting changed and the unlaunched task is gone. This
            # discharges registration, not a claim that restoration was run.
            watchdog_record(result, json.dumps({
                'ok': True, 'task_cleanup_complete': True, 'not_started': True,
                'expected': {'flags': snapshot[0], 'scheme': snapshot[1], 'context': context},
            }))
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
        with deadline.operation('enable native system high contrast'):
            set_high_contrast((snapshot[0] | HCF_HIGHCONTRASTON, snapshot[1]))
        n.wait(lambda: high_contrast()[0] & HCF_HIGHCONTRASTON,
               'real system high contrast is enabled', 10)
        evidence['high_contrast_palettes'].append(native_system_palette())
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
        saved['application_verified'] = True
        watchdog_record(artifact, json.dumps(saved))
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
            with deadline.operation('restore native system high contrast'):
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
            assert saved['application_verified'], (
                'HC application was not verified; a baseline-looking sample cannot certify restoration')
            watchdog_record(completed, str(os.getpid()))
            n.wait(result.exists, 'scheduler watchdog cleanup complete', 15)
            outcome = json.loads(result.read_text(encoding='utf-8'))
            assert outcome['ok'], outcome.get('error', 'High-contrast watchdog failed')
            assert outcome['context'] == context and outcome['expected'] == saved
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

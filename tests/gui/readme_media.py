"""Explicit README recording job, excluded from ordinary pytest discovery.

FFmpeg captures the desktop; the GUI runner provides execution and app isolation.
"""
from contextlib import contextmanager
import ctypes as ct
from ctypes import wintypes as w
import json
import math
import re
import subprocess
import sys
import threading
import time
import uuid

import pytest
import win32api
import win32con as c
import win32gui as g
import win32job

from driver import auto, until
from execute import process_identity


class Recording:
    """One actual capture command, with kill-on-close containment and retained logs."""
    def __init__(self, bounds, destination, still=False):
        self.bounds, self.destination, self.still = bounds, destination, still
        self.process = self.job = None
        self.threads = []
        self.ready = threading.Event()
        self.origin = None
        self.has_frame = False

    def __enter__(self):
        left, top, right, bottom = self.bounds
        command = ['ffmpeg', '-hide_banner', '-y', '-copyts', '-stats_period', '0.05',
                   '-progress', 'pipe:1', '-f', 'gdigrab', '-framerate', '60',
                   '-draw_mouse', '0' if self.still else '1',
                   '-offset_x', str(left), '-offset_y', str(top),
                   '-video_size', '{}x{}'.format(right-left, bottom-top),
                   '-use_wallclock_as_timestamps', '1', '-i', 'desktop',
                   '-vf', 'showinfo=checksum=0,setpts=PTS-STARTPTS', '-threads', '2']
        command += (['-frames:v', '1', '-update', '1', '-c:v', 'png'] if self.still
                    else ['-c:v', 'ffv1', '-level', '3', '-fps_mode', 'passthrough'])
        command.append(str(self.destination))
        self.job = win32job.CreateJobObject(None, 'Winlex.Media.'+uuid.uuid4().hex)
        limits = win32job.QueryInformationJobObject(self.job, win32job.JobObjectExtendedLimitInformation)
        limits['BasicLimitInformation']['LimitFlags'] = win32job.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        win32job.SetInformationJobObject(self.job, win32job.JobObjectExtendedLimitInformation, limits)
        self.wall_offset = time.time()-time.monotonic()
        try:
            try:
                self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                                stderr=subprocess.PIPE, creationflags=subprocess.CREATE_NO_WINDOW)
            except FileNotFoundError as error:
                raise RuntimeError('FFmpeg was not found on the recording desktop PATH') from error
            win32job.AssignProcessToJobObject(self.job, self.process._handle)
            self.created = process_identity(self.process.pid)
            self.destination.with_suffix('.process.json').write_text(json.dumps(dict(
                pid=self.process.pid, created=self.created, exited=False)), encoding='utf-8')
            for pipe, suffix, reader in [(self.process.stderr, '.ffmpeg.log', self.stderr),
                                         (self.process.stdout, '.progress.log', self.progress)]:
                thread = threading.Thread(target=self.drain, args=(pipe, suffix, reader), daemon=True)
                self.threads.append(thread)
                thread.start()
            deadline = time.monotonic()+20
            while not self.ready.wait(.02):
                if self.process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('FFmpeg did not produce a frame; see {}'.format(
                        self.destination.with_suffix('.ffmpeg.log')))
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def drain(self, pipe, suffix, reader):
        with self.destination.with_suffix(suffix).open('w', encoding='utf-8') as log:
            for raw in iter(pipe.readline, b''):
                line = raw.decode('utf-8', errors='replace')
                log.write(line)
                log.flush()
                reader(line)
                if self.origin is not None and self.has_frame:
                    self.ready.set()

    def stderr(self, line):
        # copyts + wallclock input retain the capture timestamp here, before
        # setpts normalizes the saved video. Parse integer PTS, not rounded pts_time.
        match = re.search(r'config in time_base:\s*(\d+)/(\d+)', line)
        if match:
            self.time_base = int(match[1])/int(match[2])
        match = re.search(r'\bn:\s*0\s+pts:\s*(-?\d+)', line)
        if match and self.origin is None:
            self.origin = int(match[1])*self.time_base-self.wall_offset

    def progress(self, line):
        if line.startswith('frame=') and int(line.split('=', 1)[1]) > 0:
            self.has_frame = True

    def __exit__(self, kind, error, traceback):
        try:
            if self.process is not None:
                if self.process.poll() is None:
                    try:
                        self.process.stdin.write(b'q\n')
                        self.process.stdin.flush()
                    except OSError:
                        pass
                try:
                    code = self.process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    self.process.kill()
                    self.process.wait(timeout=5)
                    raise RuntimeError('FFmpeg did not stop cleanly: {}'.format(self.destination))
                finally:
                    for thread in self.threads:
                        thread.join(timeout=3)
                    for pipe in (self.process.stdin, self.process.stdout, self.process.stderr):
                        pipe.close()
                    self.destination.with_suffix('.process.json').write_text(json.dumps(dict(
                        pid=self.process.pid, created=getattr(self, 'created', None),
                        exit_code=self.process.returncode, exited=self.process.poll() is not None)), encoding='utf-8')
                if code and kind is None:
                    raise RuntimeError('FFmpeg failed; see {}'.format(self.destination.with_suffix('.ffmpeg.log')))
        finally:
            if self.job is not None:
                self.job.Close()


def window_bounds(hwnd):
    rect = w.RECT()
    get = ct.WinDLL('dwmapi').DwmGetWindowAttribute
    get.argtypes = [w.HWND, w.DWORD, ct.c_void_p, w.DWORD]
    get.restype = ct.c_long
    assert get(hwnd, 9, ct.byref(rect), ct.sizeof(rect)) == 0
    return rect.left, rect.top, rect.right, rect.bottom


@contextmanager
def backdrop(bounds, frame):
    brush = g.CreateSolidBrush(win32api.RGB(220, 227, 235))
    wc = g.WNDCLASS()
    wc.lpszClassName = 'Winlex.ReadmeBackdrop'
    wc.hInstance = win32api.GetModuleHandle(None)
    wc.hbrBackground, wc.lpfnWndProc = brush, g.DefWindowProc
    atom = g.RegisterClass(wc)
    hwnd = None
    try:
        left, top, right, bottom = bounds
        hwnd = g.CreateWindowEx(c.WS_EX_TOOLWINDOW | c.WS_EX_NOACTIVATE, atom, '',
                                c.WS_POPUP, left, top, right-left, bottom-top, 0, 0, wc.hInstance, None)
        g.SetWindowPos(hwnd, c.HWND_TOPMOST, left, top, right-left, bottom-top,
                       c.SWP_SHOWWINDOW | c.SWP_NOACTIVATE)
        g.SetWindowPos(frame, c.HWND_TOPMOST, 0, 0, 0, 0,
                       c.SWP_NOMOVE | c.SWP_NOSIZE | c.SWP_NOACTIVATE)
        # This thread owns the backdrop, so paint it before blocking in capture.
        g.UpdateWindow(hwnd)
        yield
    finally:
        if hwnd:
            g.DestroyWindow(hwnd)
        g.UnregisterClass(atom, wc.hInstance)


def move_cursor(point, scale):
    start = win32api.GetCursorPos()
    dx, dy = point[0]-start[0], point[1]-start[1]
    distance = math.hypot(dx, dy)
    if distance < 1:
        return
    bend = min(distance*.14, 70*scale)
    normal = (-dy/distance*bend, dx/distance*bend)
    control = ((start[0]+point[0])/2+normal[0], (start[1]+point[1])/2+normal[1])
    duration = min(.7, .25+distance/(1800*scale))
    started = time.monotonic()
    while True:
        elapsed = min(1, (time.monotonic()-started)/duration)
        t = elapsed*elapsed*(3-2*elapsed)
        win32api.SetCursorPos(tuple(round((1-t)**2*a+2*(1-t)*t*b+t*t*z)
                                   for a, b, z in zip(start, control, point)))
        if elapsed == 1:
            break
        time.sleep(1/120)


def theme(app, name):
    sheet = app.settings()
    assert g.IsWindowEnabled(sheet.control(1301)), 'README capture requires application dark mode'
    sheet.select(1301, name)
    sheet.ok()


@pytest.mark.real_input
def record_media(app, ui_text):
    cursor, foreground = win32api.GetCursorPos(), g.GetForegroundWindow()
    try:
        sheet = app.settings()
        sheet.select(1300, ui_text(2084))
        sheet.select(1301, ui_text(2088))
        sheet.ok()
        app.restart()
        assert auto.AU3_WinActivateByHandle(app.frame), 'Cannot activate recording window'
        until('recording window foreground', lambda: g.GetForegroundWindow() == app.frame)
        left, top, right, bottom = win32api.GetMonitorInfo(win32api.MonitorFromWindow(app.frame))['Work']
        get_dpi = ct.windll.user32.GetDpiForWindow
        get_dpi.argtypes, get_dpi.restype = [w.HWND], w.UINT
        dpi = get_dpi(app.frame)
        scale, margin = dpi/96, round(24*dpi/96)
        width, height = min(round(1160*scale), right-left-margin*2), min(round(660*scale), bottom-top-margin*2)
        assert width >= 1000*scale and height >= 600*scale, 'Recording desktop is too small'
        g.SetWindowPos(app.frame, c.HWND_TOPMOST, left+margin, top+margin, width, height, 0)
        app.open_symbol('IStream')
        app.command(1014)
        time.sleep(.4)
        frame = window_bounds(app.frame)
        bounds = (frame[0]-margin, frame[1]-margin, frame[2]+margin, frame[3]+margin)
        with backdrop(bounds, app.frame):
            win32api.SetCursorPos((right-4, bottom-4))
            time.sleep(.3)
            with Recording(bounds, app.output / 'light.png', still=True):
                pass
            theme(app, '深色')
            time.sleep(.4)
            with Recording(bounds, app.output / 'dark.png', still=True):
                pass
            theme(app, '浅色')
            app.editor().close()
            app.set_text(app.child(1102), '')
            time.sleep(.3)
            events = []
            with Recording(bounds, app.output / 'workflow.mkv') as recording:
                def event(kind, point):
                    events.append(dict(kind=kind, time=time.monotonic()-recording.origin,
                                       x=point[0]-bounds[0], y=point[1]-bounds[1]))

                def click(point, count=1):
                    assert recording.process.poll() is None, 'Recorder exited during interaction'
                    assert g.GetForegroundWindow() == app.frame, 'Recording lost foreground'
                    assert window_bounds(app.frame) == frame, 'Recording window moved'
                    move_cursor(point, scale)
                    time.sleep(.1)
                    for index in range(count):
                        win32api.mouse_event(c.MOUSEEVENTF_LEFTDOWN, 0, 0)
                        event('down', point)
                        time.sleep(.04)
                        win32api.mouse_event(c.MOUSEEVENTF_LEFTUP, 0, 0)
                        event('up', point)
                        if index+1 < count:
                            time.sleep(.09)

                time.sleep(.5)
                search = app.child(1102)
                click(g.ClientToScreen(search, (30, 10)))
                auto.AU3_Send('^a', 0)
                for char in 'OVERLAPPED':
                    auto.AU3_Send(char, 1)
                    time.sleep(.10)
                auto.AU3_Send('{ENTER}', 0)
                listing, index = app.wait_result('OVERLAPPED')
                rect = app.list_item_rect(listing, index)
                click(g.ClientToScreen(listing, (30, (rect[1]+rect[3])//2)), count=2)
                document = app.wait_declaration('OVERLAPPED')
                time.sleep(.8)
                combo = app.child(1047, document.window())
                rect = g.GetClientRect(combo)
                click(g.ClientToScreen(combo, (rect[2]-12, rect[3]//2)))
                # Select the actual dropdown row with the mouse, including its click effect.
                auto.AU3_Send('{HOME}', 0)
                index = g.SendMessage(combo, c.CB_FINDSTRINGEXACT, -1, 'x64')
                assert index >= 0, 'Missing x64 choice'
                dropdown = until('architecture dropdown', lambda: g.FindWindow('ComboLBox', None))
                row_height = g.SendMessage(combo, c.CB_GETITEMHEIGHT, index, 0)
                click(g.ClientToScreen(dropdown, (30, row_height*index+row_height//2)))
                until('x64 declaration', lambda: document.architecture() == 'x64')
                time.sleep(1.2)
            (app.output / 'recording.json').write_text(json.dumps(dict(
                bounds=bounds, dpi=dpi, target_fps=60, events=events,
                symbol='OVERLAPPED', architecture=document.architecture()), indent=2), encoding='utf-8')
    finally:
        # Do not leave a held button behind if recording is cancelled during a click.
        win32api.mouse_event(c.MOUSEEVENTF_LEFTUP, 0, 0)
        win32api.SetCursorPos(cursor)
        if foreground and g.IsWindow(foreground):
            try:
                g.SetForegroundWindow(foreground)
            except g.error:
                pass

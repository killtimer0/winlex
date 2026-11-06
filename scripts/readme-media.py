"""Capture README images on the current desktop; briefly takes over the keyboard and mouse."""
import argparse
import contextlib
from datetime import datetime, timezone
import math
from pathlib import Path
import sys
import threading
import time
import tkinter as tk
import traceback
import win32api
import win32con
import win32gui
import win32process
import winreg

import uiautomation as auto
from PIL import Image, ImageChops, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/gui"))
from support import native as n
from support.config import ConfigProfile
from support.session import Session, select_combo


def capture(rect, cursor=False):
    with auto.Bitmap.FromControl(auto.GetRootControl(), *rect, captureCursor=cursor) as bitmap:
        return bitmap.ToPILImage().convert("RGB")


def compose(light, dark):
    if light.size != dark.size:
        raise ValueError("Theme captures must have identical dimensions")
    width, height = light.size
    offset = height / math.tan(math.radians(80)) / 2
    edge = ((round(width * .55 + offset), 0), (round(width * .55 - offset), height))
    mask = Image.new("L", light.size)
    ImageDraw.Draw(mask).polygon([edge[0], (width, 0), (width, height), edge[1]], fill=255)
    result = Image.composite(dark, light, mask)
    result.thumbnail((1200, 900), Image.Resampling.LANCZOS)
    return result


def focus_window(hwnd):
    attach = n.api(n.user, "AttachThreadInput", n.w.BOOL, n.w.DWORD, n.w.DWORD, n.w.BOOL)
    own = win32api.GetCurrentThreadId()
    target = win32process.GetWindowThreadProcessId(win32gui.GetForegroundWindow())[0]
    attached = target != own and attach(own, target, True)
    try:
        with contextlib.suppress(win32gui.error):
            win32gui.SetForegroundWindow(hwnd)
    finally:
        if attached:
            attach(own, target, False)


def activate(app, exe):
    if win32gui.GetForegroundWindow() == app.frame:
        return
    taskbar = auto.PaneControl(ClassName="Shell_TrayWnd")
    found = []

    def walk(control, depth=0):
        if found or depth > 6:
            return
        if control.ControlTypeName == "ButtonControl" and exe.name.lower() in control.AutomationId.lower():
            found.append(control)
        for child in control.GetChildren():
            walk(child, depth + 1)

    walk(taskbar)
    if not found:
        raise RuntimeError("Application taskbar button not found")
    found[0].Click()
    n.wait(lambda: win32gui.GetForegroundWindow() == app.frame, "application foreground", 10)


def query(app, text):
    auto.ControlFromHandle(app.search).Click()
    if win32gui.GetForegroundWindow() != app.frame:
        raise RuntimeError("Application lost the foreground; capture aborted")
    auto.SendKeys("{Ctrl}a")
    auto.SendKeys(text, interval=.08)
    n.wait(lambda: app.named_rows(text), f"{text} results", 60)


def open_symbol(app, text):
    query(app, text)
    row = app.named_rows(text)[0]
    auto.ControlFromHandle(app.results).ListItemControl(searchDepth=1, foundIndex=row + 1).DoubleClick()
    doc = app.wait_document(text, text)
    n.wait(lambda: text in app.content(doc), f"{text} declaration", 60)
    return doc


def theme(app, mode):
    sheet = app.settings()
    page = app.settings_page(sheet, 0)
    combo = auto.ControlFromHandle(n.get_item(page, 1301))
    select_combo(combo, app.choices(combo)[mode])
    app.close_settings(sheet)


def record(rect, stop, frames, timestamps, errors):
    try:
        with auto.UIAutomationInitializerInThread():
            while not stop.is_set():
                started = time.monotonic()
                image = capture(rect, cursor=True)
                image.thumbnail((1000, 760), Image.Resampling.LANCZOS)
                if frames and not ImageChops.difference(image, frames[-1]).getbbox():
                    timestamps[-1] = started
                else:
                    frames.append(image)
                    timestamps.append(started)
                stop.wait(max(0, .1 - (time.monotonic() - started)))
    except Exception:
        errors.append(traceback.format_exc())


def generate(args, app, scale, evidence):
    auto.ControlFromHandle(app.frame).MoveWindow(
        round(100 * scale), round(70 * scale), round(1160 * scale), round(660 * scale))
    activate(app, args.exe)
    backdrop = tk.Tk()
    backdrop.overrideredirect(True)
    backdrop.configure(background="#dce3eb")
    backdrop.attributes("-topmost", True)
    backdrop.geometry(f"{backdrop.winfo_screenwidth()}x{backdrop.winfo_screenheight()}+0+0")
    backdrop.update()
    try:
        win32gui.SetWindowPos(app.frame, win32con.HWND_TOPMOST, 0, 0, 0, 0,
                              win32con.SWP_NOMOVE | win32con.SWP_NOSIZE | win32con.SWP_NOACTIVATE)
        point = win32gui.ClientToScreen(app.frame, (300, 300))
        if win32process.GetWindowThreadProcessId(win32gui.WindowFromPoint(point))[1] != app.process.pid:
            raise RuntimeError("Application window is not above the backdrop")

        doc = open_symbol(app, "IStream")
        app.command(1014)
        bounds = auto.DwmGetWindowExtendFrameBounds(app.frame)
        margin = round(24 * scale)
        rect = (bounds.left - margin, bounds.top - margin,
                bounds.width() + margin * 2, bounds.height() + margin * 2)
        auto.MoveTo(rect[0] - 40, rect[1] - 40)
        time.sleep(.5)
        light = capture(rect)
        theme(app, 2)
        auto.MoveTo(rect[0] - 40, rect[1] - 40)
        time.sleep(.5)
        dark = capture(rect)
        light.save(evidence / "light.png")
        dark.save(evidence / "dark.png")
        args.output.mkdir(parents=True, exist_ok=True)
        compose(light, dark).save(args.output / "themes.png", optimize=True)

        theme(app, 1)
        for window in app.documents():
            assert auto.ControlFromHandle(window).GetWindowPattern().Close(waitTime=0)
        auto.ControlFromHandle(app.search).Click()
        auto.SendKeys("{Ctrl}a{Delete}")
        n.wait(lambda: app.count() == 0, "empty results")
        frames, timestamps, errors = [], [], []
        stop = threading.Event()
        recorder = threading.Thread(target=record, args=(rect, stop, frames, timestamps, errors))
        recorder.start()
        try:
            time.sleep(.5)
            query(app, "OVERLAPPED")
            row = app.named_rows("OVERLAPPED")[0]
            auto.ControlFromHandle(app.results).ListItemControl(
                searchDepth=1, foundIndex=row + 1).DoubleClick()
            doc = app.wait_document("OVERLAPPED", "OVERLAPPED")
            n.wait(lambda: app.content(doc), "OVERLAPPED declaration", 60)
            time.sleep(.8)
            combo = auto.ControlFromHandle(app.architecture(doc))
            combo.ButtonControl(searchDepth=1).Click()
            item = combo.ListItemControl(Name="x64")
            n.wait(lambda: item.Exists(0), "x64 architecture option")
            time.sleep(.5)
            item.Click()
            n.wait(lambda: app.choice(doc) == "x64", "x64 declaration", 60)
            time.sleep(1.5)
        finally:
            stop.set()
            recorder.join()
        if errors:
            raise RuntimeError("\n".join(errors))
        palette = light.resize(frames[0].size).quantize(colors=256)
        indexed = [image.quantize(palette=palette, dither=Image.Dither.NONE) for image in frames]
        durations = [max(30, round((b - a) * 1000)) for a, b in zip(timestamps, timestamps[1:])] + [900]
        indexed[0].save(args.output / "workflow.gif", save_all=True, append_images=indexed[1:],
                        duration=durations, loop=0, optimize=True, disposal=1)
    except Exception:
        with contextlib.suppress(Exception):
            auto.ControlFromHandle(app.frame).CaptureToImage(
                str(evidence / "failure.png"), captureCursor=True)
        raise
    finally:
        backdrop.destroy()


def main():
    auto.SetProcessDpiAwareness(auto.ProcessDpiAwareness.PerMonitorDpiAware)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--winmd", type=Path, default=ROOT / "inputs/Windows.Win32.winmd")
    parser.add_argument("--output", type=Path, default=ROOT / "docs/images")
    args = parser.parse_args()
    args.exe, args.winmd = args.exe.resolve(strict=True), args.winmd.resolve(strict=True)
    if not auto.DwmIsCompositionEnabled():
        parser.error("DWM composition is required")
    evidence = ROOT / "tmp/readme-media" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    evidence.mkdir(parents=True)
    cursor = win32api.GetCursorPos()
    foreground = win32gui.GetForegroundWindow()
    print("Capture starts in 2 seconds; do not use the keyboard or mouse until it finishes.", flush=True)
    time.sleep(2)
    errors = []
    try:
        with ConfigProfile("zh-CN") as profile:
            profile.set("Theme", winreg.REG_SZ, "light")
            app = Session(args.exe, profile, [args.winmd], "pmv2")
            try:
                generate(args, app, (app.actual_dpi or 96) / 96, evidence)
            finally:
                app.close()
    except Exception:
        errors.append(traceback.format_exc())
    win32api.SetCursorPos(cursor)
    if foreground and win32gui.IsWindow(foreground):
        focus_window(foreground)
    (evidence / "capture.log").write_text("\n".join(errors), encoding="utf-8")
    if errors:
        raise RuntimeError(errors[0])
    print(f"Images: {args.output}\nCaptures: {evidence}")


if __name__ == "__main__":
    main()

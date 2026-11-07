// SPDX-License-Identifier: MPL-2.0
// Rust port of win32-darkmodelib, e8ec95b: StaticTextSubclass and static setup.
// Original work Copyright (c) 2025-2026 ozone10.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// https://github.com/ozone10/win32-darkmodelib
//! Let native static text honor the parent's disabled foreground, without embossing.
use crate::native::*;
use std::{cell::Cell, ptr, rc::Rc};
use windows_sys::Win32::System::SystemServices::{
    SS_CENTER, SS_LEFT, SS_LEFTNOWORDWRAP, SS_RIGHT, SS_SIMPLE, SS_TYPEMASK,
};
use windows_sys::Win32::UI::Shell::GetWindowSubclass;

const SUBCLASS: usize = 0x574c5354;

struct StaticText {
    enabled: Cell<bool>,
    alive: Cell<bool>,
    redraw_depth: Cell<usize>,
}

/// Keep tracking in light mode, but leave its native rendering and WM_ENABLE alone.
pub(super) fn install(hwnd: HWND) {
    unsafe {
        let kind = GetWindowLongW(hwnd, GWL_STYLE) as u32 & SS_TYPEMASK;
        if !matches!(
            kind,
            SS_LEFT | SS_CENTER | SS_RIGHT | SS_SIMPLE | SS_LEFTNOWORDWRAP
        ) || GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, ptr::null_mut()) != 0
        {
            return;
        }
        let data = Rc::into_raw(Rc::new(StaticText {
            enabled: Cell::new(IsWindowEnabled(hwnd) != 0),
            alive: Cell::new(true),
            redraw_depth: Cell::new(0),
        }));
        if set_subclass(hwnd, Some(subclass), SUBCLASS, data as usize) == 0 {
            drop(Rc::from_raw(data));
        }
    }
}

/// WM_CTLCOLORSTATIC must see the logical state while WS_DISABLED is cleared.
pub(super) fn enabled(hwnd: HWND) -> bool {
    unsafe {
        let mut data = 0;
        if super::is_dark() && GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut data) != 0 {
            return (*(data as *const StaticText)).enabled.get();
        }
        IsWindowEnabled(hwnd) != 0
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _: usize,
    data: usize,
) -> isize {
    unsafe {
        // Style changes and synchronous parent painting can destroy the control.
        let raw = data as *const StaticText;
        Rc::increment_strong_count(raw);
        let state = Rc::from_raw(raw);
        if msg == WM_NCDESTROY {
            state.alive.set(false);
            remove_subclass(hwnd, Some(subclass), SUBCLASS);
            drop(Rc::from_raw(raw));
            return default_subclass(hwnd, msg, w, l);
        }
        if msg == WM_ENABLE {
            state.enabled.set(w != 0);
            if super::is_dark() {
                state.redraw_depth.set(state.redraw_depth.get() + 1);
                let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                if style & WS_DISABLED != 0 {
                    SetWindowLongW(hwnd, GWL_STYLE, (style & !WS_DISABLED) as i32);
                }
                if state.alive.get() {
                    let parent = GetParent(hwnd);
                    if !parent.is_null() {
                        let mut rect: RECT = std::mem::zeroed();
                        GetClientRect(hwnd, &mut rect);
                        MapWindowPoints(hwnd, parent, (&mut rect as *mut RECT).cast(), 2);
                        RedrawWindow(
                            parent,
                            &rect,
                            ptr::null_mut(),
                            RDW_INVALIDATE | RDW_UPDATENOW,
                        );
                    }
                }
                state.redraw_depth.set(state.redraw_depth.get() - 1);
                if state.alive.get() && state.redraw_depth.get() == 0 {
                    // A nested WM_ENABLE owns the newest state. Preserve unrelated
                    // style edits made by the parent, and never touch a reused HWND.
                    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                    let restored = if state.enabled.get() {
                        style & !WS_DISABLED
                    } else {
                        style | WS_DISABLED
                    };
                    if restored != style {
                        SetWindowLongW(hwnd, GWL_STYLE, restored as i32);
                    }
                }
                return 0;
            }
        }
        default_subclass(hwnd, msg, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_transitions_paint_native_text_with_the_upstream_foreground() {
        if !super::super::supported() || high_contrast() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        unsafe extern "system" fn parent_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
            unsafe {
                if msg == WM_PAINT {
                    let mut paint: PAINTSTRUCT = std::mem::zeroed();
                    BeginPaint(hwnd, &mut paint);
                    let dc = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HDC;
                    if !dc.is_null() {
                        // Exercise the real static painter during the parent's
                        // synchronous repaint, not a simulated color callback.
                        send(
                            GetDlgItem(hwnd, 1),
                            WM_PRINTCLIENT,
                            dc as usize,
                            PRF_CLIENT as isize,
                        );
                    }
                    EndPaint(hwnd, &paint);
                    return 0;
                }
                DefWindowProcW(hwnd, msg, w, l)
            }
        }
        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(parent_proc),
                hInstance: instance(),
                lpszClassName: w!("Winlex.Test.StaticText"),
                ..std::mem::zeroed()
            };
            assert!(RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS);
            let parent = CreateWindowExW(
                0,
                class.lpszClassName,
                w!(""),
                WS_POPUP,
                -30000,
                -30000,
                180,
                30,
                ptr::null_mut(),
                ptr::null_mut(),
                instance(),
                ptr::null(),
            );
            assert!(!parent.is_null());
            let text = child(
                parent,
                w!("STATIC"),
                &wide("Readable static text"),
                SS_LEFT,
                1,
            )
            .unwrap();
            let area = crate::layout::rect(0, 0, 180, 30);
            set_bounds(text, area).unwrap();
            super::super::set_mode(2);
            super::super::apply(parent);
            install(text);
            let dc = CreateCompatibleDC(ptr::null_mut());
            let screen = GetDC(parent);
            let bitmap = GdiObject(CreateCompatibleBitmap(screen, 180, 30));
            ReleaseDC(parent, screen);
            assert!(!dc.is_null() && !bitmap.0.is_null());
            let original = SelectObject(dc, bitmap.0);
            SetWindowLongPtrW(parent, GWLP_USERDATA, dc as _);
            ShowWindow(parent, SW_SHOWNOACTIVATE);
            for active in [false, true, false, true] {
                FillRect(
                    dc,
                    &area,
                    super::super::brush_role(super::super::Role::Dialog),
                );
                EnableWindow(text, i32::from(active));
                assert_eq!(IsWindowEnabled(text) != 0, active);
                let expected = if active { 0x00e0e0e0 } else { 0x00808080 };
                let mut readable = false;
                for y in 0..30 {
                    for x in 0..180 {
                        readable |= GetPixel(dc, x, y) == expected;
                    }
                }
                assert!(
                    readable,
                    "Native label must use the upstream enabled/disabled palette"
                );
            }
            DestroyWindow(parent);
            SelectObject(dc, original);
            DeleteDC(dc);
            super::super::set_mode(0);
        }
    }
}

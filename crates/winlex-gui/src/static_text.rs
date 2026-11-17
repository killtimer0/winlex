// SPDX-License-Identifier: MPL-2.0
// Rust port of win32-darkmodelib, e8ec95b: StaticTextSubclass and static setup.
// Original work Copyright (c) 2025-2026 ozone10.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// https://github.com/ozone10/win32-darkmodelib
//! Native static text reflow and disabled foreground without embossing.
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
        let mut name = [0u16; 32];
        let length = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
        if !String::from_utf16_lossy(&name[..length as usize]).eq_ignore_ascii_case("static") {
            return;
        }
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
        if crate::theme::is_dark()
            && GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut data) != 0
        {
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
        if msg == WM_SIZE && GetWindowLongW(hwnd, GWL_STYLE) as u32 & SS_TYPEMASK == SS_LEFT {
            // Wrapped text can reflow within the preserved client pixels.
            InvalidateRect(hwnd, ptr::null(), 1);
        }
        if msg == WM_ENABLE {
            state.enabled.set(w != 0);
            if crate::theme::is_dark() {
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

// SPDX-License-Identifier: MPL-2.0
// Rust port of win32-darkmodelib, e8ec95b: DmlibSubclassControl.cpp
// (CustomBorderSubclass) and Darkmodelib.cpp setup.
// Original work Copyright (c) 2025-2026 ozone10.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// https://github.com/ozone10/win32-darkmodelib
//! Native input themes and non-client edit/list borders; never consumes business notifications.
use super::{Role, brush_role, color_role, is_dark};
use crate::native::*;
use std::{cell::Cell, ptr, rc::Rc};
use windows_sys::Win32::UI::{HiDpi::*, Shell::GetWindowSubclass};

const BORDER: usize = 0x574c4244;

struct Border {
    edit: bool,
    hot: Cell<bool>,
}

/// Install once, including in light mode so subsequent mode changes are reversible.
pub(super) fn install(hwnd: HWND, class: &str) {
    unsafe {
        if class.eq_ignore_ascii_case("Edit") || class.eq_ignore_ascii_case("ListBox") {
            let list = class.eq_ignore_ascii_case("ListBox");
            let scroll = GetWindowLongW(hwnd, GWL_STYLE) & (WS_HSCROLL | WS_VSCROLL) as i32 != 0;
            if super::supported() && !list && !scroll {
                // Scroll bars can disappear during reapplication. Retire a
                // previous fallback border before returning to the native edit.
                let mut data = 0;
                if GetWindowSubclass(hwnd, Some(border_proc), BORDER, &mut data) != 0
                    && remove_subclass(hwnd, Some(border_proc), BORDER) != 0
                {
                    drop(Rc::from_raw(data as *const Border));
                    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
                    SetWindowLongW(hwnd, GWL_EXSTYLE, ex | WS_EX_CLIENTEDGE as i32);
                    frame_changed(hwnd);
                }
                return;
            }
            if GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_CLIENTEDGE as i32 == 0
                || (list && GetWindowLongW(hwnd, GWL_STYLE) & LBS_COMBOBOX != 0)
                || GetWindowSubclass(hwnd, Some(border_proc), BORDER, ptr::null_mut()) != 0
            {
                return;
            }
            let data = Rc::into_raw(Rc::new(Border {
                edit: !list,
                hot: Cell::new(false),
            }));
            if set_subclass(hwnd, Some(border_proc), BORDER, data as usize) == 0 {
                drop(Rc::from_raw(data));
            }
        }
    }
}

/// Apply native body/child themes.
pub(super) fn apply(hwnd: HWND, class: &str) {
    unsafe {
        if class.eq_ignore_ascii_case("ComboBox") {
            let mut info: COMBOBOXINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
            let has_info = GetComboBoxInfo(hwnd, &mut info) != 0;
            let edit_selection = if has_info
                && GetWindowLongW(hwnd, GWL_STYLE) & CBS_DROPDOWNLIST != CBS_DROPDOWNLIST
                && !info.hwndItem.is_null()
                && info.hwndItem != hwnd
            {
                let mut start = 0u32;
                let mut end = 0u32;
                SendMessageW(
                    info.hwndItem,
                    EM_GETSEL,
                    (&mut start as *mut u32) as usize,
                    (&mut end as *mut u32) as isize,
                );
                Some((start, end))
            } else {
                None
            };
            let body_theme = if super::native_dark_theme() {
                "DarkMode_DarkTheme"
            } else {
                "DarkMode_CFD"
            };
            super::set_native_theme(hwnd, body_theme, None);
            if has_info {
                if !info.hwndList.is_null() {
                    if GetWindowLongW(hwnd, GWL_STYLE) & CBS_DROPDOWNLIST == CBS_SIMPLE {
                        let style = GetWindowLongW(info.hwndList, GWL_STYLE);
                        let ex = GetWindowLongW(info.hwndList, GWL_EXSTYLE);
                        if style & WS_BORDER as i32 != 0 || ex & WS_EX_CLIENTEDGE as i32 != 0 {
                            let (new_style, new_ex) = if is_dark() {
                                (style | WS_BORDER as i32, ex & !(WS_EX_CLIENTEDGE as i32))
                            } else {
                                (style & !(WS_BORDER as i32), ex | WS_EX_CLIENTEDGE as i32)
                            };
                            if (style, ex) != (new_style, new_ex) {
                                SetWindowLongW(info.hwndList, GWL_STYLE, new_style);
                                SetWindowLongW(info.hwndList, GWL_EXSTYLE, new_ex);
                                frame_changed(info.hwndList);
                            }
                        }
                    }
                    super::set_native_theme(info.hwndList, "DarkMode_Explorer", Some("Explorer"));
                }
                if !info.hwndItem.is_null() && info.hwndItem != hwnd {
                    super::set_native_theme(info.hwndItem, body_theme, None);
                }
            }
            if let Some((start, end)) = edit_selection {
                let mut current_start = 0u32;
                let mut current_end = 0u32;
                SendMessageW(
                    info.hwndItem,
                    EM_GETSEL,
                    (&mut current_start as *mut u32) as usize,
                    (&mut current_end as *mut u32) as isize,
                );
                if (current_start, current_end) != (start, end) {
                    // Native combo theme changes can reset the edit selection.
                    // EM_SETSEL preserves it without generating change notifications;
                    // unlike CB_GETEDITSEL, this also retains offsets beyond 64K.
                    SendMessageW(info.hwndItem, EM_SETSEL, start as usize, end as isize);
                }
            }
        } else if class.eq_ignore_ascii_case("Edit") || class.eq_ignore_ascii_case("ListBox") {
            let scroll = GetWindowLongW(hwnd, GWL_STYLE) & (WS_HSCROLL | WS_VSCROLL) as i32 != 0;
            if scroll || class.eq_ignore_ascii_case("ListBox") {
                super::set_native_theme(hwnd, "DarkMode_Explorer", Some("Explorer"));
            } else {
                super::set_native_theme(
                    hwnd,
                    if super::native_dark_theme() {
                        "DarkMode_DarkTheme"
                    } else {
                        "DarkMode_CFD"
                    },
                    None,
                );
            }
            if GetWindowSubclass(hwnd, Some(border_proc), BORDER, ptr::null_mut()) != 0 {
                let old = GetWindowLongW(hwnd, GWL_EXSTYLE);
                let new = if is_dark() {
                    old & !(WS_EX_CLIENTEDGE as i32)
                } else {
                    old | WS_EX_CLIENTEDGE as i32
                };
                if old != new {
                    SetWindowLongW(hwnd, GWL_EXSTYLE, new);
                    frame_changed(hwnd);
                }
            }
        }
    }
}

unsafe fn frame_changed(hwnd: HWND) {
    unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        RedrawWindow(
            hwnd,
            ptr::null(),
            ptr::null_mut(),
            RDW_INVALIDATE | RDW_FRAME,
        );
    }
}

fn edge(disabled: bool, hot: bool) -> Role {
    if disabled {
        Role::DisabledEdge
    } else if hot {
        Role::HotEdge
    } else {
        Role::Edge
    }
}

fn roundness() -> i32 {
    if super::windows11() { 4 } else { 0 }
}

unsafe fn outline(dc: HDC, rect: RECT, role: Role, rounded: bool) {
    unsafe {
        let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(role)));
        let saved = SaveDC(dc);
        SelectObject(dc, pen.0);
        SelectObject(dc, GetStockObject(NULL_BRUSH));
        let radius = if rounded { roundness() } else { 0 };
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
        RestoreDC(dc, saved);
    }
}

unsafe fn track(hwnd: HWND, cancel: bool) {
    unsafe {
        let mut event = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE | if cancel { TME_CANCEL } else { 0 },
            hwndTrack: hwnd,
            dwHoverTime: HOVER_DEFAULT,
        };
        TrackMouseEvent(&mut event);
    }
}

unsafe fn border_paint(hwnd: HWND, border: &Border) {
    unsafe {
        let dc = GetWindowDC(hwnd);
        if dc.is_null() {
            return;
        }
        let dpi = GetDpiForWindow(hwnd);
        let style = GetWindowLongW(hwnd, GWL_STYLE);
        let mut rect = client(hwnd);
        rect.right += 2 * GetSystemMetricsForDpi(SM_CXEDGE, dpi);
        rect.bottom += 2 * GetSystemMetricsForDpi(SM_CYEDGE, dpi);
        if style & WS_VSCROLL as i32 != 0 {
            rect.right += GetSystemMetricsForDpi(SM_CXVSCROLL, dpi);
        }
        if style & WS_HSCROLL as i32 != 0 {
            rect.bottom += GetSystemMetricsForDpi(SM_CYHSCROLL, dpi);
        }
        let disabled = IsWindowEnabled(hwnd) == 0;
        let mut inner = rect;
        InflateRect(&mut inner, -1, -1);
        outline(
            dc,
            inner,
            if disabled {
                Role::Dialog
            } else {
                Role::Background
            },
            false,
        );
        let mut cursor: POINT = std::mem::zeroed();
        GetCursorPos(&mut cursor);
        ScreenToClient(hwnd, &mut cursor);
        let focus = GetFocus() == hwnd;
        let readonly = border.edit && style & ES_READONLY != 0;
        let hot = !readonly && ((border.hot.get() && PtInRect(&rect, cursor) != 0) || focus);
        outline(dc, rect, edge(disabled, hot), true);
        if roundness() != 0
            && focus
            && border.edit
            && color_role(Role::Highlight) != color_role(Role::Control)
        {
            let line = RECT {
                top: rect.bottom - MulDiv(2, dpi as i32, 96),
                ..rect
            };
            let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(Role::Highlight)));
            let saved = SaveDC(dc);
            SelectObject(dc, pen.0);
            SelectObject(dc, brush_role(Role::Highlight));
            RoundRect(
                dc,
                line.left,
                line.top,
                line.right,
                line.bottom,
                roundness(),
                roundness(),
            );
            RestoreDC(dc, saved);
        }
        ReleaseDC(hwnd, dc);
    }
}

unsafe extern "system" fn border_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        let raw = data as *const Border;
        Rc::increment_strong_count(raw);
        let border = Rc::from_raw(raw);
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(border_proc), id);
            drop(Rc::from_raw(raw));
        } else if is_dark() {
            match msg {
                WM_NCPAINT => {
                    default_subclass(hwnd, msg, wp, lp);
                    border_paint(hwnd, &border);
                    return 0;
                }
                WM_NCCALCSIZE if lp != 0 => {
                    // RECT is also the first member of NCCALCSIZE_PARAMS.
                    let dpi = GetDpiForWindow(hwnd);
                    InflateRect(
                        lp as *mut RECT,
                        -GetSystemMetricsForDpi(SM_CXEDGE, dpi),
                        -GetSystemMetricsForDpi(SM_CYEDGE, dpi),
                    );
                }
                WM_DPICHANGED_AFTERPARENT => {
                    frame_changed(hwnd);
                }
                WM_MOUSEMOVE if GetFocus() != hwnd => {
                    track(hwnd, false);
                    if !border.hot.replace(true) {
                        RedrawWindow(
                            hwnd,
                            ptr::null(),
                            ptr::null_mut(),
                            RDW_INVALIDATE | RDW_FRAME,
                        );
                    }
                }
                WM_MOUSELEAVE => {
                    track(hwnd, true);
                    if border.hot.replace(false) {
                        RedrawWindow(
                            hwnd,
                            ptr::null(),
                            ptr::null_mut(),
                            RDW_INVALIDATE | RDW_FRAME,
                        );
                    }
                }
                WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
                    let result = default_subclass(hwnd, msg, wp, lp);
                    RedrawWindow(
                        hwnd,
                        ptr::null(),
                        ptr::null_mut(),
                        RDW_INVALIDATE | RDW_FRAME,
                    );
                    return result;
                }
                _ => {}
            }
        }
        default_subclass(hwnd, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Buffer;
    use super::*;

    #[test]
    fn preferred_native_combo_preserves_selection_during_reapplication_and_printing() {
        if !super::super::supported() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        super::super::set_mode(2);
        unsafe {
            for style in [CBS_DROPDOWNLIST, CBS_DROPDOWN] {
                let hwnd = test_window(
                    w!("COMBOBOX"),
                    ptr::null(),
                    WS_POPUP | style as u32,
                    crate::layout::rect(0, 0, 160, 100),
                );
                install(hwnd, "ComboBox");
                apply(hwnd, "ComboBox");
                assert_eq!(
                    SendMessageW(hwnd, CB_GETCURSEL, 0, 0),
                    CB_ERR as isize,
                    "theming an initially empty combo must not create a selection"
                );
                SendMessageW(hwnd, CB_ADDSTRING, 0, w!("First") as isize);
                SendMessageW(hwnd, CB_ADDSTRING, 0, w!("Second") as isize);
                SendMessageW(hwnd, CB_SETCURSEL, 1, 0);
                if style == CBS_DROPDOWN {
                    SendMessageW(hwnd, CB_SETEDITSEL, 0, (1 | (3 << 16)) as isize);
                }
                let selected = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
                let edit_selection = SendMessageW(hwnd, CB_GETEDITSEL, 0, 0);
                for mode in [0, 2, 2] {
                    super::super::set_mode(mode);
                    // Include child enumeration, not just the combo setup:
                    // re-theming its Edit must preserve the same selection.
                    super::super::apply(hwnd);
                    assert_eq!(SendMessageW(hwnd, CB_GETCURSEL, 0, 0), selected);
                    assert_eq!(SendMessageW(hwnd, CB_GETEDITSEL, 0, 0), edit_selection);
                }
                EnableWindow(hwnd, 0);
                let area = client(hwnd);
                let dc = GetDC(hwnd);
                let target = Buffer::new(dc, area.right, area.bottom).unwrap();
                let marker = GdiObject(CreateSolidBrush(0x00123456));
                FillRect(target.dc, &area, marker.0);
                SendMessageW(
                    hwnd,
                    WM_PRINTCLIENT,
                    target.dc as usize,
                    PRF_CLIENT as isize,
                );
                let mut painted = 0;
                for y in 0..area.bottom {
                    for x in 0..area.right {
                        painted += usize::from(GetPixel(target.dc, x, y) != 0x00123456);
                    }
                }
                assert!(painted > 0, "the snapshot must replace sentinel pixels");
                assert_eq!(
                    SendMessageW(hwnd, CB_GETCURSEL, 0, 0),
                    selected,
                    "printing must preserve the native selected item"
                );
                assert_eq!(
                    SendMessageW(hwnd, CB_GETEDITSEL, 0, 0),
                    edit_selection,
                    "printing must preserve the native edit selection"
                );
                ReleaseDC(hwnd, dc);
                DestroyWindow(hwnd);
            }
        }
        super::super::set_mode(0);
    }

    #[test]
    fn preferred_native_edit_retains_client_edge_and_restores_it_after_scroll_removal() {
        if !super::super::supported() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        unsafe {
            for (class, name, scroll) in [
                (w!("EDIT"), "Edit", 0),
                (w!("EDIT"), "Edit", WS_VSCROLL),
                (w!("EDIT"), "Edit", WS_HSCROLL),
                (w!("EDIT"), "Edit", WS_HSCROLL | WS_VSCROLL),
                (w!("LISTBOX"), "ListBox", 0),
            ] {
                let hwnd = CreateWindowExW(
                    WS_EX_CLIENTEDGE,
                    class,
                    ptr::null(),
                    WS_POPUP | scroll,
                    0,
                    0,
                    160,
                    100,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                );
                assert!(!hwnd.is_null());
                let native = name == "Edit" && scroll == 0;
                for mode in [1, 2, 1, 2] {
                    super::super::set_mode(mode);
                    install(hwnd, name);
                    apply(hwnd, name);
                    assert_eq!(
                        GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_CLIENTEDGE as i32 != 0,
                        native || !is_dark(),
                        "only scroll/list fallback borders replace the native client edge in dark mode"
                    );
                    if native {
                        assert_eq!(
                            GetWindowSubclass(hwnd, Some(border_proc), BORDER, ptr::null_mut()),
                            0,
                            "PREFER_THEME must not install a custom border on a plain edit"
                        );
                    }
                }
                if name == "Edit" && scroll != 0 {
                    let style = GetWindowLongW(hwnd, GWL_STYLE);
                    SetWindowLongW(hwnd, GWL_STYLE, style & !((WS_HSCROLL | WS_VSCROLL) as i32));
                    install(hwnd, name);
                    apply(hwnd, name);
                    assert_ne!(
                        GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_CLIENTEDGE as i32,
                        0
                    );
                    assert_eq!(
                        GetWindowSubclass(hwnd, Some(border_proc), BORDER, ptr::null_mut()),
                        0,
                        "removing scroll bars must return the edit to native theming"
                    );
                }
                DestroyWindow(hwnd);
            }
        }
        super::super::set_mode(0);
    }
}

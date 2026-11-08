// SPDX-License-Identifier: MPL-2.0
// Rust port of win32-darkmodelib, e8ec95b: DmlibSubclassControl.cpp
// (CustomBorderSubclass and ComboBoxSubclass) and Darkmodelib.cpp setup.
// Original work Copyright (c) 2025-2026 ozone10.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// https://github.com/ozone10/win32-darkmodelib
//! Combo bodies and non-client edit/list borders; never consumes business notifications.
use super::{
    Role,
    animation::{animation, stop_animation},
    brush_role, color_role, is_dark,
};
use crate::native::*;
use std::{
    cell::{Cell, RefCell},
    ptr,
    rc::Rc,
};
use windows_sys::Win32::UI::{HiDpi::*, Shell::GetWindowSubclass};

const COMBO: usize = 0x574c4342;
const BORDER: usize = 0x574c4244;

struct Combo {
    style: i32,
    theme: Cell<HTHEME>,
    state: Cell<i32>,
    buffered: bool,
    buffer: RefCell<Option<ComboBuffer>>,
    active_buffers: RefCell<Vec<HDC>>,
    alive: Cell<bool>,
}

struct ActiveComboBuffer<'a>(&'a RefCell<Vec<HDC>>);

impl Drop for ActiveComboBuffer<'_> {
    fn drop(&mut self) {
        self.0.borrow_mut().pop();
    }
}

// DmlibSubclass.h::BufferData: retain the compatible surface until its size changes.
struct ComboBuffer {
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    width: i32,
    height: i32,
}

impl ComboBuffer {
    unsafe fn new(target: HDC, width: i32, height: i32) -> Option<Self> {
        unsafe {
            let dc = CreateCompatibleDC(target);
            if dc.is_null() {
                return None;
            }
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if bitmap.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old_bitmap = SelectObject(dc, bitmap);
            if old_bitmap.is_null() || old_bitmap as isize == -1 {
                DeleteObject(bitmap);
                DeleteDC(dc);
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                old_bitmap,
                width,
                height,
            })
        }
    }
}

impl Drop for ComboBuffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

impl Combo {
    fn animates(&self) -> bool {
        if !self.buffered || self.style == CBS_SIMPLE {
            return false;
        }
        let mut enabled = 0i32;
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            ) != 0
                && enabled != 0
        }
    }

    fn owns_buffer_dc(&self, dc: HDC) -> bool {
        !dc.is_null()
            && (self.active_buffers.borrow().contains(&dc)
                || self.buffer.borrow().as_ref().is_some_and(|b| b.dc == dc))
    }

    fn theme(&self, hwnd: HWND) -> HTHEME {
        if self.theme.get() == 0 {
            self.theme
                .set(unsafe { OpenThemeData(hwnd, w!("COMBOBOX")) });
        }
        self.theme.get()
    }

    fn close(&self) {
        let theme = self.theme.replace(0);
        if theme != 0 {
            unsafe { CloseThemeData(theme) };
        }
    }
}

impl Drop for Combo {
    fn drop(&mut self) {
        self.close();
        if self.buffered {
            animation(|api| unsafe { (api.uninit)() });
        }
    }
}

struct Border {
    edit: bool,
    hot: Cell<bool>,
}

/// Install once, including in light mode so subsequent mode changes are reversible.
pub(super) fn install(hwnd: HWND, class: &str) {
    unsafe {
        if class.eq_ignore_ascii_case("ComboBox") {
            // PREFER_THEME leaves the combo's body and input behavior native,
            // independently of whether the current palette is dark or light.
            if super::prefer_theme() {
                return;
            }
            if GetWindowSubclass(hwnd, Some(combo_proc), COMBO, ptr::null_mut()) != 0 {
                return;
            }
            let style = GetWindowLongW(hwnd, GWL_STYLE);
            let kind = style as i32 & CBS_DROPDOWNLIST;
            if kind == 0 {
                return;
            }
            // ComboBoxEx owns its child's painting and notifications.
            let mut parent_class = [0u16; 64];
            let len = GetClassNameW(GetParent(hwnd), parent_class.as_mut_ptr(), 64);
            if String::from_utf16_lossy(&parent_class[..len.max(0) as usize])
                .eq_ignore_ascii_case("ComboBoxEx32")
            {
                return;
            }
            if kind != CBS_DROPDOWNLIST {
                SetWindowLongW(hwnd, GWL_STYLE, style | WS_CLIPCHILDREN as i32);
            }
            let data = Rc::into_raw(Rc::new(Combo {
                style: kind,
                theme: Cell::new(0),
                state: Cell::new(CBXSR_NORMAL),
                buffered: animation(|api| (api.init)() >= 0),
                buffer: RefCell::new(None),
                active_buffers: RefCell::new(Vec::new()),
                alive: Cell::new(true),
            }));
            if set_subclass(hwnd, Some(combo_proc), COMBO, data as usize) == 0 {
                drop(Rc::from_raw(data));
            }
        } else if class.eq_ignore_ascii_case("Edit") || class.eq_ignore_ascii_case("ListBox") {
            let list = class.eq_ignore_ascii_case("ListBox");
            let scroll = GetWindowLongW(hwnd, GWL_STYLE) & (WS_HSCROLL | WS_VSCROLL) as i32 != 0;
            if super::prefer_theme() && !list && !scroll {
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

/// Apply native body/child themes. WM_THEMECHANGED only closes cached handles.
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

fn background(disabled: bool, hot: bool) -> Role {
    if disabled {
        Role::Dialog
    } else if hot {
        Role::Hot
    } else {
        Role::Control
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
    // Same Win11-only corner policy as upstream, independently of dark-theme class support.
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut version: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    version.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    if unsafe { windows_sys::Wdk::System::SystemServices::RtlGetVersion(&mut version) } >= 0
        && version.dwBuildNumber >= 22000
    {
        4
    } else {
        0
    }
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

unsafe fn render(hwnd: HWND, dc: HDC, combo: &Combo, state: i32) {
    unsafe {
        let saved = SaveDC(dc);
        let theme = combo.theme(hwnd);
        let mut info: COMBOBOXINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<COMBOBOXINFO>() as u32;
        if GetComboBoxInfo(hwnd, &mut info) == 0 {
            RestoreDC(dc, saved);
            return;
        }
        let mut rect = client(hwnd);
        let disabled = state == CBXSR_DISABLED;
        let hot = state == CBXSR_HOT;
        let list = combo.style == CBS_DROPDOWNLIST;
        let simple = combo.style == CBS_SIMPLE;
        let focus = GetFocus() == if list { hwnd } else { info.hwndItem };
        let border = edge(disabled, hot || focus || simple);
        let text_color = color_role(if disabled {
            Role::DisabledText
        } else {
            Role::Text
        });
        let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
        if !font.is_null() {
            SelectObject(dc, font);
        }
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, text_color);
        let mut arrow = info.rcButton;
        arrow.left -= 1;
        if list {
            FillRect(dc, &rect, brush_role(background(disabled, hot)));
            let index = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
            if index != CB_ERR as isize {
                if GetWindowLongW(hwnd, GWL_STYLE) & (CBS_OWNERDRAWFIXED | CBS_OWNERDRAWVARIABLE)
                    != 0
                {
                    let id = GetDlgCtrlID(hwnd);
                    let draw = DRAWITEMSTRUCT {
                        CtlType: ODT_COMBOBOX,
                        CtlID: id as u32,
                        itemID: index as u32,
                        itemAction: ODA_DRAWENTIRE,
                        itemState: ODS_COMBOBOXEDIT | if disabled { ODS_DISABLED } else { 0 },
                        hwndItem: hwnd,
                        hDC: dc,
                        rcItem: info.rcItem,
                        itemData: SendMessageW(hwnd, CB_GETITEMDATA, index as usize, 0) as usize,
                    };
                    // Preserve application-owned painting and its item data.
                    let draw_saved = SaveDC(dc);
                    SendMessageW(
                        GetParent(hwnd),
                        WM_DRAWITEM,
                        id as usize,
                        (&draw as *const DRAWITEMSTRUCT) as isize,
                    );
                    RestoreDC(dc, draw_saved);
                    if !combo.alive.get() {
                        RestoreDC(dc, saved);
                        return;
                    }
                } else {
                    let len = SendMessageW(hwnd, CB_GETLBTEXTLEN, index as usize, 0);
                    if len >= 0 {
                        let mut text = vec![0u16; len as usize + 1];
                        SendMessageW(
                            hwnd,
                            CB_GETLBTEXT,
                            index as usize,
                            text.as_mut_ptr() as isize,
                        );
                        let mut text_rect = info.rcItem;
                        InflateRect(&mut text_rect, -2, 0);
                        let flags = DT_NOPREFIX | DT_LEFT | DT_VCENTER | DT_SINGLELINE;
                        let options = DTTOPTS {
                            dwSize: std::mem::size_of::<DTTOPTS>() as u32,
                            dwFlags: DTT_TEXTCOLOR,
                            crText: text_color,
                            ..std::mem::zeroed()
                        };
                        if theme == 0
                            || DrawThemeTextEx(
                                theme,
                                dc,
                                CP_DROPDOWNITEM,
                                state,
                                text.as_ptr(),
                                -1,
                                flags,
                                &mut text_rect,
                                &options,
                            ) < 0
                        {
                            DrawTextW(dc, text.as_ptr(), -1, &mut text_rect, flags);
                        }
                    }
                }
            }
            if !disabled && focus && SendMessageW(hwnd, CB_GETDROPPEDSTATE, 0, 0) == 0 {
                DrawFocusRect(dc, &info.rcItem);
            }
        } else {
            DrawThemeParentBackground(hwnd, dc, &rect);
            if !simple {
                FillRect(dc, &arrow, brush_role(background(disabled, hot)));
            }
        }
        if !simple {
            let themed_arrow = RECT {
                top: arrow.top - 1,
                bottom: arrow.bottom - 1,
                ..arrow
            };
            if theme == 0
                || DrawThemeBackground(
                    theme,
                    dc,
                    CP_DROPDOWNBUTTONRIGHT,
                    if disabled {
                        CBXSR_DISABLED
                    } else {
                        CBXSR_NORMAL
                    },
                    &themed_arrow,
                    ptr::null(),
                ) < 0
            {
                SetTextColor(
                    dc,
                    color_role(if disabled {
                        Role::DisabledText
                    } else if hot {
                        Role::Text
                    } else {
                        Role::DimText
                    }),
                );
                DrawTextW(
                    dc,
                    w!("\u{02c5}"),
                    -1,
                    &mut arrow,
                    DT_NOPREFIX | DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
                );
            }
        }
        if list {
            ExcludeClipRect(
                dc,
                rect.left + 1,
                rect.top + 1,
                rect.right - 1,
                rect.bottom - 1,
            );
        } else {
            ExcludeClipRect(
                dc,
                info.rcItem.left,
                info.rcItem.top,
                info.rcItem.right,
                info.rcItem.bottom,
            );
            if simple && !info.hwndList.is_null() {
                let mut item = info.rcItem;
                MapWindowPoints(info.hwndItem, hwnd, (&mut item as *mut RECT).cast(), 2);
                rect.bottom = item.bottom;
            }
            let mut inner = rect;
            InflateRect(&mut inner, -1, -1);
            if !simple {
                let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(border)));
                let old = SelectObject(dc, pen.0);
                let points = [
                    POINT {
                        x: arrow.left - 1,
                        y: arrow.top,
                    },
                    POINT {
                        x: arrow.left - 1,
                        y: arrow.bottom,
                    },
                ];
                Polyline(dc, points.as_ptr(), 2);
                SelectObject(dc, old);
                ExcludeClipRect(dc, arrow.left - 1, arrow.top, arrow.right, arrow.bottom);
                inner.right = arrow.left - 1;
            }
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
            InflateRect(&mut inner, -1, -1);
            FillRect(
                dc,
                &inner,
                brush_role(if disabled {
                    Role::Dialog
                } else {
                    Role::Control
                }),
            );
        }
        outline(dc, rect, border, true);
        RestoreDC(dc, saved);
    }
}

unsafe fn current_state(hwnd: HWND) -> i32 {
    unsafe {
        let mut cursor: POINT = std::mem::zeroed();
        GetCursorPos(&mut cursor);
        ScreenToClient(hwnd, &mut cursor);
        if IsWindowEnabled(hwnd) == 0 {
            CBXSR_DISABLED
        } else if PtInRect(&client(hwnd), cursor) != 0 {
            CBXSR_HOT
        } else {
            CBXSR_NORMAL
        }
    }
}

unsafe fn paint(hwnd: HWND, dc: HDC, combo: &Combo, dirty: RECT) {
    unsafe {
        let rect = client(hwnd);
        let state = current_state(hwnd);
        if combo.animates() {
            if animation(|api| (api.render)(hwnd, dc) != 0) {
                return;
            }
            let mut params = BP_ANIMATIONPARAMS {
                cbSize: std::mem::size_of::<BP_ANIMATIONPARAMS>() as u32,
                style: BPAS_LINEAR,
                ..std::mem::zeroed()
            };
            if state != combo.state.get() {
                GetThemeTransitionDuration(
                    combo.theme(hwnd),
                    CP_DROPDOWNBUTTONRIGHT,
                    combo.state.get(),
                    state,
                    TMT_TRANSITIONDURATIONS as i32,
                    &mut params.dwDuration,
                );
            }
            let mut from = ptr::null_mut();
            let mut to = ptr::null_mut();
            let buffer = animation(|api| {
                (api.begin)(
                    hwnd,
                    dc,
                    &rect,
                    BPBF_COMPATIBLEBITMAP,
                    ptr::null(),
                    &params,
                    &mut from,
                    &mut to,
                )
            });
            if buffer != 0 {
                if !from.is_null() {
                    render(hwnd, from, combo, combo.state.get());
                }
                if !to.is_null() && combo.alive.get() {
                    render(hwnd, to, combo, state);
                }
                combo.state.set(state);
                animation(|api| (api.end)(buffer, i32::from(combo.alive.get())));
                return;
            }
        }
        if combo.style == CBS_DROPDOWNLIST {
            buffered_combo(hwnd, dc, combo, state, dirty);
        } else {
            render(hwnd, dc, combo, state);
        }
        combo.state.set(state);
    }
}

/// PaintWithBuffer clips both rendering and copying to the invalid rectangle.
unsafe fn buffered_combo(hwnd: HWND, target: HDC, combo: &Combo, state: i32, dirty: RECT) {
    unsafe {
        let area = client(hwnd);
        if area.right <= 0
            || area.bottom <= 0
            || dirty.right <= dirty.left
            || dirty.bottom <= dirty.top
        {
            return;
        }
        // Do not retain a RefCell borrow across native owner draw. A nested paint
        // leases a different surface, and destruction cannot free the active one.
        let mut buffer = combo.buffer.borrow_mut().take();
        if buffer
            .as_ref()
            .is_some_and(|b| b.width != area.right || b.height != area.bottom)
        {
            buffer = None;
        }
        let buffer = buffer.or_else(|| ComboBuffer::new(target, area.right, area.bottom));
        let dc = buffer.as_ref().map_or(target, |b| b.dc);
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let layout = GetLayout(target);
        if buffer.is_some() && layout != u32::MAX {
            SetLayout(dc, layout);
        }
        IntersectClipRect(dc, dirty.left, dirty.top, dirty.right, dirty.bottom);
        // The cache is leased out during owner drawing. Keep every live fallback
        // DC identifiable by WM_ERASEBKGND, including across nested paints.
        let _active = buffer.as_ref().map(|_| {
            combo.active_buffers.borrow_mut().push(dc);
            ActiveComboBuffer(&combo.active_buffers)
        });
        render(hwnd, dc, combo, state);
        if buffer.is_some() && combo.alive.get() {
            BitBlt(
                target,
                dirty.left,
                dirty.top,
                dirty.right - dirty.left,
                dirty.bottom - dirty.top,
                dc,
                dirty.left,
                dirty.top,
                SRCCOPY,
            );
        }
        RestoreDC(dc, saved);
        if combo.alive.get() {
            let current = client(hwnd);
            let mut cached = combo.buffer.borrow_mut();
            if cached.is_none() && current.right == area.right && current.bottom == area.bottom {
                *cached = buffer;
            }
        }
    }
}

unsafe extern "system" fn combo_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        // Parent WM_DRAWITEM may destroy the control. Keep state alive across callbacks.
        let raw = data as *const Combo;
        Rc::increment_strong_count(raw);
        let combo = Rc::from_raw(raw);
        if msg == WM_NCDESTROY {
            combo.alive.set(false);
            combo.buffer.borrow_mut().take();
            remove_subclass(hwnd, Some(combo_proc), id);
            drop(Rc::from_raw(raw));
        } else {
            match msg {
                WM_THEMECHANGED | WM_DPICHANGED_AFTERPARENT => {
                    combo.close();
                    if combo.buffered {
                        stop_animation(hwnd);
                    }
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                WM_SIZE | WM_DESTROY if combo.buffered => {
                    stop_animation(hwnd);
                }
                WM_ERASEBKGND if is_dark() && combo.theme(hwnd) != 0 => {
                    // Upstream ComboBoxSubclass leaves target-DC erasure pending
                    // for the non-animated dropdown-list fallback; only its own
                    // compatible surface acknowledges erasure. Missing animation
                    // APIs follow the same contract as disabled animation.
                    return isize::from(
                        combo.style != CBS_DROPDOWNLIST
                            || combo.animates()
                            || combo.owns_buffer_dc(wp as HDC),
                    );
                }
                WM_PAINT if is_dark() => {
                    let mut ps = std::mem::zeroed();
                    let dc = BeginPaint(hwnd, &mut ps);
                    if !dc.is_null() {
                        paint(hwnd, dc, &combo, ps.rcPaint);
                    }
                    EndPaint(hwnd, &ps);
                    return 0;
                }
                WM_PRINTCLIENT if is_dark() && wp != 0 => {
                    // A snapshot renders the current state synchronously, without
                    // consuming, starting, or updating the live animation.
                    render(hwnd, wp as HDC, &combo, current_state(hwnd));
                    return 0;
                }
                WM_MOUSEMOVE if is_dark() => {
                    track(hwnd, false);
                    if combo.state.get() != CBXSR_HOT {
                        InvalidateRect(hwnd, ptr::null(), 0);
                    }
                }
                WM_MOUSELEAVE | WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE if is_dark() => {
                    let result = default_subclass(hwnd, msg, wp, lp);
                    InvalidateRect(hwnd, ptr::null(), 0);
                    return result;
                }
                _ => {}
            }
        }
        default_subclass(hwnd, msg, wp, lp)
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
                    assert_eq!(
                        GetWindowSubclass(hwnd, Some(combo_proc), COMBO, ptr::null_mut()),
                        0,
                        "PREFER_THEME must leave combo painting to the native control"
                    );
                    assert_eq!(SendMessageW(hwnd, CB_GETCURSEL, 0, 0), selected);
                    assert_eq!(SendMessageW(hwnd, CB_GETEDITSEL, 0, 0), edit_selection);
                }
                EnableWindow(hwnd, 0);
                let area = client(hwnd);
                let dc = GetDC(hwnd);
                let target = ComboBuffer::new(dc, area.right, area.bottom).unwrap();
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

    // These tests attach the custom fallback explicitly; PREFER_THEME must not
    // install it on supported systems, but its buffer/owner-draw contract remains.
    fn fallback_combo() -> Combo {
        Combo {
            style: CBS_DROPDOWNLIST,
            theme: Cell::new(0),
            state: Cell::new(CBXSR_NORMAL),
            buffered: false,
            buffer: RefCell::new(None),
            active_buffers: RefCell::new(Vec::new()),
            alive: Cell::new(true),
        }
    }

    #[test]
    fn fallback_erasure_acknowledges_only_its_compatible_dc() {
        if !super::super::supported() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        super::super::set_mode(2);
        unsafe {
            for style in [CBS_DROPDOWNLIST, CBS_DROPDOWN, CBS_SIMPLE] {
                let hwnd = test_window(
                    w!("COMBOBOX"),
                    ptr::null(),
                    WS_POPUP | style as u32,
                    crate::layout::rect(0, 0, 100, 100),
                );
                let mut state = fallback_combo();
                state.style = style;
                let combo = Rc::new(state);
                assert_ne!(
                    combo.theme(hwnd),
                    0,
                    "the erase contract requires a native theme"
                );
                assert_ne!(
                    set_subclass(
                        hwnd,
                        Some(combo_proc),
                        COMBO,
                        Rc::into_raw(combo.clone()) as usize,
                    ),
                    0
                );
                let screen = GetDC(hwnd);
                let target = ComboBuffer::new(screen, 160, 100).unwrap();
                let marker = GdiObject(CreateSolidBrush(0x00123456));
                FillRect(target.dc, &client(hwnd), marker.0);
                let expected = isize::from(style != CBS_DROPDOWNLIST);
                assert_eq!(
                    SendMessageW(hwnd, WM_ERASEBKGND, target.dc as usize, 0),
                    expected,
                    "only dropdown-list fallback target erasure must remain pending"
                );
                assert_eq!(
                    GetPixel(target.dc, 3, 3),
                    0x00123456,
                    "acknowledging erasure must not paint the caller's DC"
                );
                if style == CBS_DROPDOWNLIST {
                    buffered_combo(hwnd, target.dc, &combo, CBXSR_NORMAL, client(hwnd));
                    let cached = combo.buffer.borrow().as_ref().unwrap().dc;
                    assert_eq!(
                        SendMessageW(hwnd, WM_ERASEBKGND, cached as usize, 0),
                        1,
                        "cached compatible DC owns fallback erasure"
                    );
                    assert_eq!(
                        SendMessageW(hwnd, WM_ERASEBKGND, target.dc as usize, 0),
                        0,
                        "allocating a cache must not claim the target DC"
                    );
                }
                ReleaseDC(hwnd, screen);
                DestroyWindow(hwnd);
            }
        }
        super::super::set_mode(0);
    }

    #[test]
    fn fallback_surface_clips_reuses_and_resizes() {
        use crate::test_support as surfaces;

        unsafe {
            let hwnd = test_window(
                w!("COMBOBOX"),
                ptr::null(),
                WS_POPUP | CBS_DROPDOWNLIST as u32,
                crate::layout::rect(0, 0, 100, 100),
            );
            let combo = fallback_combo();
            let screen = GetDC(hwnd);
            let target = ComboBuffer::new(screen, 160, 80).unwrap();
            let full = crate::layout::rect(0, 0, 160, 80);
            surfaces::mark_surface(target.dc, full);
            let dirty = RECT {
                left: 10,
                top: 5,
                right: 20,
                bottom: 10,
            };
            buffered_combo(hwnd, target.dc, &combo, CBXSR_NORMAL, dirty);
            surfaces::assert_surface_clipped(target.dc);
            assert_eq!(
                GetPixel(target.dc, 15, 7),
                color_role(Role::Control),
                "the invalid region must receive the combo background"
            );
            let (dc, bitmap) = {
                let cached = combo.buffer.borrow();
                let cached = cached.as_ref().unwrap();
                surfaces::mark_surface(cached.dc, full);
                (cached.dc, cached.bitmap)
            };
            buffered_combo(hwnd, target.dc, &combo, CBXSR_NORMAL, dirty);
            {
                let cached = combo.buffer.borrow();
                let cached = cached.as_ref().unwrap();
                surfaces::assert_surface_reused((dc, bitmap), (cached.dc, cached.bitmap));
            }
            surfaces::resize_surface_window(hwnd, 140, 100);
            let area = client(hwnd);
            buffered_combo(hwnd, target.dc, &combo, CBXSR_NORMAL, area);
            assert_eq!(
                surfaces::bitmap_size(combo.buffer.borrow().as_ref().unwrap().bitmap),
                (area.right, area.bottom),
                "resizing must replace the backing bitmap with the new client size"
            );
            ReleaseDC(hwnd, screen);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn owner_draw_can_reenter_and_destroy_a_buffered_combo() {
        if !super::super::supported() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        super::super::set_mode(2);
        unsafe {
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                crate::layout::rect(0, 0, 160, 100),
            );
            let hwnd = CreateWindowExW(
                0,
                w!("COMBOBOX"),
                ptr::null(),
                WS_CHILD | (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED) as u32,
                0,
                0,
                100,
                100,
                parent,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            assert!(!hwnd.is_null());
            SendMessageW(hwnd, CB_ADDSTRING, 0, 123);
            SendMessageW(hwnd, CB_SETCURSEL, 0, 0);
            let combo = Rc::new(fallback_combo());
            assert_ne!(
                set_subclass(
                    hwnd,
                    Some(combo_proc),
                    COMBO,
                    Rc::into_raw(combo.clone()) as usize
                ),
                0
            );
            let outer = Cell::new(ptr::null_mut());
            let nested = Cell::new(false);
            let independent = Cell::new(false);
            let erasure_valid = Cell::new(true);
            let observe = |_, msg, _, lp| {
                if msg != WM_DRAWITEM {
                    return None;
                }
                let draw = &*(lp as *const DRAWITEMSTRUCT);
                erasure_valid.set(
                    erasure_valid.get()
                        && SendMessageW(draw.hwndItem, WM_ERASEBKGND, draw.hDC as usize, 0) == 1,
                );
                if nested.replace(true) {
                    erasure_valid.set(
                        erasure_valid.get()
                            && SendMessageW(draw.hwndItem, WM_ERASEBKGND, outer.get() as usize, 0)
                                == 1,
                    );
                    independent.set(draw.hDC != outer.get());
                    DestroyWindow(draw.hwndItem);
                } else {
                    outer.set(draw.hDC);
                    let target = ComboBuffer::new(draw.hDC, 160, 80).unwrap();
                    buffered_combo(
                        draw.hwndItem,
                        target.dc,
                        &combo,
                        CBXSR_NORMAL,
                        client(draw.hwndItem),
                    );
                }
                Some(1)
            };
            let callback = crate::test_support::observe_messages(parent, 1, &observe);
            let screen = GetDC(parent);
            let target = ComboBuffer::new(screen, 160, 80).unwrap();
            buffered_combo(hwnd, target.dc, &combo, CBXSR_NORMAL, client(hwnd));
            assert!(nested.get(), "native owner drawing must be dispatched");
            assert!(independent.get(), "nested paints need independent surfaces");
            assert!(
                erasure_valid.get(),
                "each live fallback surface must acknowledge erasure throughout nested owner drawing"
            );
            assert_eq!(
                IsWindow(hwnd),
                0,
                "owner drawing must be allowed to destroy the combo"
            );
            assert!(
                combo.buffer.borrow().is_none(),
                "an active paint must not restore a cache after control destruction"
            );
            ReleaseDC(parent, screen);
            remove_subclass(parent, callback, 1);
            DestroyWindow(parent);
        }
        super::super::set_mode(0);
    }
}

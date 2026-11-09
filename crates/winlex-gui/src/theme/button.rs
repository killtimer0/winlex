// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 ozone10
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Rust port of win32-darkmodelib's DmlibSubclassControl.cpp / DmlibSubclass.h:
// https://github.com/ozone10/win32-darkmodelib/tree/e8ec95b

use super::{
    Role,
    animation::{animation, client_animation_enabled, stop_animation},
};
use crate::native::*;
use std::{cell::Cell, mem, ptr, rc::Rc};
use windows_sys::Win32::UI::Shell::GetWindowSubclass;

const SUBCLASS: usize = 0x574c424e;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Check,
    Radio,
    Group,
}

fn kind(style: u32) -> Option<Kind> {
    if style & BS_PUSHLIKE as u32 != 0 {
        return None;
    }
    match (style & BS_TYPEMASK as u32) as i32 {
        BS_CHECKBOX | BS_AUTOCHECKBOX | BS_3STATE | BS_AUTO3STATE => Some(Kind::Check),
        BS_RADIOBUTTON | BS_AUTORADIOBUTTON => Some(Kind::Radio),
        BS_GROUPBOX => Some(Kind::Group),
        _ => None,
    }
}

struct Button {
    theme: Cell<HTHEME>,
    state: Cell<i32>,
    buffered: bool,
    alive: Cell<bool>,
}

impl Button {
    fn ensure_theme(&self, hwnd: HWND) -> HTHEME {
        if self.theme.get() == 0 {
            self.theme.set(unsafe { OpenThemeData(hwnd, w!("Button")) });
        }
        self.theme.get()
    }

    fn reset(&self, hwnd: HWND) {
        stop_animation(hwnd);
        self.state.set(0);
        let theme = self.theme.replace(0);
        if theme != 0 {
            unsafe { CloseThemeData(theme) };
        }
    }
}

impl Drop for Button {
    fn drop(&mut self) {
        unsafe {
            if self.theme.get() != 0 {
                CloseThemeData(self.theme.get());
            }
            if self.buffered {
                animation(|api| (api.uninit)());
            }
        }
    }
}

/// Installs only the checkbox/radio/group-box renderer, without changing styles.
/// The caller gates installation to Windows 10 build 17763 or newer.
pub(super) fn install(hwnd: HWND) {
    unsafe {
        if kind(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32).is_none() {
            return;
        }
        let mut existing = 0;
        if GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut existing) != 0 {
            return;
        }
        let data = Rc::new(Button {
            theme: Cell::new(0),
            state: Cell::new(0),
            buffered: animation(|api| (api.init)() >= 0),
            alive: Cell::new(true),
        });
        let data = Rc::into_raw(data);
        if set_subclass(hwnd, Some(subclass), SUBCLASS, data as usize) == 0 {
            drop(Rc::from_raw(data));
        }
    }
}

// Cell-backed data is shared across synchronous SendMessage/parent-paint calls;
// never retain an exclusive Rust reference across Win32 reentrancy.
unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        // Drawing the parent background can destroy this control synchronously.
        let raw = data as *const Button;
        Rc::increment_strong_count(raw);
        let button = Rc::from_raw(raw);
        if message == WM_NCDESTROY {
            button.alive.set(false);
            stop_animation(hwnd);
            remove_subclass(hwnd, Some(subclass), id);
            drop(Rc::from_raw(raw));
            return default_subclass(hwnd, message, wp, lp);
        }
        match message {
            WM_THEMECHANGED | WM_DPICHANGED_AFTERPARENT | WM_DPICHANGED => {
                button.reset(hwnd);
                InvalidateRect(hwnd, ptr::null(), 0);
            }
            WM_SIZE | WM_DESTROY => {
                stop_animation(hwnd);
                button.state.set(0);
            }
            WM_STYLECHANGED => {
                button.reset(hwnd);
                InvalidateRect(hwnd, ptr::null(), 0);
            }
            _ => {}
        }

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let Some(kind) = kind(style).filter(|_| super::is_dark()) else {
            return default_subclass(hwnd, message, wp, lp);
        };
        match message {
            WM_ERASEBKGND if button.ensure_theme(hwnd) != 0 => return 1,
            WM_PAINT | WM_PRINTCLIENT if button.ensure_theme(hwnd) != 0 => {
                let mut ps: PAINTSTRUCT = mem::zeroed();
                let dc = if wp == 0 {
                    BeginPaint(hwnd, &mut ps)
                } else {
                    wp as HDC
                };
                if !dc.is_null() {
                    if kind == Kind::Group {
                        paint_group(hwnd, dc, &button);
                    } else {
                        paint_button(hwnd, dc, &button, kind, message == WM_PAINT);
                    }
                }
                if !ps.hdc.is_null() {
                    EndPaint(hwnd, &ps);
                }
                return 0;
            }
            WM_ENABLE => {
                // BUTTON redraws synchronously in its WM_ENABLE handler, outside
                // WM_PAINT. Bypass that handler rather than flashing a light glyph.
                let result = DefWindowProcW(hwnd, message, wp, lp);
                stop_animation(hwnd);
                if button.alive.get() {
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                return result;
            }
            WM_UPDATEUISTATE | WM_CHANGEUISTATE | WM_SETFONT | WM_SETTEXT | BM_SETCHECK
            | BM_SETSTATE | WM_SETFOCUS | WM_KILLFOCUS => {
                let result = default_subclass(hwnd, message, wp, lp);
                if button.alive.get() {
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                return result;
            }
            _ => {}
        }
        default_subclass(hwnd, message, wp, lp)
    }
}

fn visual_state(kind: Kind, enabled: bool, state: u32) -> i32 {
    let base = if !enabled {
        CBS_UNCHECKEDDISABLED
    } else if state & BST_PUSHED != 0 {
        CBS_UNCHECKEDPRESSED
    } else if state & BST_HOT != 0 {
        CBS_UNCHECKEDHOT
    } else {
        CBS_UNCHECKEDNORMAL
    };
    base + if state & BST_CHECKED != 0 {
        4
    } else if kind == Kind::Check && state & BST_INDETERMINATE != 0 {
        8
    } else {
        0
    }
}

fn paint_button(hwnd: HWND, dc: HDC, button: &Button, kind: Kind, animate: bool) {
    unsafe {
        let theme = button.theme.get();
        let part = if kind == Kind::Radio {
            BP_RADIOBUTTON
        } else {
            BP_CHECKBOX
        };
        let state = visual_state(
            kind,
            IsWindowEnabled(hwnd) != 0,
            SendMessageW(hwnd, BM_GETSTATE, 0, 0) as u32,
        );
        if !button.alive.get() {
            return;
        }
        let animate = animate && button.buffered && client_animation_enabled();
        if !animate {
            stop_animation(hwnd);
            render_button(hwnd, dc, button, theme, part, state);
            if button.alive.get() {
                button.state.set(state);
            }
            return;
        }
        if animation(|api| (api.render)(hwnd, dc) != 0) {
            return;
        }
        let previous = if button.state.get() == 0 {
            state
        } else {
            button.state.get()
        };
        let mut params = BP_ANIMATIONPARAMS {
            cbSize: mem::size_of::<BP_ANIMATIONPARAMS>() as u32,
            style: BPAS_LINEAR,
            ..mem::zeroed()
        };
        if previous != state {
            GetThemeTransitionDuration(
                theme,
                part,
                previous,
                state,
                TMT_TRANSITIONDURATIONS as i32,
                &mut params.dwDuration,
            );
        }
        let rect = client(hwnd);
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
                render_button(hwnd, from, button, theme, part, previous);
            }
            if !to.is_null() && button.alive.get() {
                render_button(hwnd, to, button, theme, part, state);
            }
            if button.alive.get() {
                button.state.set(state);
            }
            animation(|api| (api.end)(buffer, i32::from(button.alive.get())));
        } else {
            render_button(hwnd, dc, button, theme, part, state);
            if button.alive.get() {
                button.state.set(state);
            }
        }
    }
}

struct SavedDc(HDC, i32);
impl SavedDc {
    fn new(dc: HDC) -> Self {
        Self(dc, unsafe { SaveDC(dc) })
    }
}
impl Drop for SavedDc {
    fn drop(&mut self) {
        if self.1 != 0 {
            unsafe { RestoreDC(self.0, self.1) };
        }
    }
}

struct Font {
    dc: HDC,
    old: HGDIOBJ,
    owned: GdiObject,
}
impl Font {
    fn select(hwnd: HWND, dc: HDC, theme: HTHEME, part: i32, state: i32) -> Self {
        unsafe {
            let mut lf = mem::zeroed();
            let mut owned = GdiObject::default();
            if GetThemeFont(theme, dc, part, state, TMT_FONT as i32, &mut lf) >= 0 {
                owned.0 = CreateFontIndirectW(&lf);
            }
            let font = if owned.0.is_null() {
                SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT
            } else {
                owned.0
            };
            Self {
                dc,
                old: if font.is_null() {
                    ptr::null_mut()
                } else {
                    SelectObject(dc, font)
                },
                owned,
            }
        }
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        if !self.old.is_null() {
            unsafe { SelectObject(self.dc, self.old) };
        }
        // Keep the owned font alive until after it has been deselected.
        let _ = &self.owned;
    }
}

fn text_options(hwnd: HWND) -> DTTOPTS {
    DTTOPTS {
        dwSize: mem::size_of::<DTTOPTS>() as u32,
        dwFlags: DTT_TEXTCOLOR,
        crText: super::color_role(if unsafe { IsWindowEnabled(hwnd) } == 0 {
            Role::DisabledText
        } else {
            Role::Text
        }),
        ..unsafe { mem::zeroed() }
    }
}

fn render_button(hwnd: HWND, dc: HDC, button: &Button, theme: HTHEME, part: i32, state: i32) {
    unsafe {
        let _saved = SavedDc::new(dc);
        let _font = Font::select(hwnd, dc, theme, part, state);
        if !button.alive.get() {
            return;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let multiline = style & BS_MULTILINE as u32 != 0;
        let vertical = style & BS_VCENTER as u32;
        let horizontal = style & BS_CENTER as u32;
        let mut flags = if multiline {
            DT_WORDBREAK
        } else {
            DT_SINGLELINE
        };
        flags |= match horizontal as i32 {
            BS_CENTER => DT_CENTER,
            BS_RIGHT => DT_RIGHT,
            _ => DT_LEFT,
        };
        flags |= match vertical as i32 {
            BS_VCENTER => DT_VCENTER,
            BS_BOTTOM => DT_BOTTOM,
            0 if !multiline => DT_VCENTER,
            _ => 0,
        };
        let ui = SendMessageW(hwnd, WM_QUERYUISTATE, 0, 0) as u32;
        if ui & UISF_HIDEACCEL != 0 {
            flags |= DT_HIDEPREFIX;
        }
        let rect = client(hwnd);
        let text = text_wide(hwnd);
        let mut size = mem::zeroed::<SIZE>();
        GetThemePartSize(theme, dc, part, state, ptr::null(), TS_DRAW, &mut size);
        let mut text_rect = rect;
        GetThemeBackgroundContentRect(theme, dc, part, state, &rect, &mut text_rect);
        let mut glyph = rect;
        if !multiline {
            glyph.top += (text_rect.bottom - text_rect.top - size.cy) / 2;
        }
        glyph.bottom = glyph.top + size.cy;
        glyph.right = glyph.left + size.cx;
        if style & BS_LEFTTEXT as u32 != 0 {
            glyph.right = rect.right;
            glyph.left = glyph.right - size.cx;
            text_rect.right = glyph.left - 3;
        } else {
            text_rect.left = glyph.right + 3;
        }
        DrawThemeParentBackground(hwnd, dc, &rect);
        if !button.alive.get() {
            return;
        }
        if super::windows11() || !draw_glyph(dc, part, state, glyph) {
            DrawThemeBackground(theme, dc, part, state, &glyph, ptr::null());
        }
        let mut options = text_options(hwnd);
        DrawThemeTextEx(
            theme,
            dc,
            part,
            state,
            text.as_ptr(),
            -1,
            flags,
            &mut text_rect,
            &options,
        );
        if SendMessageW(hwnd, BM_GETSTATE, 0, 0) as u32 & BST_FOCUS != 0 && ui & UISF_HIDEFOCUS == 0
        {
            options.dwFlags |= DTT_CALCRECT;
            DrawThemeTextEx(
                theme,
                dc,
                part,
                state,
                text.as_ptr(),
                -1,
                flags | DT_CALCRECT,
                &mut text_rect,
                &options,
            );
            text_rect.left -= 1;
            text_rect.right += 1;
            text_rect.bottom += 1;
            DrawFocusRect(dc, &text_rect);
        }
    }
}

/// Windows 10's native theme does not provide the dark glyphs used by Windows 11.
fn draw_glyph(dc: HDC, part: i32, state: i32, rect: RECT) -> bool {
    if dc.is_null() || !(1..=12).contains(&state) {
        return false;
    }
    let side = (rect.right - rect.left).min(rect.bottom - rect.top);
    if side <= 0 {
        return false;
    }
    let saved = SavedDc::new(dc);
    if saved.1 == 0 {
        return false;
    }
    let color = super::color_role(match (state - 1) % 4 {
        3 => Role::DisabledText,
        1 | 2 => Role::Link,
        _ => Role::Text,
    });
    let checked = (5..=8).contains(&state);
    let mixed = state >= 9;
    let x = rect.left + (rect.right - rect.left - side) / 2;
    let y = rect.top + (rect.bottom - rect.top - side) / 2;
    let inset = 3.max(side / 4);
    unsafe {
        SetDCBrushColor(dc, color);
        SetDCPenColor(dc, color);
        SelectObject(dc, GetStockObject(NULL_BRUSH));
        SelectObject(dc, GetStockObject(DC_PEN));
        if part == BP_RADIOBUTTON {
            Ellipse(dc, x, y, x + side, y + side);
            if checked {
                SelectObject(dc, GetStockObject(DC_BRUSH));
                Ellipse(dc, x + inset, y + inset, x + side - inset, y + side - inset);
            }
        } else {
            Rectangle(dc, x, y, x + side, y + side);
            if mixed {
                let width = 1.max(side - 2 * inset);
                let height = 1.max(width / 4);
                let top = y + (side - height) / 2;
                SelectObject(dc, GetStockObject(DC_BRUSH));
                Rectangle(dc, x + inset, top, x + inset + width, top + height);
            } else if checked {
                let half = side / 2 - inset;
                let cx = x + side / 2;
                let cy = y + side / 2;
                let pen = GdiObject(CreatePen(PS_SOLID, 2.max(side / 8), color));
                let old = SelectObject(dc, pen.0);
                MoveToEx(dc, cx - half, cy - half, ptr::null_mut());
                LineTo(dc, cx + half, cy + half);
                MoveToEx(dc, cx + half, cy - half, ptr::null_mut());
                LineTo(dc, cx - half, cy + half);
                SelectObject(dc, old);
            }
        }
    }
    true
}

fn paint_group(hwnd: HWND, dc: HDC, button: &Button) {
    unsafe {
        let theme = button.theme.get();
        let _saved = SavedDc::new(dc);
        let state = if IsWindowEnabled(hwnd) == 0 {
            GBS_DISABLED
        } else {
            GBS_NORMAL
        };
        let _font = Font::select(hwnd, dc, theme, BP_GROUPBOX, state);
        if !button.alive.get() {
            return;
        }
        let text = text_wide(hwnd);
        let length = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        let centered =
            GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & BS_CENTER as u32 == BS_CENTER as u32;
        let mut rect = client(hwnd);
        rect.bottom -= 1;
        let mut text_rect = rect;
        let mut frame = rect;
        let mut size = mem::zeroed::<SIZE>();
        if length != 0 {
            GetTextExtentPoint32W(dc, text.as_ptr(), length as i32, &mut size);
            text_rect.left += if centered {
                (rect.right - rect.left - size.cx) / 2
            } else {
                7
            };
            text_rect.bottom = text_rect.top + size.cy;
            text_rect.right = text_rect.left + size.cx + 4;
        } else {
            GetTextExtentPoint32W(dc, w!("M"), 1, &mut size);
        }
        frame.top += size.cy / 2;
        // Preserve the caller's clip (including a WM_PRINTCLIENT clip), rather
        // than clearing it as the C++ implementation does after the frame.
        {
            let _clip = SavedDc::new(dc);
            if length != 0 {
                ExcludeClipRect(
                    dc,
                    text_rect.left,
                    text_rect.top,
                    text_rect.right,
                    text_rect.bottom,
                );
            }
            let mut content = frame;
            GetThemeBackgroundContentRect(theme, dc, BP_GROUPBOX, state, &frame, &mut content);
            ExcludeClipRect(dc, content.left, content.top, content.right, content.bottom);
            SelectObject(dc, GetStockObject(NULL_BRUSH));
            SelectObject(dc, GetStockObject(DC_PEN));
            SetDCPenColor(dc, super::color_role(Role::Edge));
            Rectangle(dc, frame.left, frame.top, frame.right, frame.bottom);
        }
        if length != 0 {
            InflateRect(&mut text_rect, -2, 0);
            let mut flags = DT_SINGLELINE | if centered { DT_CENTER } else { DT_LEFT };
            if SendMessageW(hwnd, WM_QUERYUISTATE, 0, 0) as u32 & UISF_HIDEACCEL != 0 {
                flags |= DT_HIDEPREFIX;
            }
            DrawThemeTextEx(
                theme,
                dc,
                BP_GROUPBOX,
                state,
                text.as_ptr(),
                -1,
                flags,
                &mut text_rect,
                &text_options(hwnd),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_background_can_destroy_a_painting_checkbox() {
        if !super::super::supported() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        super::super::set_mode(2);
        unsafe {
            for message in [WM_PRINTCLIENT, WM_PAINT] {
                let parent = test_window(
                    w!("STATIC"),
                    ptr::null(),
                    WS_POPUP,
                    crate::layout::rect(0, 0, 200, 100),
                );
                let hwnd = CreateWindowExW(
                    0,
                    w!("BUTTON"),
                    w!("Check"),
                    WS_CHILD | BS_AUTOCHECKBOX as u32,
                    0,
                    0,
                    150,
                    30,
                    parent,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                );
                assert!(!hwnd.is_null());
                install(hwnd);
                let mut raw = 0;
                assert_ne!(
                    GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut raw),
                    0
                );
                let raw = raw as *const Button;
                Rc::increment_strong_count(raw);
                let state = Rc::from_raw(raw);
                assert_ne!(state.ensure_theme(hwnd), 0);
                let weak = Rc::downgrade(&state);
                drop(state);
                let child = Cell::new(hwnd);
                let observe = |_, msg, _, _| {
                    if matches!(msg, WM_ERASEBKGND | WM_PRINTCLIENT) {
                        let target = child.replace(ptr::null_mut());
                        if !target.is_null() {
                            DestroyWindow(target);
                            return Some(1);
                        }
                    }
                    None
                };
                let callback = crate::test_support::observe_messages(parent, 1, &observe);
                let dc = GetDC(parent);
                let target = CreateCompatibleDC(dc);
                let bitmap = GdiObject(CreateCompatibleBitmap(dc, 150, 30));
                assert!(!target.is_null() && !bitmap.0.is_null());
                let old = SelectObject(target, bitmap.0);
                SendMessageW(hwnd, message, target as usize, PRF_CLIENT as isize);
                assert!(
                    child.get().is_null(),
                    "painting must enter the native parent background callback"
                );
                assert_eq!(
                    IsWindow(hwnd),
                    0,
                    "parent painting may destroy its checkbox"
                );
                assert!(
                    weak.upgrade().is_none(),
                    "the outer paint must release retained state after destruction"
                );
                SelectObject(target, old);
                DeleteDC(target);
                ReleaseDC(parent, dc);
                remove_subclass(parent, callback, 1);
                DestroyWindow(parent);
            }
        }
        super::super::set_mode(0);
    }

    #[test]
    fn native_three_state_cycle_survives_subclass_and_theme_refresh() {
        owned_handle! {
            struct Window(HWND); DestroyWindow
        }
        unsafe {
            let parent = Window(test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                crate::layout::rect(0, 0, 200, 100),
            ));
            let hwnd = CreateWindowExW(
                0,
                w!("BUTTON"),
                w!(""),
                WS_CHILD | BS_AUTO3STATE as u32,
                0,
                0,
                150,
                30,
                parent.0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            );
            assert!(!hwnd.is_null());
            install(hwnd);
            install(hwnd);
            for expected in [BST_CHECKED, BST_INDETERMINATE, BST_UNCHECKED] {
                SendMessageW(hwnd, BM_CLICK, 0, 0);
                assert_eq!(SendMessageW(hwnd, BM_GETCHECK, 0, 0) as u32, expected);
                EnableWindow(hwnd, 0);
                assert_eq!(IsWindowEnabled(hwnd), 0);
                SendMessageW(hwnd, WM_THEMECHANGED, 0, 0);
                EnableWindow(hwnd, 1);
                assert_ne!(IsWindowEnabled(hwnd), 0);
                assert_eq!(SendMessageW(hwnd, BM_GETCHECK, 0, 0) as u32, expected);
            }
        }
    }
}

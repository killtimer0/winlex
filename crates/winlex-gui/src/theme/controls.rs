// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 ozone10
// Rust adaptation of win32-darkmodelib, revision e8ec95b:
// DmlibSubclassControl.cpp (tabs, status bar, up-down) and
// DmlibSubclassWindow.cpp (toolbar custom draw), with BufferData/ThemeData
// from DmlibSubclass.h and PaintWithBuffer from DmlibPaintHelper.h.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::animation::{animation, client_animation_enabled, stop_animation};
use super::{
    Buffer, Role, brush_role, color_role, edge, is_dark, native_dark_theme, roundness, windows11,
};
use crate::native::*;
use std::{
    cell::{Cell, RefCell},
    ptr,
    rc::Rc,
};
use windows_sys::Win32::{
    UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi},
    UI::Shell::GetWindowSubclass,
};

const SUBCLASS: usize = 0x574c4443;

#[derive(Clone, Copy)]
pub(super) enum Control {
    Tab = 1,
    Status,
    Overflow,
}

struct ControlData {
    kind: Control,
    buffer: RefCell<Option<Buffer>>,
    active_dc: Cell<HDC>,
    status: RefCell<Option<Rc<StatusResources>>>,
    spin: RefCell<Option<Rc<Theme>>>,
    spin_states: Cell<[i32; 2]>,
    animated: bool,
}

impl ControlData {
    fn new(kind: Control) -> Self {
        Self {
            kind,
            buffer: RefCell::new(None),
            active_dc: Cell::new(ptr::null_mut()),
            status: RefCell::new(None),
            spin: RefCell::new(None),
            spin_states: Cell::new([UPS_NORMAL, UPS_NORMAL]),
            animated: matches!(kind, Control::Overflow)
                && animation(|api| unsafe { (api.init)() >= 0 }),
        }
    }

    fn status(&self, hwnd: HWND) -> Option<Rc<StatusResources>> {
        if !matches!(self.kind, Control::Status) {
            return None;
        }
        if self.status.borrow().is_none() {
            let resources = Rc::new(unsafe { StatusResources::new(hwnd) });
            *self.status.borrow_mut() = Some(resources);
        }
        self.status.borrow().clone()
    }

    fn spin(&self, hwnd: HWND) -> Rc<Theme> {
        if self.spin.borrow().is_none() {
            *self.spin.borrow_mut() = Some(Rc::new(Theme::empty()));
        }
        let theme = self.spin.borrow().as_ref().unwrap().clone();
        theme.get(hwnd, w!("SPIN"));
        theme
    }

    fn stop(&self, hwnd: HWND) {
        if matches!(self.kind, Control::Overflow) {
            stop_animation(hwnd);
            self.spin_states.set([UPS_NORMAL, UPS_NORMAL]);
        }
    }

    fn animates(&self) -> bool {
        self.animated && client_animation_enabled()
    }
}

impl Drop for ControlData {
    fn drop(&mut self) {
        if self.animated {
            animation(|api| unsafe { (api.uninit)() });
        }
    }
}

struct StatusResources {
    theme: Theme,
    font: GdiObject,
}

impl StatusResources {
    unsafe fn new(hwnd: HWND) -> Self {
        unsafe {
            let mut metrics: NONCLIENTMETRICSW = std::mem::zeroed();
            metrics.cbSize = std::mem::size_of::<NONCLIENTMETRICSW>() as u32;
            // Already used by dpi::Font and covered by the project's YY-Thunks.
            let font = if SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS,
                metrics.cbSize,
                (&mut metrics as *mut NONCLIENTMETRICSW).cast(),
                0,
                GetDpiForWindow(GetParent(hwnd)).max(96),
            ) != 0
            {
                CreateFontIndirectW(&metrics.lfStatusFont)
            } else {
                ptr::null_mut()
            };
            Self {
                theme: Theme::open(hwnd, w!("STATUS")),
                font: GdiObject(font),
            }
        }
    }
}

/// Install once; light mode and unsupported tab arrangements keep native painting.
/// Native dark tabs leave dynamic overflow creation entirely to comctl32.
/// Existing overflow buttons are reapplied separately, like upstream enumeration.
pub(super) fn install(hwnd: HWND, kind: Control) {
    if !matches!(kind, Control::Tab) || !native_dark_theme() {
        install_one(hwnd, kind);
    }
    if matches!(kind, Control::Tab) {
        unsafe {
            let overflow = FindWindowExW(hwnd, ptr::null_mut(), UPDOWN_CLASSW, ptr::null());
            if !overflow.is_null() {
                super::set_native_theme(overflow, "DarkMode_Explorer", None);
                install_one(overflow, Control::Overflow);
            }
        }
    }
}

fn install_one(hwnd: HWND, kind: Control) {
    unsafe {
        // This painter serves horizontal native tab overflow, not edit spinners.
        if matches!(kind, Control::Overflow)
            && GetWindowLongW(hwnd, GWL_STYLE) as u32 & UDS_HORZ == 0
        {
            return;
        }
        let mut data = 0;
        if GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut data) == 0 {
            let data = Rc::into_raw(Rc::new(ControlData::new(kind)));
            if set_subclass(hwnd, Some(subclass), SUBCLASS, data as usize) == 0 {
                drop(Rc::from_raw(data));
            }
        }
    }
}

fn custom(hwnd: HWND, kind: Control) -> bool {
    is_dark()
        && match kind {
            // Only fallback tabs install this subclass, so native dark tabs are
            // already excluded; vertical, bottom, button and owner-drawn styles
            // keep their native painting.
            Control::Tab => unsafe {
                GetWindowLongW(hwnd, GWL_STYLE) as u32
                    & (TCS_VERTICAL | TCS_BOTTOM | TCS_BUTTONS | TCS_OWNERDRAWFIXED)
                    == 0
            },
            Control::Status => !native_dark_theme(),
            Control::Overflow => true,
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
        // Owner drawing can reenter this subclass or destroy the control.
        let pointer = data as *const ControlData;
        Rc::increment_strong_count(pointer);
        let state = Rc::from_raw(pointer);
        if msg == WM_NCDESTROY {
            state.stop(hwnd);
            remove_subclass(hwnd, Some(subclass), SUBCLASS);
            drop(Rc::from_raw(pointer));
            return default_subclass(hwnd, msg, w, l);
        }
        let kind = state.kind;
        if matches!(
            msg,
            WM_THEMECHANGED | WM_DPICHANGED | WM_DPICHANGED_AFTERPARENT
        ) {
            state.stop(hwnd);
            state.status.borrow_mut().take();
            state.spin.borrow_mut().take();
        }
        if matches!(
            msg,
            WM_SIZE | WM_DESTROY | WM_STYLECHANGED | WM_SETTINGCHANGE
        ) {
            state.stop(hwnd);
        }
        if matches!(kind, Control::Tab) && msg == WM_PARENTNOTIFY && w as u16 as u32 == WM_CREATE {
            let mut name = [0; 32];
            let length = GetClassNameW(l as HWND, name.as_mut_ptr(), name.len() as i32);
            if name[..length.max(0) as usize] == wide("msctls_updown32")[..15] {
                install_one(l as HWND, Control::Overflow);
            }
        }
        if custom(hwnd, kind) {
            match msg {
                WM_ERASEBKGND => {
                    let cached = state.buffer.borrow();
                    return isize::from(
                        state.animates()
                            || w as HDC == state.active_dc.get()
                            || cached.as_ref().is_some_and(|buffer| buffer.dc == w as HDC),
                    );
                }
                WM_PAINT | WM_PRINTCLIENT => {
                    let mut ps: PAINTSTRUCT = std::mem::zeroed();
                    let dc = if msg == WM_PAINT {
                        BeginPaint(hwnd, &mut ps)
                    } else {
                        w as HDC
                    };
                    if !dc.is_null() {
                        let dirty = if msg == WM_PAINT {
                            ps.rcPaint
                        } else {
                            client(hwnd)
                        };
                        if matches!(kind, Control::Overflow) && msg == WM_PAINT {
                            paint_overflow(hwnd, dc, &state, dirty);
                        } else {
                            buffered(hwnd, dc, &state, dirty);
                        }
                    }
                    if msg == WM_PAINT {
                        EndPaint(hwnd, &ps);
                    }
                    return 0;
                }
                WM_MOUSEMOVE if !matches!(kind, Control::Status) => {
                    track_leave(hwnd);
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                _ => {}
            }
        }
        if matches!(kind, Control::Overflow) && !custom(hwnd, kind) {
            state.stop(hwnd);
        }
        // Native input, accessibility, selection, layout and owner data remain
        // with comctl32. Invalidate after it has committed the new state.
        let result = default_subclass(hwnd, msg, w, l);
        if matches!(
            msg,
            WM_THEMECHANGED
                | WM_SYSCOLORCHANGE
                | WM_SETTINGCHANGE
                | WM_SIZE
                | WM_DPICHANGED_AFTERPARENT
                | WM_STYLECHANGED
                | WM_SETFONT
                | WM_SETFOCUS
                | WM_KILLFOCUS
                | WM_UPDATEUISTATE
                | WM_ENABLE
                | WM_MOUSELEAVE
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_CAPTURECHANGED
                | WM_KEYDOWN
                | WM_KEYUP
                | WM_HSCROLL
                | TCM_SETCURSEL
                | TCM_SETCURFOCUS
        ) {
            InvalidateRect(hwnd, ptr::null(), 0);
        }
        result
    }
}

fn scale(hwnd: HWND, value: i32) -> i32 {
    crate::layout::scale(value, unsafe { GetDpiForWindow(hwnd) }.max(96))
}

unsafe fn font(hwnd: HWND, dc: HDC) {
    unsafe {
        let handle = send(hwnd, WM_GETFONT, 0, 0) as HFONT;
        SelectObject(
            dc,
            if handle.is_null() {
                GetStockObject(DEFAULT_GUI_FONT)
            } else {
                handle
            },
        );
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(
            dc,
            color_role(if IsWindowEnabled(hwnd) == 0 {
                Role::DisabledText
            } else {
                Role::Text
            }),
        );
    }
}

/// DmlibPaintHelper.h::PaintWithBuffer: clip drawing and blitting to rcPaint.
/// Temporarily lease the cached surface so nested owner drawing cannot overwrite it.
unsafe fn buffered(hwnd: HWND, target: HDC, state: &ControlData, dirty: RECT) {
    unsafe {
        let area = client(hwnd);
        if area.right <= 0
            || area.bottom <= 0
            || dirty.right <= dirty.left
            || dirty.bottom <= dirty.top
        {
            return;
        }
        let mut buffer = state.buffer.borrow_mut().take();
        if buffer
            .as_ref()
            .is_some_and(|b| b.width != area.right || b.height != area.bottom)
        {
            buffer = None;
        }
        let buffer = buffer.or_else(|| Buffer::new(target, area.right, area.bottom));
        let status = state.status(hwnd);
        let spin = matches!(state.kind, Control::Overflow).then(|| state.spin(hwnd));
        if let Some(buffer) = buffer {
            let dc = buffer.dc;
            let saved = SaveDC(dc);
            if saved == 0 {
                return;
            }
            let layout = GetLayout(target);
            if layout != u32::MAX {
                SetLayout(dc, layout);
            }
            IntersectClipRect(dc, dirty.left, dirty.top, dirty.right, dirty.bottom);
            let previous = state.active_dc.replace(dc);
            paint(hwnd, dc, state.kind, status.as_deref(), spin.as_deref());
            state.active_dc.set(previous);
            // Keep the target layout for the copy too (notably RTL status grips).
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
            RestoreDC(dc, saved);
            let mut cached = state.buffer.borrow_mut();
            if cached.is_none() {
                *cached = Some(buffer);
            }
        } else {
            // Unlike upstream's allocation-failure no-op, still produce a usable paint.
            let saved = SaveDC(target);
            if saved != 0 {
                IntersectClipRect(target, dirty.left, dirty.top, dirty.right, dirty.bottom);
                paint(hwnd, target, state.kind, status.as_deref(), spin.as_deref());
                RestoreDC(target, saved);
            }
        }
    }
}

unsafe fn paint(
    hwnd: HWND,
    dc: HDC,
    kind: Control,
    status_resources: Option<&StatusResources>,
    spin: Option<&Theme>,
) {
    unsafe {
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        font(hwnd, dc);
        match kind {
            Control::Tab => tabs(hwnd, dc),
            Control::Status => {
                if let Some(resources) = status_resources {
                    if !resources.font.0.is_null() {
                        SelectObject(dc, resources.font.0);
                    }
                    status(hwnd, dc, resources);
                }
            }
            Control::Overflow => {
                let resources = spin.expect("overflow paint resources");
                render_overflow(
                    hwnd,
                    dc,
                    resources.get(hwnd, w!("SPIN")),
                    overflow_states(hwnd),
                );
            }
        }
        RestoreDC(dc, saved);
    }
}

/// One-pixel outline with an optional solid fill, outlined at `radius`.
pub(super) unsafe fn outline(dc: HDC, area: RECT, edge: Role, fill: Option<Role>, radius: i32) {
    unsafe {
        let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(edge)));
        let old_pen = SelectObject(dc, pen.0);
        let old_brush = SelectObject(
            dc,
            fill.map_or_else(|| GetStockObject(NULL_BRUSH), brush_role),
        );
        if radius > 0 {
            RoundRect(
                dc,
                area.left,
                area.top,
                area.right,
                area.bottom,
                radius,
                radius,
            );
        } else {
            Rectangle(dc, area.left, area.top, area.right, area.bottom);
        }
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
    }
}

pub(super) unsafe fn rounded(dc: HDC, area: RECT, edge: Role, fill: Role, radius: i32) {
    unsafe {
        outline(dc, area, edge, Some(fill), radius);
    }
}

fn cursor(hwnd: HWND) -> POINT {
    unsafe {
        let mut point = POINT {
            x: i32::MIN,
            y: i32::MIN,
        };
        if GetCursorPos(&mut point) != 0 {
            ScreenToClient(hwnd, &mut point);
        }
        point
    }
}

unsafe fn tabs(hwnd: HWND, dc: HDC) {
    unsafe {
        let area = client(hwnd);
        FillRect(dc, &area, brush_role(Role::Dialog));
        let count = send(hwnd, TCM_GETITEMCOUNT, 0, 0).max(0) as usize;
        let selected = send(hwnd, TCM_GETCURSEL, 0, 0);
        let mut selected_rect: RECT = std::mem::zeroed();
        let body_top = if selected >= 0
            && send_mut(hwnd, TCM_GETITEMRECT, selected as usize, &mut selected_rect) != 0
        {
            selected_rect.bottom - 1
        } else {
            let mut body = area;
            send_mut(hwnd, TCM_ADJUSTRECT, 0, &mut body);
            body.top - scale(hwnd, 2)
        };
        let saved = SaveDC(dc);
        if selected >= 0 {
            ExcludeClipRect(
                dc,
                selected_rect.left,
                selected_rect.top,
                selected_rect.right,
                selected_rect.bottom,
            );
        }
        let body = RECT {
            top: body_top,
            ..area
        };
        rounded(dc, body, Role::Edge, Role::Dialog, roundness());
        if saved != 0 {
            RestoreDC(dc, saved);
        }

        // Avoid drawing under the native overflow child, including PRINTCLIENT.
        let child = FindWindowExW(hwnd, ptr::null_mut(), UPDOWN_CLASSW, ptr::null());
        if !child.is_null() && IsWindowVisible(child) != 0 {
            let r = bounds(child, hwnd);
            ExcludeClipRect(dc, r.left, r.top, r.right, r.bottom);
        }
        let point = cursor(hwnd);
        let ui = send(hwnd, WM_QUERYUISTATE, 0, 0) as u32;
        let enabled = IsWindowEnabled(hwnd) != 0;
        let focused = send(hwnd, TCM_GETCURFOCUS, 0, 0);
        let top_padding = scale(hwnd, 2);
        for index in 0..count {
            let mut item_rect: RECT = std::mem::zeroed();
            if send_mut(hwnd, TCM_GETITEMRECT, index, &mut item_rect) == 0 {
                continue;
            }
            let mut label = vec![0u16; 256];
            let mut item = TCITEMW {
                mask: TCIF_TEXT | TCIF_IMAGE,
                pszText: label.as_mut_ptr(),
                cchTextMax: label.len() as i32,
                iImage: -1,
                ..std::mem::zeroed()
            };
            // TCITEM has no length query. Grow until the native label fits.
            loop {
                item.pszText = label.as_mut_ptr();
                item.cchTextMax = label.len() as i32;
                send_mut(hwnd, TCM_GETITEMW, index, &mut item);
                if label[label.len() - 2] == 0 || label.len() >= 65536 {
                    break;
                }
                label.resize(label.len() * 2, 0);
            }
            let active = index as isize == selected;
            let hot = enabled && PtInRect(&item_rect, point) != 0;
            let mut frame = item_rect;
            if active {
                frame.left -= 2;
                frame.bottom += 1;
            } else {
                frame.top += top_padding;
                if index as isize - 1 != selected {
                    frame.left -= 1;
                }
            }
            if send(hwnd, TCM_GETROWCOUNT, 0, 0) == 1 && index + 1 != count {
                frame.right += 1;
            }
            let saved = SaveDC(dc);
            IntersectClipRect(
                dc,
                item_rect.left - 2,
                item_rect.top,
                item_rect.right + 1,
                item_rect.bottom + 1,
            );
            rounded(
                dc,
                frame,
                Role::Edge,
                if active {
                    Role::Dialog
                } else if hot {
                    Role::Hot
                } else {
                    Role::Control
                },
                0,
            );
            if active && windows11() && color_role(Role::Highlight) != color_role(Role::Control) {
                let line = RECT {
                    left: frame.left + 1,
                    top: frame.top + 1,
                    right: frame.right - 1,
                    bottom: frame.top + top_padding + 1,
                };
                FillRect(dc, &line, brush_role(Role::Highlight));
            }
            let mut text_rect = item_rect;
            InflateRect(&mut text_rect, -1, -1);
            text_rect.right += 1;
            if !active {
                OffsetRect(&mut text_rect, 0, 1);
            }
            let images = send(hwnd, TCM_GETIMAGELIST, 0, 0) as HIMAGELIST;
            if item.iImage >= 0 && images != 0 {
                let (mut width, mut height) = (0, 0);
                ImageList_GetIconSize(images, &mut width, &mut height);
                let gap = scale(hwnd, 2);
                ImageList_Draw(
                    images,
                    item.iImage,
                    dc,
                    text_rect.left + gap,
                    text_rect.top + (text_rect.bottom - text_rect.top - height) / 2,
                    ILD_NORMAL,
                );
                text_rect.left += width + gap;
            }
            SetTextColor(
                dc,
                color_role(if !enabled {
                    Role::DisabledText
                } else if active || hot {
                    Role::Text
                } else {
                    Role::DimText
                }),
            );
            DrawTextW(
                dc,
                label.as_ptr(),
                -1,
                &mut text_rect,
                DT_CENTER
                    | DT_VCENTER
                    | DT_SINGLELINE
                    | if ui & UISF_HIDEACCEL != 0 {
                        DT_HIDEPREFIX
                    } else {
                        0
                    },
            );
            if index as isize == focused && GetFocus() == hwnd && ui & UISF_HIDEFOCUS == 0 {
                if windows11() {
                    frame.top += top_padding + 1;
                }
                InflateRect(&mut frame, -2, -1);
                DrawFocusRect(dc, &frame);
            }
            if saved != 0 {
                RestoreDC(dc, saved);
            }
        }
    }
}

/// Native status text uses up to three tab-separated fields (left/center/right).
fn status_fields(text: &[u16], no_tabs: bool) -> Vec<(&[u16], u32)> {
    if no_tabs {
        return vec![(text, DT_LEFT)];
    }
    text.splitn(3, |&ch| ch == b'\t' as u16)
        .zip([DT_LEFT, DT_CENTER, DT_RIGHT])
        .collect()
}

unsafe fn status(hwnd: HWND, dc: HDC, resources: &StatusResources) {
    unsafe {
        let area = client(hwnd);
        FillRect(dc, &area, brush_role(Role::Background));
        let mut borders = [0i32; 3];
        send(hwnd, SB_GETBORDERS, 0, borders.as_mut_ptr() as isize);
        let grip = GetWindowLongW(hwnd, GWL_STYLE) as u32 & SBARS_SIZEGRIP != 0
            && IsZoomed(GetParent(hwnd)) == 0;
        let simple = send(hwnd, SB_ISSIMPLE, 0, 0) != 0;
        let count = if simple {
            1
        } else {
            send(hwnd, SB_GETPARTS, 0, 0).max(1) as usize
        };
        for part in 0..count {
            let index = if simple { SB_SIMPLEID as usize } else { part };
            let mut pane = area;
            if !simple && send_mut(hwnd, SB_GETRECT, index, &mut pane) == 0 {
                continue;
            }
            let mut intersection: RECT = std::mem::zeroed();
            if IntersectRect(&mut intersection, &pane, &area) == 0 {
                continue;
            }
            let info = send(hwnd, SB_GETTEXTLENGTHW, index, 0) as u32;
            let flags = info >> 16;
            // Separators belong between panes; the last pane's right edge is
            // the status bar edge (or the grip), not a pane boundary.
            if !simple && flags & SBT_NOBORDERS == 0 && part + 1 < count {
                let edge = RECT {
                    left: pane.right - borders[2],
                    top: pane.top + 1,
                    right: pane.right - borders[2] + 1,
                    bottom: pane.bottom - 3,
                };
                FillRect(dc, &edge, brush_role(Role::Edge));
            }
            pane.left += borders[2];
            pane.right -= borders[1];
            let saved = SaveDC(dc);
            IntersectClipRect(dc, pane.left, pane.top, pane.right, pane.bottom);
            if flags & SBT_OWNERDRAW != 0 {
                // SB_GETTEXT returns the pointer-sized application data, not text.
                let item_data = send(hwnd, SB_GETTEXTW, index, 0) as usize;
                let id = GetDlgCtrlID(hwnd) as u32;
                let draw = DRAWITEMSTRUCT {
                    CtlType: 0,
                    CtlID: id,
                    itemID: index as u32,
                    itemAction: 0,
                    itemState: 0,
                    hwndItem: hwnd,
                    hDC: dc,
                    rcItem: pane,
                    itemData: item_data,
                };
                send_ref(GetParent(hwnd), WM_DRAWITEM, id as usize, &draw);
            } else {
                let mut text = vec![0u16; (info & 0xffff) as usize + 1];
                send(hwnd, SB_GETTEXTW, index, text.as_mut_ptr() as isize);
                let length = text.iter().position(|&ch| ch == 0).unwrap_or(text.len());
                let icon = send(hwnd, SB_GETICON, index, 0) as HICON;
                if !icon.is_null() {
                    let size = scale(hwnd, 16);
                    DrawIconEx(
                        dc,
                        pane.left,
                        pane.top + (pane.bottom - pane.top - size) / 2,
                        icon,
                        size,
                        size,
                        0,
                        ptr::null_mut(),
                        DI_NORMAL,
                    );
                    pane.left += size + scale(hwnd, 2);
                }
                for (field, align) in status_fields(&text[..length], flags & SBT_NOTABPARSING != 0)
                {
                    let mut text_rect = pane;
                    DrawTextW(
                        dc,
                        field.as_ptr(),
                        field.len() as i32,
                        &mut text_rect,
                        align
                            | DT_SINGLELINE
                            | DT_VCENTER
                            | DT_NOPREFIX
                            | if flags & SBT_RTLREADING != 0 {
                                DT_RTLREADING
                            } else {
                                0
                            },
                    );
                }
            }
            if saved != 0 {
                RestoreDC(dc, saved);
            }
        }
        if grip {
            // Upstream uses the STATUS theme's native SP_GRIPPER, not dots or a
            // hand-drawn approximation. Theme/DPI messages invalidate its cache.
            // Like ThemeData::ensureTheme, retry if the theme was unavailable.
            let theme = resources.theme.get(hwnd, w!("STATUS"));
            if theme != 0 {
                let mut size: SIZE = std::mem::zeroed();
                if GetThemePartSize(theme, dc, SP_GRIPPER, 0, &area, TS_DRAW, &mut size) >= 0 {
                    let r = RECT {
                        left: area.right - size.cx,
                        top: area.bottom - size.cy,
                        ..area
                    };
                    DrawThemeBackground(theme, dc, SP_GRIPPER, 0, &r, ptr::null());
                }
            }
        }
    }
}

fn overflow_rects(hwnd: HWND) -> [RECT; 2] {
    let area = client(hwnd);
    let mut previous = area;
    previous.right -= (area.right - area.left) / 2;
    [
        previous,
        RECT {
            left: previous.right,
            ..area
        },
    ]
}

fn overflow_states(hwnd: HWND) -> [i32; 2] {
    let point = cursor(hwnd);
    overflow_rects(hwnd).map(|r| unsafe {
        if IsWindowEnabled(hwnd) == 0 {
            UPS_DISABLED
        } else if PtInRect(&r, point) != 0 {
            UPS_HOT
        } else {
            UPS_NORMAL
        }
    })
}

// Match paintUpDown: normal -> hot is immediate; transitions away from a
// non-normal state use half the SPIN theme duration. Crossing halves must not
// animate the newly hot half from an unrelated old state.
fn overflow_from_states(old: [i32; 2], new: [i32; 2]) -> [i32; 2] {
    let mut from = old;
    for half in 0..2 {
        if old[half] != UPS_NORMAL && new[half] == UPS_NORMAL {
            from[1 - half] = new[1 - half];
        }
    }
    from
}

unsafe fn paint_overflow(hwnd: HWND, dc: HDC, state: &ControlData, dirty: RECT) {
    unsafe {
        if !state.animates() {
            state.stop(hwnd);
            state.spin_states.set(overflow_states(hwnd));
            buffered(hwnd, dc, state, dirty);
            return;
        }
        if animation(|api| (api.render)(hwnd, dc) != 0) {
            return;
        }
        let resources = state.spin(hwnd);
        let theme = resources.get(hwnd, w!("SPIN"));
        let current = overflow_states(hwnd);
        let old = state.spin_states.get();
        let from = overflow_from_states(old, current);
        let mut params = BP_ANIMATIONPARAMS {
            cbSize: std::mem::size_of::<BP_ANIMATIONPARAMS>() as u32,
            style: BPAS_LINEAR,
            ..std::mem::zeroed()
        };
        for half in 0..2 {
            if current[half] != old[half] && old[half] != UPS_NORMAL {
                GetThemeTransitionDuration(
                    theme,
                    SPNP_UP,
                    old[half],
                    current[half],
                    TMT_TRANSITIONDURATIONS as i32,
                    &mut params.dwDuration,
                );
            }
        }
        params.dwDuration /= 2;
        let mut from_dc = ptr::null_mut();
        let mut to_dc = ptr::null_mut();
        let area = client(hwnd);
        let handle = animation(|api| {
            (api.begin)(
                hwnd,
                dc,
                &area,
                BPBF_COMPATIBLEBITMAP,
                ptr::null(),
                &params,
                &mut from_dc,
                &mut to_dc,
            )
        });
        state.spin_states.set(current);
        if handle != 0 {
            for (surface, states) in [(from_dc, from), (to_dc, current)] {
                if !surface.is_null() {
                    render_overflow(hwnd, surface, theme, states);
                }
            }
            animation(|api| (api.end)(handle, 1));
        } else {
            buffered(hwnd, dc, state, dirty);
        }
    }
}

unsafe fn render_overflow(hwnd: HWND, dc: HDC, theme: HTHEME, states: [i32; 2]) {
    unsafe {
        let saved = SaveDC(dc);
        if saved == 0 {
            return;
        }
        let area = client(hwnd);
        FillRect(dc, &area, brush_role(Role::Dialog));
        font(hwnd, dc);
        let native = windows11();
        if theme != 0 && native {
            // renderUpDown uses the previous-arrow state for both themed parts.
            for (half, rect) in overflow_rects(hwnd).into_iter().enumerate() {
                DrawThemeBackground(
                    theme,
                    dc,
                    if half == 0 {
                        SPNP_DOWNHORZ
                    } else {
                        SPNP_UPHORZ
                    },
                    states[0],
                    &rect,
                    ptr::null(),
                );
            }
            RestoreDC(dc, saved);
            return;
        }
        let disabled = states[0] == UPS_DISABLED;
        for (half, r) in overflow_rects(hwnd).into_iter().enumerate() {
            let hot = states[half] == UPS_HOT;
            rounded(
                dc,
                r,
                edge(disabled, hot),
                if disabled {
                    Role::Dialog
                } else if hot {
                    Role::Hot
                } else {
                    Role::Control
                },
                if windows11() { 5 } else { 0 },
            );
            let color = if disabled {
                Role::DisabledText
            } else if hot {
                Role::Text
            } else {
                Role::DimText
            };
            if theme != 0 {
                let mut size: SIZE = std::mem::zeroed();
                GetThemePartSize(
                    theme,
                    ptr::null_mut(),
                    SPNP_UP,
                    UPS_NORMAL,
                    ptr::null(),
                    TS_TRUE,
                    &mut size,
                );
                let base = scale(GetParent(hwnd), ((size.cy - 1) / 3) + 1) as f32;
                let width = base * 0.5;
                let offset = if half == 0 { 1.0 } else { -1.0 };
                let x = r.left as f32 + ((r.right - r.left) as f32 - width - offset) / 2.0;
                let y = r.top as f32 + ((r.bottom - r.top) as f32 - base) / 2.0;
                let shape = if half == 0 {
                    [(1.0, 0.0), (0.0, 0.5), (1.0, 1.0)]
                } else {
                    [(0.0, 0.0), (1.0, 0.5), (0.0, 1.0)]
                };
                let points = shape.map(|(px, py)| POINT {
                    x: (px * width + x) as i32,
                    y: (py * base + y) as i32,
                });
                let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(color)));
                let old_pen = SelectObject(dc, pen.0);
                let old_brush = SelectObject(dc, brush_role(color));
                Polygon(dc, points.as_ptr(), points.len() as i32);
                SelectObject(dc, old_brush);
                SelectObject(dc, old_pen);
            } else {
                let mut text = RECT {
                    left: r.left + half as i32,
                    bottom: r.bottom - 1,
                    ..r
                };
                SetTextColor(dc, color_role(color));
                DrawTextW(
                    dc,
                    if half == 0 { w!("<") } else { w!(">") },
                    -1,
                    &mut text,
                    DT_NOPREFIX | DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP,
                );
            }
        }
        RestoreDC(dc, saved);
    }
}

// NMTBCUSTOMDRAW retains this pen after the notification returns. Keep borrowed
// palette pens alive across nested notifications and theme switches, like brushes.
thread_local! {
    static PENS: RefCell<Vec<(COLORREF, GdiObject)>> = const { RefCell::new(Vec::new()) };
}

fn toolbar_pen() -> HPEN {
    let color = color_role(Role::Edge);
    PENS.with(|pens| {
        let mut pens = pens.borrow_mut();
        if let Some((_, pen)) = pens.iter().find(|(rgb, _)| *rgb == color) {
            return pen.0;
        }
        let handle = unsafe { CreatePen(PS_SOLID, 1, color) };
        if !handle.is_null() {
            pens.push((color, GdiObject(handle)));
        }
        handle
    })
}

unsafe fn button_info(hwnd: HWND, id: usize) -> TBBUTTONINFOW {
    let mut info = unsafe {
        TBBUTTONINFOW {
            cbSize: std::mem::size_of::<TBBUTTONINFOW>() as u32,
            dwMask: TBIF_IMAGE | TBIF_STYLE,
            ..std::mem::zeroed()
        }
    };
    send_mut(hwnd, TB_GETBUTTONINFOW, id, &mut info);
    info
}

unsafe fn dropdown(hwnd: HWND, id: usize, info: &TBBUTTONINFOW) -> Option<RECT> {
    unsafe {
        if info.iImage == I_IMAGENONE || info.fsStyle as u32 & BTNS_DROPDOWN == 0 {
            return None;
        }
        let index = send(hwnd, TB_COMMANDTOINDEX, id, 0);
        let mut r: RECT = std::mem::zeroed();
        if index >= 0
            && send_mut(hwnd, TB_GETITEMDROPDOWNRECT, index as usize, &mut r) != 0
            && r.right > r.left
        {
            Some(r)
        } else {
            None
        }
    }
}

/// Handle the toolbar's NM_CUSTOMDRAW after the owner has had first refusal.
/// Native text, images, disabled rendering and pressed offsets remain native.
pub(super) fn toolbar(draw: &mut NMTBCUSTOMDRAW) -> isize {
    if !is_dark() {
        return CDRF_DODEFAULT as isize;
    }
    unsafe {
        let hwnd = draw.nmcd.hdr.hwndFrom;
        let dc = draw.nmcd.hdc;
        match draw.nmcd.dwDrawStage {
            CDDS_PREPAINT => {
                FillRect(dc, &draw.nmcd.rc, brush_role(Role::Dialog));
                (CDRF_NOTIFYITEMDRAW | CDRF_NOTIFYPOSTPAINT) as isize
            }
            CDDS_ITEMPREPAINT => {
                draw.hbrMonoDither = brush_role(Role::Background);
                draw.hbrLines = brush_role(Role::Edge);
                draw.hpenLines = toolbar_pen();
                let disabled = draw.nmcd.uItemState & CDIS_DISABLED != 0;
                draw.clrText = color_role(if disabled {
                    Role::DisabledText
                } else {
                    Role::DimText
                });
                draw.clrTextHighlight = color_role(if disabled {
                    Role::DisabledText
                } else {
                    Role::Text
                });
                draw.clrBtnFace = color_role(Role::Background);
                draw.clrBtnHighlight = color_role(Role::Control);
                draw.clrHighlightHotTrack = color_role(Role::Hot);
                draw.nStringBkMode = TRANSPARENT as i32;
                draw.nHLStringBkMode = TRANSPARENT as i32;
                let hot = !disabled && draw.nmcd.uItemState & CDIS_HOT != 0;
                let checked = draw.nmcd.uItemState & CDIS_CHECKED != 0;
                let info = button_info(hwnd, draw.nmcd.dwItemSpec);
                let drop = dropdown(hwnd, draw.nmcd.dwItemSpec, &info);
                let mut r = draw.nmcd.rc;
                if let Some(drop) = drop {
                    r.right = drop.left;
                }
                if hot || checked {
                    let fill = if hot { Role::Hot } else { Role::Control };
                    let saved = SaveDC(dc);
                    if info.iImage == I_IMAGENONE {
                        FillRect(dc, &r, brush_role(fill));
                    } else {
                        let radius = if windows11() { 5 } else { 0 };
                        rounded(dc, r, edge(false, hot), fill, radius);
                        if let Some(drop) = drop {
                            rounded(dc, drop, edge(false, hot), fill, radius);
                        }
                    }
                    if saved != 0 {
                        RestoreDC(dc, saved);
                    }
                    draw.nmcd.uItemState &= !CDIS_CHECKED;
                    if hot {
                        draw.nmcd.uItemState &= !CDIS_HOT;
                    }
                }
                let mut result = TBCDRF_USECDCOLORS;
                if draw.nmcd.uItemState & CDIS_SELECTED != 0 {
                    result |= TBCDRF_NOBACKGROUND;
                }
                if drop.is_some() {
                    result |= CDRF_NOTIFYPOSTPAINT;
                }
                result as isize
            }
            CDDS_ITEMPOSTPAINT => {
                let info = button_info(hwnd, draw.nmcd.dwItemSpec);
                if let Some(mut r) = dropdown(hwnd, draw.nmcd.dwItemSpec, &info) {
                    let saved = SaveDC(dc);
                    font(hwnd, dc);
                    SetTextColor(
                        dc,
                        color_role(if draw.nmcd.uItemState & CDIS_DISABLED != 0 {
                            Role::DisabledText
                        } else {
                            Role::Text
                        }),
                    );
                    r.left += 1;
                    r.bottom -= scale(hwnd, 3);
                    DrawTextW(
                        dc,
                        w!("\u{23f7}"),
                        -1,
                        &mut r,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP | DT_NOPREFIX,
                    );
                    if saved != 0 {
                        RestoreDC(dc, saved);
                    }
                }
                CDRF_DODEFAULT as isize
            }
            _ => CDRF_DODEFAULT as isize,
        }
    }
}

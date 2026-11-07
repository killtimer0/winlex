// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 ozone10
// Rust adaptation of win32-darkmodelib, revision e8ec95b:
// DmlibSubclassControl.cpp (tabs, status bar, up-down) and
// DmlibSubclassWindow.cpp (toolbar custom draw), with BufferData/ThemeData
// from DmlibSubclass.h and PaintWithBuffer from DmlibPaintHelper.h.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::animation::{animation, stop_animation};
use super::{
    Role, brush_role, color_role, is_dark, native_dark_theme, prefer_native_tabs, prefer_theme,
    windows11,
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
enum Control {
    Tab = 1,
    Status,
    Overflow,
}

struct ControlData {
    kind: Control,
    buffer: RefCell<Option<Buffer>>,
    active_dc: Cell<HDC>,
    status: RefCell<Option<Rc<StatusResources>>>,
    spin: RefCell<Option<Rc<SpinResources>>>,
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

    fn spin(&self, hwnd: HWND) -> Rc<SpinResources> {
        if self.spin.borrow().is_none() {
            *self.spin.borrow_mut() = Some(Rc::new(SpinResources(Cell::new(0))));
        }
        let resources = self.spin.borrow().as_ref().unwrap().clone();
        resources.theme(hwnd);
        resources
    }

    fn stop(&self, hwnd: HWND) {
        if matches!(self.kind, Control::Overflow) {
            stop_animation(hwnd);
            self.spin_states.set([UPS_NORMAL, UPS_NORMAL]);
        }
    }

    fn animates(&self) -> bool {
        let mut enabled = 0i32;
        self.animated
            && unsafe {
                SystemParametersInfoW(
                    SPI_GETCLIENTAREAANIMATION,
                    0,
                    (&mut enabled as *mut i32).cast(),
                    0,
                ) != 0
            }
            && enabled != 0
    }
}

impl Drop for ControlData {
    fn drop(&mut self) {
        if self.animated {
            animation(|api| unsafe { (api.uninit)() });
        }
    }
}

struct SpinResources(Cell<HTHEME>);

impl SpinResources {
    fn theme(&self, hwnd: HWND) -> HTHEME {
        if self.0.get() == 0 {
            self.0.set(unsafe { OpenThemeData(hwnd, w!("SPIN")) });
        }
        self.0.get()
    }
}

impl Drop for SpinResources {
    fn drop(&mut self) {
        if self.0.get() != 0 {
            unsafe { CloseThemeData(self.0.get()) };
        }
    }
}

// DmlibSubclass.h::BufferData: retain a compatible surface until its size changes.
struct Buffer {
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    width: i32,
    height: i32,
}

impl Buffer {
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

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

struct StatusResources {
    theme: Cell<HTHEME>,
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
                theme: Cell::new(OpenThemeData(hwnd, w!("STATUS"))),
                font: GdiObject(font),
            }
        }
    }
}

impl Drop for StatusResources {
    fn drop(&mut self) {
        if self.theme.get() != 0 {
            unsafe {
                CloseThemeData(self.theme.get());
            }
        }
    }
}

/// Install once; light mode and unsupported tab arrangements keep native painting.
/// Native dark tabs leave dynamic overflow creation entirely to comctl32.
/// Existing overflow buttons are reapplied separately, like upstream enumeration.
pub(super) fn install(hwnd: HWND, class: &str) {
    let kind = if class.eq_ignore_ascii_case("systabcontrol32") {
        Control::Tab
    } else if class.eq_ignore_ascii_case("msctls_statusbar32") {
        Control::Status
    } else {
        return;
    };
    if !matches!(kind, Control::Tab) || !prefer_native_tabs() {
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
    #[cfg(test)]
    if matches!(kind, Control::Overflow) && tests::CUSTOM_OVERFLOW.with(Cell::get) {
        return true;
    }
    is_dark()
        && match kind {
            Control::Tab => unsafe {
                !prefer_native_tabs()
                    && GetWindowLongW(hwnd, GWL_STYLE) as u32
                        & (TCS_VERTICAL | TCS_BOTTOM | TCS_BUTTONS | TCS_OWNERDRAWFIXED)
                        == 0
            },
            Control::Status => !(prefer_theme() && native_dark_theme()),
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
        if matches!(kind, Control::Tab)
            && !prefer_native_tabs()
            && msg == WM_PARENTNOTIFY
            && w as u16 as u32 == WM_CREATE
        {
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
                    let mut tracking = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tracking);
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
    unsafe { MulDiv(value, GetDpiForWindow(hwnd).max(96) as i32, 96) }
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
    spin: Option<&SpinResources>,
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
                render_overflow(hwnd, dc, resources.theme(hwnd), overflow_states(hwnd));
            }
        }
        RestoreDC(dc, saved);
    }
}

unsafe fn rounded(dc: HDC, area: RECT, edge: Role, fill: Role, radius: i32) {
    unsafe {
        let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(edge)));
        let old_pen = SelectObject(dc, pen.0);
        let old_brush = SelectObject(dc, brush_role(fill));
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
        let pen = GdiObject(CreatePen(PS_SOLID, 1, color_role(Role::Edge)));
        let old_pen = SelectObject(dc, pen.0);
        let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let radius = if windows11() { 4 } else { 0 };
        RoundRect(
            dc,
            body.left,
            body.top,
            body.right,
            body.bottom,
            radius,
            radius,
        );
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
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
            if !simple && flags & SBT_NOBORDERS == 0 && part < count - usize::from(grip) {
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
            if resources.theme.get() == 0 {
                resources.theme.set(OpenThemeData(hwnd, w!("STATUS")));
            }
            let theme = resources.theme.get();
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
        let theme = resources.theme(hwnd);
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
        let preferred = prefer_theme();
        #[cfg(test)]
        let preferred = preferred && !tests::CUSTOM_OVERFLOW.with(Cell::get);
        if theme != 0 && windows11() && preferred {
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
                if disabled {
                    Role::DisabledEdge
                } else if hot {
                    Role::HotEdge
                } else {
                    Role::Edge
                },
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

unsafe fn dropdown(draw: &NMTBCUSTOMDRAW) -> Option<RECT> {
    unsafe {
        let hwnd = draw.nmcd.hdr.hwndFrom;
        let mut info = TBBUTTONINFOW {
            cbSize: std::mem::size_of::<TBBUTTONINFOW>() as u32,
            dwMask: TBIF_IMAGE | TBIF_STYLE,
            ..std::mem::zeroed()
        };
        if send_mut(hwnd, TB_GETBUTTONINFOW, draw.nmcd.dwItemSpec, &mut info) < 0
            || info.iImage == I_IMAGENONE
            || info.fsStyle as u32 & BTNS_DROPDOWN == 0
        {
            return None;
        }
        let index = send(hwnd, TB_COMMANDTOINDEX, draw.nmcd.dwItemSpec, 0);
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
                let drop = dropdown(draw);
                let mut r = draw.nmcd.rc;
                if let Some(drop) = drop {
                    r.right = drop.left;
                }
                if hot || checked {
                    let mut info = TBBUTTONINFOW {
                        cbSize: std::mem::size_of::<TBBUTTONINFOW>() as u32,
                        dwMask: TBIF_IMAGE,
                        ..std::mem::zeroed()
                    };
                    send_mut(hwnd, TB_GETBUTTONINFOW, draw.nmcd.dwItemSpec, &mut info);
                    let fill = if hot { Role::Hot } else { Role::Control };
                    let saved = SaveDC(dc);
                    if info.iImage == I_IMAGENONE {
                        FillRect(dc, &r, brush_role(fill));
                    } else {
                        let radius = if windows11() { 5 } else { 0 };
                        rounded(
                            dc,
                            r,
                            if hot { Role::HotEdge } else { Role::Edge },
                            fill,
                            radius,
                        );
                        if let Some(drop) = drop {
                            rounded(
                                dc,
                                drop,
                                if hot { Role::HotEdge } else { Role::Edge },
                                fill,
                                radius,
                            );
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
                if let Some(mut r) = dropdown(draw) {
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

#[cfg(test)]
mod tests {
    use super::super::animation::{AnimationApis, test_support};
    use super::*;
    use windows_sys::Win32::System::SystemServices::MK_LBUTTON;

    thread_local! {
        pub(super) static CUSTOM_OVERFLOW: Cell<bool> = const { Cell::new(false) };
        static ANIMATION_PROBE: RefCell<Option<AnimationProbe>> = const { RefCell::new(None) };
    }

    struct AnimationProbe {
        real: AnimationApis,
        init: usize,
        uninit: usize,
        stops: usize,
        begins: usize,
        ends: usize,
        frames: usize,
        last_begin: isize,
        last_end: i32,
        pending: Vec<(isize, HWND)>,
        running: Vec<HWND>,
    }

    fn real_animation() -> AnimationApis {
        ANIMATION_PROBE.with(|slot| slot.borrow().as_ref().unwrap().real)
    }

    unsafe extern "system" fn probe_init() -> i32 {
        let result = unsafe { (real_animation().init)() };
        if result >= 0 {
            ANIMATION_PROBE.with(|slot| slot.borrow_mut().as_mut().unwrap().init += 1);
        }
        result
    }

    unsafe extern "system" fn probe_uninit() -> i32 {
        let result = unsafe { (real_animation().uninit)() };
        ANIMATION_PROBE.with(|slot| slot.borrow_mut().as_mut().unwrap().uninit += 1);
        result
    }

    unsafe extern "system" fn probe_stop(hwnd: HWND) -> i32 {
        let result = unsafe { (real_animation().stop)(hwnd) };
        if result >= 0 {
            ANIMATION_PROBE.with(|slot| {
                let mut slot = slot.borrow_mut();
                let probe = slot.as_mut().unwrap();
                probe.stops += 1;
                probe.running.retain(|&window| window != hwnd);
            });
        }
        result
    }

    unsafe extern "system" fn probe_begin(
        hwnd: HWND,
        dc: HDC,
        rect: *const RECT,
        format: BP_BUFFERFORMAT,
        paint: *const BP_PAINTPARAMS,
        animation: *const BP_ANIMATIONPARAMS,
        from: *mut HDC,
        to: *mut HDC,
    ) -> isize {
        // Keep the real OS animation alive until the message under test, without
        // depending on theme duration, elapsed wall time or the user's cursor.
        let mut params = unsafe { *animation };
        params.dwDuration = 60_000;
        let handle =
            unsafe { (real_animation().begin)(hwnd, dc, rect, format, paint, &params, from, to) };
        ANIMATION_PROBE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let probe = slot.as_mut().unwrap();
            probe.last_begin = handle;
            if handle != 0 {
                probe.begins += 1;
                probe.pending.push((handle, hwnd));
            }
        });
        handle
    }

    unsafe extern "system" fn probe_end(handle: isize, update: i32) -> i32 {
        let result = unsafe { (real_animation().end)(handle, update) };
        ANIMATION_PROBE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let probe = slot.as_mut().unwrap();
            probe.ends += 1;
            probe.last_end = result;
            if let Some(index) = probe.pending.iter().position(|&(owned, _)| owned == handle) {
                let (_, hwnd) = probe.pending.remove(index);
                if result >= 0 && update != 0 {
                    probe.running.push(hwnd);
                }
            }
        });
        result
    }

    unsafe extern "system" fn probe_render(hwnd: HWND, dc: HDC) -> i32 {
        let result = unsafe { (real_animation().render)(hwnd, dc) };
        if result != 0 {
            ANIMATION_PROBE.with(|slot| slot.borrow_mut().as_mut().unwrap().frames += 1);
        }
        result
    }

    fn animation_frames() -> usize {
        ANIMATION_PROBE.with(|slot| slot.borrow().as_ref().unwrap().frames)
    }

    unsafe fn pump_animation_timers() {
        // UxTheme only lets RenderAnimation draw inside the WM_PAINT sent by
        // its animation timer. An arbitrary call outside that callback returns
        // FALSE even while the animation is live. Dispatch timers, not queued
        // ordinary paints that would start a new transition.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        while std::time::Instant::now() < deadline {
            let mut message: MSG = unsafe { std::mem::zeroed() };
            for _ in 0..8 {
                if unsafe {
                    PeekMessageW(&mut message, ptr::null_mut(), WM_TIMER, WM_TIMER, PM_REMOVE)
                } == 0
                {
                    break;
                }
                unsafe { DispatchMessageW(&message) };
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn custom_overflow() -> impl Drop {
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                CUSTOM_OVERFLOW.with(|slot| slot.set(self.0));
            }
        }
        Restore(CUSTOM_OVERFLOW.with(|slot| slot.replace(true)))
    }

    fn with_animation_probe(action: impl FnOnce(AnimationApis)) {
        let Some(real) = test_support::real() else {
            eprintln!("SKIPPED real overflow animation: required UxTheme exports unavailable");
            return;
        };
        let _custom = custom_overflow();
        struct Restore {
            probe: Option<AnimationProbe>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                ANIMATION_PROBE.with(|slot| slot.replace(self.probe.take()));
            }
        }
        let probe = AnimationProbe {
            real,
            init: 0,
            uninit: 0,
            stops: 0,
            begins: 0,
            ends: 0,
            frames: 0,
            last_begin: 0,
            last_end: 0,
            pending: Vec::new(),
            running: Vec::new(),
        };
        let _restore = Restore {
            probe: ANIMATION_PROBE.with(|slot| slot.replace(Some(probe))),
        };
        test_support::scoped(
            Some(AnimationApis {
                init: probe_init,
                uninit: probe_uninit,
                stop: probe_stop,
                begin: probe_begin,
                end: probe_end,
                render: probe_render,
            }),
            || action(real),
        );
    }

    struct OverflowWindow {
        parent: HWND,
        tab: HWND,
        hwnd: HWND,
    }

    impl OverflowWindow {
        unsafe fn new() -> Self {
            unsafe {
                let classes = INITCOMMONCONTROLSEX {
                    dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                    dwICC: ICC_TAB_CLASSES | ICC_UPDOWN_CLASS,
                };
                assert_ne!(InitCommonControlsEx(&classes), 0);
                let parent = test_window(
                    w!("STATIC"),
                    ptr::null(),
                    WS_POPUP,
                    RECT {
                        left: 0,
                        top: 0,
                        right: 180,
                        bottom: 100,
                    },
                );
                let tab = child(parent, w!("SysTabControl32"), &[0], WS_TABSTOP, 1).unwrap();
                set_bounds(
                    tab,
                    RECT {
                        left: 0,
                        top: 0,
                        right: 140,
                        bottom: 80,
                    },
                )
                .unwrap();
                for index in 0..12 {
                    let mut text = wide(&format!("Page {index}"));
                    let item = TCITEMW {
                        mask: TCIF_TEXT,
                        pszText: text.as_mut_ptr(),
                        ..std::mem::zeroed()
                    };
                    assert_eq!(send_ref(tab, TCM_INSERTITEMW, index, &item), index as isize);
                }
                let hwnd = FindWindowExW(tab, ptr::null_mut(), UPDOWN_CLASSW, ptr::null());
                assert!(!hwnd.is_null());
                let result = Self { parent, tab, hwnd };
                install_one(hwnd, Control::Overflow);
                ShowWindow(parent, SW_SHOWNOACTIVATE);
                let before = result.native_state();
                let next = overflow_rects(hwnd)[1];
                let point = (((next.top + next.bottom) / 2) << 16) | ((next.left + next.right) / 2);
                send(hwnd, WM_LBUTTONDOWN, MK_LBUTTON as usize, point as isize);
                send(hwnd, WM_LBUTTONUP, 0, point as isize);
                let after = result.native_state();
                assert!(
                    after.2 < before.2,
                    "native arrow input must scroll the tab strip"
                );
                assert_eq!(
                    after.0, before.0,
                    "native scrolling must preserve selection"
                );
                // Disabled state is independent of the physical cursor.
                EnableWindow(hwnd, 0);
                result
            }
        }

        unsafe fn state(&self) -> &ControlData {
            unsafe {
                let mut data = 0;
                assert_ne!(
                    GetWindowSubclass(self.hwnd, Some(subclass), SUBCLASS, &mut data),
                    0
                );
                &*(data as *const ControlData)
            }
        }

        unsafe fn native_state(&self) -> (isize, isize, i32) {
            unsafe {
                let mut item: RECT = std::mem::zeroed();
                assert_ne!(send_mut(self.tab, TCM_GETITEMRECT, 0, &mut item), 0);
                (
                    send(self.tab, TCM_GETCURSEL, 0, 0),
                    send(self.hwnd, UDM_GETPOS32, 0, 0),
                    item.left,
                )
            }
        }
    }

    impl Drop for OverflowWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.parent) };
        }
    }

    struct OverflowSurface {
        screen: HDC,
        buffer: Buffer,
    }

    impl OverflowSurface {
        unsafe fn new(hwnd: HWND) -> Self {
            unsafe {
                let screen = GetDC(ptr::null_mut());
                assert!(!screen.is_null());
                let area = client(hwnd);
                assert!(
                    area.right > 0 && area.bottom > 0,
                    "overflow must have a drawable client area"
                );
                let buffer = Buffer::new(screen, area.right, area.bottom).unwrap();
                Self { screen, buffer }
            }
        }
    }

    impl Drop for OverflowSurface {
        fn drop(&mut self) {
            unsafe { ReleaseDC(ptr::null_mut(), self.screen) };
        }
    }

    unsafe fn assert_bitmap_released(bitmap: HBITMAP) {
        unsafe {
            // Flush batched deletion and query the object, not GetObjectType:
            // the latter can still report the type encoded in a stale handle.
            GdiFlush();
            let mut info: BITMAP = std::mem::zeroed();
            assert_eq!(
                GetObjectW(
                    bitmap,
                    std::mem::size_of::<BITMAP>() as i32,
                    (&mut info as *mut BITMAP).cast()
                ),
                0,
                "destroy must release the cached bitmap"
            );
        }
    }

    unsafe fn cached_overflow_bitmap(hwnd: HWND, state: &ControlData) -> HBITMAP {
        unsafe {
            let bitmap = state.buffer.borrow().as_ref().unwrap().bitmap;
            let mut info: BITMAP = std::mem::zeroed();
            assert_ne!(
                GetObjectW(
                    bitmap,
                    std::mem::size_of::<BITMAP>() as i32,
                    (&mut info as *mut BITMAP).cast()
                ),
                0
            );
            let area = client(hwnd);
            assert!(area.right > 1 && area.bottom > 1);
            assert_eq!(
                (info.bmWidth, info.bmHeight),
                (area.right, area.bottom),
                "the release assertion must cover a real full-size bitmap, not a stock zero-size substitute"
            );
            bitmap
        }
    }

    struct DestroyObservation {
        stopped_before: usize,
        cancelled: Cell<bool>,
    }

    unsafe extern "system" fn observe_overflow_destroy(
        hwnd: HWND,
        message: u32,
        w: usize,
        l: isize,
        _: usize,
        data: usize,
    ) -> isize {
        unsafe {
            let result = default_subclass(hwnd, message, w, l);
            if message == WM_DESTROY {
                let observation = &*(data as *const DestroyObservation);
                // WM_NCDESTROY has not happened yet. OS teardown must not mask
                // failure to explicitly stop animation during WM_DESTROY.
                let stopped = ANIMATION_PROBE.with(|slot| {
                    slot.borrow().as_ref().unwrap().stops > observation.stopped_before
                });
                let frames = animation_frames();
                pump_animation_timers();
                observation
                    .cancelled
                    .set(stopped && animation_frames() == frames);
            }
            result
        }
    }

    unsafe fn assert_overflow_final_pixels(hwnd: HWND, dc: HDC, state: &ControlData) {
        unsafe {
            let expected = OverflowSurface::new(hwnd);
            render_overflow(
                hwnd,
                expected.buffer.dc,
                state.spin(hwnd).theme(hwnd),
                [UPS_DISABLED; 2],
            );
            for half in overflow_rects(hwnd) {
                let mut readable = false;
                for y in half.top..half.bottom {
                    for x in half.left..half.right {
                        let pixel = GetPixel(dc, x, y);
                        assert_eq!(
                            pixel,
                            GetPixel(expected.buffer.dc, x, y),
                            "final overflow pixel ({x}, {y})"
                        );
                        readable |= pixel == color_role(Role::DisabledText);
                    }
                }
                assert!(
                    readable,
                    "both arrow halves must retain readable foreground pixels"
                );
            }
        }
    }

    #[test]
    fn overflow_missing_each_animation_export_paints_and_preserves_native_state() {
        let _activation = super::super::tests::CommonControls::activate();
        let _custom = custom_overflow();
        for missing in test_support::EXPORTS {
            let api = test_support::without(missing);
            assert!(
                api.is_none(),
                "missing {missing:?} must disable the complete API set"
            );
            test_support::scoped(api, || unsafe {
                let window = OverflowWindow::new();
                let surface = OverflowSurface::new(window.hwnd);
                let before = window.native_state();
                let state = window.state();
                assert!(!state.animated);
                let old_font = GetCurrentObject(surface.buffer.dc, OBJ_FONT as u32);
                SetTextColor(surface.buffer.dc, 0x00123456);
                for message in [WM_SIZE, WM_THEMECHANGED, WM_DPICHANGED_AFTERPARENT] {
                    send(window.hwnd, message, 0, 0);
                    PatBlt(
                        surface.buffer.dc,
                        0,
                        0,
                        client(window.hwnd).right,
                        client(window.hwnd).bottom,
                        BLACKNESS,
                    );
                    paint_overflow(window.hwnd, surface.buffer.dc, state, client(window.hwnd));
                    assert_overflow_final_pixels(window.hwnd, surface.buffer.dc, state);
                    assert_eq!(
                        GetCurrentObject(surface.buffer.dc, OBJ_FONT as u32),
                        old_font
                    );
                    assert_eq!(GetTextColor(surface.buffer.dc), 0x00123456);
                    assert_eq!(
                        window.native_state(),
                        before,
                        "painting must not scroll or select tabs"
                    );
                }
                let spin = Rc::downgrade(state.spin.borrow().as_ref().unwrap());
                let bitmap = cached_overflow_bitmap(window.hwnd, state);
                drop(surface);
                drop(window);
                assert!(
                    spin.upgrade().is_none(),
                    "destroy must release theme ownership"
                );
                assert_bitmap_released(bitmap);
            });
        }
    }

    #[test]
    fn overflow_real_animation_is_cancelled_by_resize_theme_dpi_and_destroy() {
        let _activation = super::super::tests::CommonControls::activate();
        let mut enabled = 0i32;
        let preference = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            )
        };
        if preference == 0 || enabled == 0 {
            eprintln!(
                "SKIPPED real overflow animation: client-area animation preference unavailable or disabled"
            );
            return;
        }
        with_animation_probe(|_| unsafe {
            for message in [
                WM_SIZE,
                WM_THEMECHANGED,
                WM_DPICHANGED,
                WM_DPICHANGED_AFTERPARENT,
                WM_DESTROY,
            ] {
                let window = OverflowWindow::new();
                let surface = OverflowSurface::new(window.hwnd);
                let state = window.state();
                assert!(
                    state.animated,
                    "available real buffered-paint API must initialize"
                );
                assert!(state.animates());
                state.stop(window.hwnd);
                // Allocate the same retained resources as ordinary static painting.
                buffered(window.hwnd, surface.buffer.dc, state, client(window.hwnd));
                let before = window.native_state();
                let old_font = GetCurrentObject(surface.buffer.dc, OBJ_FONT as u32);
                SetTextColor(surface.buffer.dc, 0x00123456);
                state.spin_states.set([UPS_HOT, UPS_HOT]);
                let frames = animation_frames();
                assert_ne!(
                    RedrawWindow(
                        window.hwnd,
                        ptr::null(),
                        ptr::null_mut(),
                        RDW_INVALIDATE | RDW_UPDATENOW
                    ),
                    0
                );
                pump_animation_timers();
                ANIMATION_PROBE.with(|slot| {
                    let slot = slot.borrow();
                    let probe = slot.as_ref().unwrap();
                    assert!(probe.frames > frames, "real UxTheme timer must render an intermediate frame before testing message {message:#x}; begin={:#x}, end HRESULT={:#x}, successful begins={}, ends={}", probe.last_begin, probe.last_end, probe.begins, probe.ends);
                });
                assert_eq!(
                    GetCurrentObject(surface.buffer.dc, OBJ_FONT as u32),
                    old_font
                );
                assert_eq!(GetTextColor(surface.buffer.dc), 0x00123456);
                let spin = Rc::downgrade(state.spin.borrow().as_ref().unwrap());
                let stops = ANIMATION_PROBE.with(|slot| slot.borrow().as_ref().unwrap().stops);
                if message == WM_DESTROY {
                    let observation = DestroyObservation {
                        stopped_before: stops,
                        cancelled: Cell::new(false),
                    };
                    assert_ne!(
                        set_subclass(
                            window.hwnd,
                            Some(observe_overflow_destroy),
                            SUBCLASS + 1,
                            &observation as *const _ as usize
                        ),
                        0
                    );
                    let bitmap = cached_overflow_bitmap(window.hwnd, state);
                    drop(window);
                    assert!(
                        observation.cancelled.get(),
                        "WM_DESTROY must stop a live animation before native teardown"
                    );
                    assert!(spin.upgrade().is_none());
                    assert_bitmap_released(bitmap);
                    ANIMATION_PROBE.with(|slot| {
                        let slot = slot.borrow();
                        let probe = slot.as_ref().unwrap();
                        assert_eq!(probe.init, probe.uninit);
                        assert_eq!(probe.begins, probe.ends);
                        assert!(probe.pending.is_empty());
                        assert!(probe.running.is_empty());
                    });
                    continue;
                }
                if message == WM_SIZE {
                    let area = client(window.hwnd);
                    assert_ne!(
                        SetWindowPos(
                            window.hwnd,
                            ptr::null_mut(),
                            0,
                            0,
                            area.right + 2,
                            area.bottom,
                            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
                        ),
                        0
                    );
                } else {
                    send(window.hwnd, message, 0, 0);
                }
                let frames = animation_frames();
                pump_animation_timers();
                assert_eq!(
                    animation_frames(),
                    frames,
                    "message {message:#x} must stop actual OS animation frames"
                );
                ANIMATION_PROBE.with(|slot| {
                    let slot = slot.borrow();
                    let probe = slot.as_ref().unwrap();
                    assert!(
                        probe.stops > stops,
                        "cancellation must call the real stop API"
                    );
                    assert!(
                        probe.pending.is_empty(),
                        "every begun animation must be ended"
                    );
                    assert!(!probe.running.contains(&window.hwnd));
                });
                if matches!(
                    message,
                    WM_THEMECHANGED | WM_DPICHANGED | WM_DPICHANGED_AFTERPARENT
                ) {
                    assert!(
                        spin.upgrade().is_none(),
                        "theme/DPI changes must release old theme ownership"
                    );
                }
                // Static final painting is deliberate: do not start a fresh
                // transition when verifying the result of cancellation.
                let final_surface = OverflowSurface::new(window.hwnd);
                buffered(
                    window.hwnd,
                    final_surface.buffer.dc,
                    state,
                    client(window.hwnd),
                );
                assert_overflow_final_pixels(window.hwnd, final_surface.buffer.dc, state);
                assert_eq!(window.native_state(), before);
                let spin = Rc::downgrade(state.spin.borrow().as_ref().unwrap());
                let bitmap = cached_overflow_bitmap(window.hwnd, state);
                drop(final_surface);
                drop(surface);
                drop(window);
                assert!(spin.upgrade().is_none());
                assert_bitmap_released(bitmap);
                ANIMATION_PROBE.with(|slot| {
                    let slot = slot.borrow();
                    let probe = slot.as_ref().unwrap();
                    assert_eq!(
                        probe.init, probe.uninit,
                        "destroy must balance successful initialization"
                    );
                    assert_eq!(
                        probe.begins, probe.ends,
                        "successful animation begins must release ownership"
                    );
                    assert!(probe.pending.is_empty());
                    assert!(probe.running.is_empty());
                });
            }
        });
    }

    #[test]
    fn overflow_crossing_halves_only_fades_the_departing_arrow() {
        assert_eq!(
            overflow_from_states([UPS_HOT, UPS_NORMAL], [UPS_NORMAL, UPS_HOT]),
            [UPS_HOT, UPS_HOT],
            "the arriving arrow is hot in both transition surfaces"
        );
        assert_eq!(
            overflow_from_states([UPS_NORMAL, UPS_HOT], [UPS_HOT, UPS_NORMAL]),
            [UPS_HOT, UPS_HOT]
        );
        assert_eq!(
            overflow_from_states([UPS_HOT, UPS_NORMAL], [UPS_NORMAL, UPS_NORMAL]),
            [UPS_HOT, UPS_NORMAL],
            "leaving the control fades only the formerly hot arrow"
        );
    }

    #[test]
    fn tab_overflow_created_after_install_keeps_native_scrolling_and_selection() {
        let _activation = super::super::tests::CommonControls::activate();
        unsafe {
            let classes = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TAB_CLASSES | ICC_UPDOWN_CLASS,
            };
            assert_ne!(InitCommonControlsEx(&classes), 0);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 1800,
                    bottom: 200,
                },
            );
            let tab = child(parent, w!("SysTabControl32"), &[0], WS_TABSTOP, 1).unwrap();
            set_bounds(
                tab,
                RECT {
                    left: 0,
                    top: 0,
                    right: 1800,
                    bottom: 150,
                },
            )
            .unwrap();
            install(tab, "systabcontrol32");
            // Create overflow after installation: only fallback tabs detect it.
            assert!(FindWindowExW(tab, ptr::null_mut(), UPDOWN_CLASSW, ptr::null()).is_null());
            for index in 0..12 {
                let mut text = wide(&format!("Page {index}"));
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: text.as_mut_ptr(),
                    ..std::mem::zeroed()
                };
                assert_eq!(send_ref(tab, TCM_INSERTITEMW, index, &item), index as isize);
            }
            set_bounds(
                tab,
                RECT {
                    left: 0,
                    top: 0,
                    right: 140,
                    bottom: 150,
                },
            )
            .unwrap();
            super::super::set_mode(2);
            ShowWindow(parent, SW_SHOW);
            UpdateWindow(tab);
            let overflow = FindWindowExW(tab, ptr::null_mut(), UPDOWN_CLASSW, ptr::null());
            assert!(
                !overflow.is_null(),
                "narrow tabs must create native overflow"
            );
            let mut data = 0;
            assert_eq!(
                GetWindowSubclass(overflow, Some(subclass), SUBCLASS, &mut data) != 0,
                !prefer_native_tabs(),
                "only fallback tabs install a dynamic overflow painter"
            );
            assert_eq!(
                GetWindowSubclass(tab, Some(subclass), SUBCLASS, &mut data) != 0,
                !prefer_native_tabs(),
                "native dark tabs must not install an overflow detector"
            );
            let selection = send(tab, TCM_GETCURSEL, 0, 0);
            let mut before: RECT = std::mem::zeroed();
            assert_ne!(send_mut(tab, TCM_GETITEMRECT, 0, &mut before), 0);
            let next = overflow_rects(overflow)[1];
            let position = (((next.top + next.bottom) / 2) << 16) | ((next.left + next.right) / 2);
            send(
                overflow,
                WM_LBUTTONDOWN,
                MK_LBUTTON as usize,
                position as isize,
            );
            send(overflow, WM_LBUTTONUP, 0, position as isize);
            let mut after: RECT = std::mem::zeroed();
            assert_ne!(send_mut(tab, TCM_GETITEMRECT, 0, &mut after), 0);
            assert!(
                after.left < before.left,
                "native overflow input must scroll the tab strip"
            );
            assert_eq!(
                send(tab, TCM_GETCURSEL, 0, 0),
                selection,
                "scrolling the strip must not select another page"
            );
            let position = send(overflow, UDM_GETPOS32, 0, 0);
            EnableWindow(overflow, 0);
            send(overflow, WM_THEMECHANGED, 0, 0);
            send(overflow, WM_DPICHANGED_AFTERPARENT, 0, 0);
            assert_eq!(IsWindowEnabled(overflow), 0);
            assert_eq!(send(overflow, UDM_GETPOS32, 0, 0), position);
            assert_eq!(send(tab, TCM_GETCURSEL, 0, 0), selection);
            DestroyWindow(parent);
            super::super::set_mode(0);
        }
    }

    struct NativeOverflowWindow {
        parent: HWND,
        mode: u32,
        cursor: POINT,
    }

    impl Drop for NativeOverflowWindow {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.parent);
                SetCursorPos(self.cursor.x, self.cursor.y);
            }
            super::super::set_mode(self.mode);
        }
    }

    unsafe fn settle_native_overflow(parent: HWND) {
        unsafe {
            RedrawWindow(
                parent,
                ptr::null(),
                ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
            // Let native hover transitions finish, with a bounded message pump.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
            while std::time::Instant::now() < deadline {
                let mut message: MSG = std::mem::zeroed();
                for _ in 0..32 {
                    if PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                        break;
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            GdiFlush();
        }
    }

    unsafe fn themed_overflow_pixels(hwnd: HWND, label: &str, dark: bool, contrast: u32) {
        unsafe {
            assert_ne!(
                IsWindowVisible(hwnd),
                0,
                "{label}: overflow must be visible"
            );
            let dc = GetDC(hwnd);
            assert!(!dc.is_null());
            let mut halves = Vec::new();
            for half in overflow_rects(hwnd) {
                let mut colors = std::collections::BTreeMap::<u32, usize>::new();
                // Exclude the outer edge: borders are not the button background.
                for y in half.top + 2..half.bottom - 2 {
                    for x in half.left + 2..half.right - 2 {
                        let pixel = GetPixel(dc, x, y);
                        *colors.entry(pixel).or_default() += 1;
                    }
                }
                halves.push(colors);
            }
            ReleaseDC(hwnd, dc);
            let luma = |pixel: u32| {
                ((pixel & 255) * 299 + ((pixel >> 8) & 255) * 587 + ((pixel >> 16) & 255) * 114)
                    / 1000
            };
            for (half, colors) in halves.iter().enumerate() {
                eprintln!(
                    "{label}: hwnd={hwnd:p} half={half} client={}x{} COLORREF/count={colors:06x?}",
                    client(hwnd).right,
                    client(hwnd).bottom,
                );
                assert!(!colors.contains_key(&CLR_INVALID), "GetPixel failed");
                let count: usize = colors.values().sum();
                assert!(count > 0, "{label}: no visible pixels");
                let (&background, _) = colors.iter().max_by_key(|(_, count)| *count).unwrap();
                let background = luma(background);
                let expected_background: usize = colors
                    .iter()
                    .filter(|(pixel, _)| (luma(**pixel) < 110) == dark)
                    .map(|(_, count)| count)
                    .sum();
                assert!(
                    expected_background * 2 > count,
                    "{label}: half {half} must have a predominantly {} background",
                    if dark { "dark" } else { "light" },
                );
                let arrow: usize = colors
                    .iter()
                    .filter(|(pixel, _)| {
                        if dark {
                            luma(**pixel) >= background + contrast
                        } else {
                            luma(**pixel) + contrast <= background
                        }
                    })
                    .map(|(_, count)| count)
                    .sum();
                assert!(
                    arrow >= 2,
                    "{label}: half {half} needs a readable native arrow"
                );
            }
        }
    }

    fn exercise_preferred_theme_overflow(late: bool) {
        if !native_dark_theme() {
            eprintln!(
                "SKIPPED overflow_native_dark: this Windows build does not support native dark-theme delegation"
            );
            return;
        }
        if super::super::high_contrast() {
            eprintln!("SKIPPED overflow_native_dark: high contrast overrides explicit dark mode");
            return;
        }
        let _activation = super::super::tests::CommonControls::activate();
        assert!(!CUSTOM_OVERFLOW.with(Cell::get));
        unsafe {
            let classes = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_TAB_CLASSES | ICC_UPDOWN_CLASS,
            };
            assert_ne!(InitCommonControlsEx(&classes), 0);
            let mode = super::super::STATE.with(|state| state.borrow().mode);
            let mut cursor = std::mem::zeroed();
            assert_ne!(GetCursorPos(&mut cursor), 0);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 64,
                    top: 64,
                    right: 1900,
                    bottom: 260,
                },
            );
            let _window = NativeOverflowWindow {
                parent,
                mode,
                cursor,
            };
            let tab = child(parent, w!("SysTabControl32"), &[0], WS_TABSTOP, 1).unwrap();
            let resize = |width| {
                set_bounds(
                    tab,
                    RECT {
                        left: 0,
                        top: 0,
                        right: width,
                        bottom: 150,
                    },
                )
                .unwrap();
            };
            resize(if late { 1800 } else { 160 });
            for index in 0..12 {
                let mut text = wide(&format!("Page {index}"));
                let item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: text.as_mut_ptr(),
                    ..std::mem::zeroed()
                };
                assert_eq!(send_ref(tab, TCM_INSERTITEMW, index, &item), index as isize);
            }
            let find = || FindWindowExW(tab, ptr::null_mut(), UPDOWN_CLASSW, ptr::null());
            if late {
                assert!(
                    find().is_null(),
                    "wide tab must start without an overflow child"
                );
            } else {
                assert!(
                    !find().is_null(),
                    "overflow must precede theme installation"
                );
            }
            super::super::set_mode(2);
            super::super::apply(parent);
            assert!(is_dark());
            if late {
                resize(160);
                let mut data = 0;
                assert_eq!(
                    GetWindowSubclass(find(), Some(subclass), SUBCLASS, &mut data),
                    0,
                    "preferred-theme tabs leave newly created overflow native"
                );
            }
            let overflow = find();
            assert!(!overflow.is_null());
            assert_ne!(
                SetWindowPos(parent, HWND_TOPMOST, 64, 64, 1840, 200, SWP_NOACTIVATE),
                0
            );
            ShowWindow(parent, SW_SHOWNOACTIVATE);
            SetCursorPos(32, 32);
            settle_native_overflow(parent);
            themed_overflow_pixels(overflow, "dark normal", true, 45);

            if late {
                // Verify late native creation before reapplication, then check
                // the separate upstream existing-child enumeration path.
                super::super::apply(parent);
            }
            let mut data = 0;
            assert_ne!(
                GetWindowSubclass(overflow, Some(subclass), SUBCLASS, &mut data),
                0,
                "theme application must include existing overflow"
            );
            assert!(custom(overflow, Control::Overflow));

            let selection = send(tab, TCM_GETCURSEL, 0, 0);
            let mut before: RECT = std::mem::zeroed();
            assert_ne!(send_mut(tab, TCM_GETITEMRECT, 0, &mut before), 0);
            let half = overflow_rects(overflow)[1];
            let x = (half.left + half.right) / 2;
            let y = (half.top + half.bottom) / 2;
            let position = (y << 16) | x;
            let mut screen = POINT { x, y };
            assert_ne!(ClientToScreen(overflow, &mut screen), 0);
            assert_ne!(SetCursorPos(screen.x, screen.y), 0);
            settle_native_overflow(parent);
            themed_overflow_pixels(overflow, "dark hot", true, 45);
            send(
                overflow,
                WM_LBUTTONDOWN,
                MK_LBUTTON as usize,
                position as isize,
            );
            send(overflow, WM_LBUTTONUP, 0, position as isize);
            let mut after: RECT = std::mem::zeroed();
            assert_ne!(send_mut(tab, TCM_GETITEMRECT, 0, &mut after), 0);
            assert!(
                after.left < before.left,
                "native mouse input must scroll the strip"
            );
            assert_eq!(send(tab, TCM_GETCURSEL, 0, 0), selection);

            SetCursorPos(32, 32);
            let position = send(overflow, UDM_GETPOS32, 0, 0);
            EnableWindow(overflow, 0);
            settle_native_overflow(parent);
            themed_overflow_pixels(overflow, "dark disabled", true, 20);
            send(overflow, WM_THEMECHANGED, 0, 0);
            assert_eq!(IsWindowEnabled(overflow), 0);
            assert_eq!(send(overflow, UDM_GETPOS32, 0, 0), position);
            EnableWindow(overflow, 1);

            for (mode, dark) in [(1, false), (2, true)] {
                super::super::set_mode(mode);
                assert_eq!(
                    find(),
                    overflow,
                    "theme round trip must retain the native HWND"
                );
                settle_native_overflow(parent);
                themed_overflow_pixels(overflow, "theme round trip", dark, 45);
                assert_eq!(send(tab, TCM_GETCURSEL, 0, 0), selection);
            }
            resize(1800);
            settle_native_overflow(parent);
            let wide_child = find();
            assert!(wide_child.is_null() || IsWindowVisible(wide_child) == 0);
            resize(160);
            settle_native_overflow(parent);
            let resized = find();
            assert!(!resized.is_null());
            eprintln!(
                "overflow_native_dark late={late}: initial={overflow:p}, wide={wide_child:p}, narrowed={resized:p}"
            );
            themed_overflow_pixels(resized, "dark after widen/narrow", true, 45);
            assert_eq!(send(tab, TCM_GETCURSEL, 0, 0), selection);
        }
    }

    #[test]
    #[ignore = "requires exclusive active desktop for native pointer-state pixels"]
    fn overflow_preferred_theme_existing_child_pixels_input_and_theme_roundtrip() {
        exercise_preferred_theme_overflow(false);
    }

    #[test]
    #[ignore = "requires exclusive active desktop for native pointer-state pixels"]
    fn overflow_preferred_theme_late_child_pixels_input_and_theme_roundtrip() {
        exercise_preferred_theme_overflow(true);
    }

    #[test]
    fn overflow_without_a_spin_theme_draws_readable_font_arrows_and_preserves_dc() {
        unsafe {
            let hwnd = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 80,
                    bottom: 24,
                },
            );
            let screen = GetDC(hwnd);
            let surface = Buffer::new(screen, 80, 24).unwrap();
            let previous = GetCurrentObject(surface.dc, OBJ_FONT as u32);
            SetTextColor(surface.dc, 0x00123456);
            render_overflow(hwnd, surface.dc, 0, [UPS_DISABLED, UPS_DISABLED]);
            for half in overflow_rects(hwnd) {
                let mut glyph = false;
                for y in 4..20 {
                    for x in (half.left + 8)..(half.right - 8) {
                        glyph |= GetPixel(surface.dc, x, y) == color_role(Role::DisabledText);
                    }
                }
                assert!(
                    glyph,
                    "each no-theme arrow must have readable foreground pixels"
                );
            }
            assert_eq!(GetCurrentObject(surface.dc, OBJ_FONT as u32), previous);
            assert_eq!(GetTextColor(surface.dc), 0x00123456);
            ReleaseDC(hwnd, screen);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn check_toolbar_notifications_use_palette_without_changing_native_check_state() {
        if !super::super::supported() || high_contrast() {
            return;
        }
        let _activation = super::super::tests::CommonControls::activate();
        unsafe {
            let classes = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&classes), 0);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 40,
                },
            );
            let control = child(parent, w!("ToolbarWindow32"), &[0], TBSTYLE_FLAT, 7).unwrap();
            send(
                control,
                TB_BUTTONSTRUCTSIZE,
                std::mem::size_of::<TBBUTTON>(),
                0,
            );
            let button = TBBUTTON {
                iBitmap: 0,
                idCommand: 42,
                fsState: TBSTATE_ENABLED as u8,
                fsStyle: BTNS_CHECK as u8,
                ..std::mem::zeroed()
            };
            assert_ne!(send_ref(control, TB_ADDBUTTONSW, 1, &button), 0);
            assert_ne!(send(control, TB_CHECKBUTTON, 42, 1), 0);
            let original_state = send(control, TB_GETSTATE, 42, 0);
            super::super::set_mode(2);
            super::super::apply(parent);
            let screen = GetDC(parent);
            let surface = Buffer::new(screen, 100, 40).unwrap();
            let mut draw: NMTBCUSTOMDRAW = std::mem::zeroed();
            draw.nmcd.hdr = NMHDR {
                hwndFrom: control,
                idFrom: 7,
                code: NM_CUSTOMDRAW,
            };
            draw.nmcd.hdc = surface.dc;
            draw.nmcd.rc = RECT {
                left: 0,
                top: 0,
                right: 40,
                bottom: 30,
            };
            draw.nmcd.dwDrawStage = CDDS_PREPAINT;
            assert_ne!(
                send_mut(parent, WM_NOTIFY, 7, &mut draw) & CDRF_NOTIFYITEMDRAW as isize,
                0
            );
            for (flags, expected) in [
                (CDIS_CHECKED, Role::Control),
                (CDIS_CHECKED | CDIS_HOT, Role::Hot),
            ] {
                draw.nmcd.dwDrawStage = CDDS_ITEMPREPAINT;
                draw.nmcd.dwItemSpec = 42;
                draw.nmcd.uItemState = flags;
                let result = send_mut(parent, WM_NOTIFY, 7, &mut draw);
                assert_ne!(result & TBCDRF_USECDCOLORS as isize, 0);
                assert_eq!(GetPixel(surface.dc, 20, 15), color_role(expected));
                assert_eq!(draw.clrText, color_role(Role::DimText));
                assert_eq!(draw.nStringBkMode, TRANSPARENT as i32);
                assert_eq!(draw.nmcd.uItemState & (CDIS_CHECKED | CDIS_HOT), 0);
                assert_eq!(
                    send(control, TB_GETSTATE, 42, 0),
                    original_state,
                    "custom-draw state masking must not change the actual checked button"
                );
            }
            draw.nmcd.uItemState = CDIS_DISABLED;
            send_mut(parent, WM_NOTIFY, 7, &mut draw);
            assert_eq!(draw.clrText, color_role(Role::DisabledText));
            ReleaseDC(parent, screen);
            DestroyWindow(parent);
            super::super::set_mode(0);
        }
    }

    #[test]
    fn nested_status_owner_draw_uses_an_independent_buffer() {
        struct Observation {
            state: ControlData,
            outer_dc: Cell<HDC>,
            nested: Cell<bool>,
            independent: Cell<bool>,
        }
        unsafe extern "system" fn observe(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _: usize,
            data: usize,
        ) -> isize {
            unsafe {
                if msg == WM_DRAWITEM {
                    let draw = &*(l as *const DRAWITEMSTRUCT);
                    let observation = &*(data as *const Observation);
                    if observation.nested.replace(true) {
                        observation
                            .independent
                            .set(draw.hDC != observation.outer_dc.get());
                    } else {
                        observation.outer_dc.set(draw.hDC);
                        let target = Buffer::new(draw.hDC, 300, 100).unwrap();
                        buffered(
                            draw.hwndItem,
                            target.dc,
                            &observation.state,
                            client(draw.hwndItem),
                        );
                        // A nested paint must restore the outer erase target too.
                        observation.independent.set(
                            observation.independent.get()
                                && observation.state.active_dc.get() == draw.hDC,
                        );
                    }
                    return 1;
                }
                default_subclass(hwnd, msg, w, l)
            }
        }
        unsafe {
            let controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&controls), 0);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 100,
                },
            );
            let status = child(parent, STATUSCLASSNAMEW, &[0], 0, 42).unwrap();
            set_bounds(
                status,
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 24,
                },
            )
            .unwrap();
            let observation = Observation {
                state: ControlData::new(Control::Status),
                outer_dc: Cell::new(ptr::null_mut()),
                nested: Cell::new(false),
                independent: Cell::new(false),
            };
            set_subclass(parent, Some(observe), 1, &observation as *const _ as usize);
            assert_ne!(send(status, SB_SETTEXTW, SBT_OWNERDRAW as usize, 123), 0);
            let dc = GetDC(status);
            buffered(status, dc, &observation.state, client(status));
            assert!(
                observation.nested.get(),
                "owner drawing must still be dispatched"
            );
            assert!(
                observation.independent.get(),
                "nested status paints must not overwrite the outer cached surface"
            );
            ReleaseDC(status, dc);
            DestroyWindow(parent);
        }
    }

    #[test]
    fn cached_buffer_clips_repaint_reuses_surface_and_resizes() {
        unsafe {
            let hwnd = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 40,
                },
            );
            let state = ControlData::new(Control::Overflow);
            let screen = GetDC(hwnd);
            let target = Buffer::new(screen, 160, 80).unwrap();
            let marker = GdiObject(CreateSolidBrush(0x00123456));
            let full = RECT {
                left: 0,
                top: 0,
                right: 160,
                bottom: 80,
            };
            FillRect(target.dc, &full, marker.0);
            let dirty = RECT {
                left: 10,
                top: 10,
                right: 20,
                bottom: 20,
            };
            buffered(hwnd, target.dc, &state, dirty);
            assert_eq!(
                GetPixel(target.dc, 5, 5),
                0x00123456,
                "partial paint must not blit outside rcPaint"
            );
            assert_ne!(
                GetPixel(target.dc, 15, 15),
                0x00123456,
                "partial paint must update rcPaint"
            );
            let (dc, bitmap) = {
                let cached = state.buffer.borrow();
                let cached = cached.as_ref().unwrap();
                FillRect(cached.dc, &full, marker.0);
                (cached.dc, cached.bitmap)
            };
            buffered(hwnd, target.dc, &state, dirty);
            {
                let cached = state.buffer.borrow();
                let cached = cached.as_ref().unwrap();
                assert_eq!(
                    (cached.dc, cached.bitmap),
                    (dc, bitmap),
                    "same-size repaints reuse their GDI surface"
                );
                assert_eq!(
                    GetPixel(cached.dc, 5, 5),
                    0x00123456,
                    "drawing itself must also be clipped to rcPaint"
                );
            }
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                0,
                0,
                140,
                60,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            buffered(hwnd, target.dc, &state, client(hwnd));
            let mut info: BITMAP = std::mem::zeroed();
            let cached = state.buffer.borrow();
            assert_ne!(
                GetObjectW(
                    cached.as_ref().unwrap().bitmap,
                    std::mem::size_of::<BITMAP>() as i32,
                    (&mut info as *mut BITMAP).cast()
                ),
                0
            );
            assert_eq!(
                (info.bmWidth, info.bmHeight),
                (140, 60),
                "resize replaces the cached bitmap with the client size"
            );
            ReleaseDC(hwnd, screen);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn status_theme_and_dpi_messages_retire_cached_resources_without_invalidating_active_paints() {
        unsafe {
            let hwnd = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 100,
                    bottom: 40,
                },
            );
            install_one(hwnd, Control::Status);
            let mut data = 0;
            assert_ne!(
                GetWindowSubclass(hwnd, Some(subclass), SUBCLASS, &mut data),
                0
            );
            let state = &*(data as *const ControlData);
            let first = state.status(hwnd).unwrap();
            assert!(Rc::ptr_eq(&first, &state.status(hwnd).unwrap()));
            send(hwnd, WM_THEMECHANGED, 0, 0);
            let second = state.status(hwnd).unwrap();
            assert!(
                !Rc::ptr_eq(&first, &second),
                "theme changes replace cached STATUS resources"
            );
            send(hwnd, WM_DPICHANGED_AFTERPARENT, 0, 0);
            let third = state.status(hwnd).unwrap();
            assert!(
                !Rc::ptr_eq(&second, &third),
                "DPI changes replace theme and status font together"
            );
            // A paint already using the old font can finish safely after a nested change.
            let mut definition: LOGFONTW = std::mem::zeroed();
            assert_ne!(
                GetObjectW(
                    first.font.0,
                    std::mem::size_of::<LOGFONTW>() as i32,
                    (&mut definition as *mut LOGFONTW).cast()
                ),
                0
            );
            DestroyWindow(hwnd);
            assert_ne!(
                GetObjectW(
                    third.font.0,
                    std::mem::size_of::<LOGFONTW>() as i32,
                    (&mut definition as *mut LOGFONTW).cast()
                ),
                0,
                "active resources outlive WM_NCDESTROY"
            );
        }
    }

    #[test]
    fn status_owner_draw_retains_pointer_sized_data_and_restores_the_dc() {
        unsafe extern "system" fn observe(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _: usize,
            data: usize,
        ) -> isize {
            unsafe {
                if msg == WM_DRAWITEM {
                    let draw = &*(l as *const DRAWITEMSTRUCT);
                    let result = &mut *(data as *mut Option<(usize, u32, u32, bool)>);
                    *result = Some((
                        draw.itemData,
                        draw.itemID,
                        draw.CtlID,
                        draw.rcItem.right > draw.rcItem.left,
                    ));
                    // An owner is allowed to alter its drawing DC.
                    SetTextColor(draw.hDC, 0x00123456);
                    return 1;
                }
                default_subclass(hwnd, msg, w, l)
            }
        }
        unsafe {
            let controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_BAR_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&controls), 0);
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 100,
                },
            );
            let status = child(parent, STATUSCLASSNAMEW, &[0], 0, 42).unwrap();
            set_bounds(
                status,
                RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 24,
                },
            )
            .unwrap();
            let mut observed: Option<(usize, u32, u32, bool)> = None;
            set_subclass(parent, Some(observe), 1, &mut observed as *mut _ as usize);
            let item_data = usize::MAX - 0x1234;
            assert_ne!(
                send(
                    status,
                    SB_SETTEXTW,
                    SBT_OWNERDRAW as usize,
                    item_data as isize
                ),
                0
            );
            let dc = GetDC(status);
            assert!(!dc.is_null());
            let before_font = GetCurrentObject(dc, OBJ_FONT as u32);
            SetTextColor(dc, 0x00654321);
            SetBkMode(dc, OPAQUE as i32);
            let resources = StatusResources::new(status);
            paint(status, dc, Control::Status, Some(&resources), None);
            assert_eq!(observed, Some((item_data, 0, 42, true)));
            assert_eq!(GetTextColor(dc), 0x00654321);
            assert_eq!(GetBkMode(dc), OPAQUE as i32);
            assert_eq!(GetCurrentObject(dc, OBJ_FONT as u32), before_font);
            ReleaseDC(status, dc);
            DestroyWindow(parent);
        }
    }

    #[test]
    fn status_tabs_keep_empty_fields_and_native_alignment() {
        let text = wide("left\t\tright");
        assert_eq!(
            status_fields(&text[..text.len() - 1], false),
            vec![
                (&wide("left")[..4], DT_LEFT),
                (&[][..], DT_CENTER),
                (&wide("right")[..5], DT_RIGHT)
            ]
        );
        assert_eq!(
            status_fields(&text[..text.len() - 1], true),
            vec![(&text[..text.len() - 1], DT_LEFT)]
        );
    }
}

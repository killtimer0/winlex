use crate::{
    dpi::{Dpi, Mode},
    native::*,
};
use std::{cell::Cell, mem::size_of, ptr};

pub mod modal;
pub mod property_sheet;

struct Hook {
    owner: HWND,
    handle: Cell<HHOOK>,
}
thread_local! { static CREATION: Cell<*const Hook> = const { Cell::new(ptr::null()) }; }

pub struct CreationHook {
    state: Box<Hook>,
    previous: *const Hook,
}
impl CreationHook {
    pub fn new(dpi: Dpi, owner: HWND) -> anyhow::Result<Self> {
        let state = Box::new(Hook {
            owner,
            handle: Cell::new(ptr::null_mut()),
        });
        let previous = CREATION.with(|current| current.replace(&*state));
        let hook = Self { state, previous };
        if dpi.mode == Mode::Pmv1 {
            let handle = unsafe {
                SetWindowsHookExW(
                    WH_CALLWNDPROC,
                    Some(creation_hook),
                    ptr::null_mut(),
                    windows_sys::Win32::System::Threading::GetCurrentThreadId(),
                )
            };
            anyhow::ensure!(!handle.is_null(), "Observe dialog creation failed");
            hook.state.handle.set(handle);
        }
        Ok(hook)
    }
}
impl Drop for CreationHook {
    fn drop(&mut self) {
        let handle = self.state.handle.replace(ptr::null_mut());
        if !handle.is_null() {
            unsafe {
                UnhookWindowsHookEx(handle);
            }
        }
        CREATION.with(|current| current.set(self.previous));
    }
}
unsafe extern "system" fn creation_hook(code: i32, w: usize, l: isize) -> isize {
    unsafe {
        if code >= 0 {
            CREATION.with(|current| {
                let Some(state) = current.get().as_ref() else {
                    return;
                };
                if state.handle.get().is_null() {
                    return;
                }
                let message = &*(l as *const CWPSTRUCT);
                if message.message != WM_NCCREATE {
                    return;
                }
                let create = &*(message.lParam as *const CREATESTRUCTW);
                if create.style as u32 & WS_CHILD == 0
                    && create.hwndParent == state.owner
                    && GetClassLongW(message.hwnd, GCW_ATOM) == 32770
                {
                    // PMv1 non-client scaling must be enabled during WM_NCCREATE;
                    // WM_INITDIALOG and PSCB_INITIALIZED are already too late.
                    let handle = state.handle.replace(ptr::null_mut());
                    UnhookWindowsHookEx(handle);
                    Dpi { mode: Mode::Pmv1 }.enable_nonclient(message.hwnd);
                }
            });
        }
        CallNextHookEx(ptr::null_mut(), code, w, l)
    }
}

pub const SIZE_X: u32 = 1;
pub const SIZE_Y: u32 = 2;
pub const MOVE_X: u32 = 4;
pub const MOVE_Y: u32 = 8;
/// Register a control for font scaling without managing its geometry.
pub const FONT_ONLY: u32 = 16;

// Non-client DPI scaling may already have changed the client area while
// PMv1 template metrics still use system DPI. Read the baseline from
// the resource rather than the current client rectangle.
pub fn template_size(template: &[u8]) -> anyhow::Result<SIZE> {
    let extended = template.get(..4) == Some(&[1, 0, 255, 255]);
    let offset = if extended { 22 } else { 14 };
    let bytes = template
        .get(offset..offset + 4)
        .ok_or_else(|| anyhow::anyhow!("Truncated dialog template"))?;
    let size = SIZE {
        cx: i16::from_le_bytes([bytes[0], bytes[1]]) as i32,
        cy: i16::from_le_bytes([bytes[2], bytes[3]]) as i32,
    };
    anyhow::ensure!(size.cx > 0 && size.cy > 0, "Invalid dialog template size");
    Ok(size)
}

fn repaint(hwnd: HWND) {
    unsafe {
        RedrawWindow(
            hwnd,
            ptr::null(),
            ptr::null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
        );
    }
}

struct Control {
    window: HWND,
    units: RECT,
    flags: u32,
}
fn centered(owner: HWND, size: SIZE) -> RECT {
    let area = bounds(owner, ptr::null_mut());
    let mut x = area.left + (area.right - area.left - size.cx) / 2;
    let mut y = area.top + (area.bottom - area.top - size.cy) / 2;
    if let Some(info) = monitor(owner) {
        x = x.clamp(
            info.rcWork.left,
            (info.rcWork.right - size.cx).max(info.rcWork.left),
        );
        y = y.clamp(
            info.rcWork.top,
            (info.rcWork.bottom - size.cy).max(info.rcWork.top),
        );
    }
    crate::layout::rect(x, y, size.cx, size.cy)
}

pub struct Layout {
    window: HWND,
    dpi: Dpi,
    baseline: SIZE,
    initial_dpi: u32,
    units: SIZE,
    outer: SIZE,
    font: LOGFONTW,
    scaled_font: GdiObject,
    font_dpi: u32,
    controls: Vec<Control>,
    minimum: bool,
}
impl Layout {
    pub fn capture(
        window: HWND,
        dpi: Dpi,
        baseline: SIZE,
        rules: &[(i32, u32)],
        excluded: &[HWND],
        minimum: bool,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            baseline.cx > 0 && baseline.cy > 0,
            "Invalid dialog baseline"
        );
        let mut units = RECT {
            left: 0,
            top: 0,
            right: 4,
            bottom: 8,
        };
        let mut font: LOGFONTW = unsafe { std::mem::zeroed() };
        unsafe {
            anyhow::ensure!(
                MapDialogRect(window, &mut units) != 0 && units.right > 0 && units.bottom > 0,
                "Read dialog units failed"
            );
            anyhow::ensure!(
                GetObjectW(
                    send(window, WM_GETFONT, 0, 0) as HFONT,
                    size_of::<LOGFONTW>() as i32,
                    (&mut font as *mut LOGFONTW).cast()
                ) != 0,
                "Read dialog font failed"
            );
            for &(id, flags) in rules {
                let child = GetDlgItem(window, id);
                anyhow::ensure!(
                    !child.is_null()
                        && !excluded.contains(&child)
                        && flags & !(SIZE_X | SIZE_Y | MOVE_X | MOVE_Y | FONT_ONLY) == 0
                        && (flags & FONT_ONLY == 0 || flags == FONT_ONLY)
                        && (flags & (SIZE_X | MOVE_X)).count_ones() <= 1
                        && (flags & (SIZE_Y | MOVE_Y)).count_ones() <= 1,
                    "Invalid dialog layout rule {id}"
                );
            }
        }
        let outer = bounds(window, ptr::null_mut());
        let mut controls = Vec::new();
        unsafe {
            let mut child = GetWindow(window, GW_CHILD);
            while !child.is_null() {
                if !excluded.contains(&child) {
                    let r = bounds(child, window);
                    let flags = rules
                        .iter()
                        .find(|(id, _)| *id == GetDlgCtrlID(child))
                        .map_or(0, |(_, flags)| *flags);
                    controls.push(Control {
                        window: child,
                        flags,
                        units: RECT {
                            left: MulDiv(r.left, 4, units.right),
                            top: MulDiv(r.top, 8, units.bottom),
                            right: MulDiv(r.right - r.left, 4, units.right),
                            bottom: MulDiv(r.bottom - r.top, 8, units.bottom),
                        },
                    });
                }
                child = GetWindow(child, GW_HWNDNEXT);
            }
        }
        Ok(Self {
            window,
            dpi,
            baseline,
            initial_dpi: dpi.system(),
            units: SIZE {
                cx: units.right,
                cy: units.bottom,
            },
            outer: SIZE {
                cx: outer.right - outer.left,
                cy: outer.bottom - outer.top,
            },
            font,
            scaled_font: GdiObject::default(),
            font_dpi: 0,
            controls,
            minimum,
        })
    }

    // All layout baselines use dialog units.
    // PMv1 suggested bounds scale linearly with DPI, so scale the initial
    // pixel metrics by the same ratio rather than using new font metrics.
    // PMv2 dialog sizing follows font-derived DLU metrics; use MapDialogRect.
    fn map(&self, x: i32, y: i32, target: u32) -> SIZE {
        if self.dpi.mode == Mode::Pmv1 {
            let px = ((i64::from(x) * i64::from(self.units.cx) + 2) / 4) as i32;
            let py = ((i64::from(y) * i64::from(self.units.cy) + 4) / 8) as i32;
            SIZE {
                cx: unsafe { MulDiv(px, target as i32, self.initial_dpi as i32) },
                cy: unsafe { MulDiv(py, target as i32, self.initial_dpi as i32) },
            }
        } else {
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: x,
                bottom: y,
            };
            unsafe {
                MapDialogRect(self.window, &mut rect);
            }
            SIZE {
                cx: rect.right,
                cy: rect.bottom,
            }
        }
    }

    /// Expand a prepared template using the same anchors as subsequent resizing.
    pub fn rebase(&mut self, baseline: SIZE) {
        let dx = baseline.cx - self.baseline.cx;
        let dy = baseline.cy - self.baseline.cy;
        for control in &mut self.controls {
            for (flag, value, delta) in [
                (MOVE_X, &mut control.units.left, dx),
                (MOVE_Y, &mut control.units.top, dy),
                (SIZE_X, &mut control.units.right, dx),
                (SIZE_Y, &mut control.units.bottom, dy),
            ] {
                if control.flags & flag != 0 {
                    *value += delta;
                }
            }
        }
        let extra = self.map(dx, dy, self.initial_dpi);
        self.outer.cx += extra.cx;
        self.outer.cy += extra.cy;
        self.baseline = baseline;
    }

    pub fn resize(&self, target: u32) -> anyhow::Result<()> {
        let client = client(self.window);
        let base = self.map(self.baseline.cx, self.baseline.cy, target);
        let mut positions = Vec::with_capacity(self.controls.len());
        for control in &self.controls {
            if control.flags & FONT_ONLY != 0 {
                continue;
            }
            let r = control.units;
            let mut pos = self.map(r.left, r.top, target);
            let mut size = self.map(r.right, r.bottom, target);
            if control.flags & MOVE_X != 0 {
                pos.cx += client.right - base.cx;
            } else if control.flags & SIZE_X != 0 {
                size.cx += client.right - base.cx;
            }
            if control.flags & MOVE_Y != 0 {
                pos.cy += client.bottom - base.cy;
            } else if control.flags & SIZE_Y != 0 {
                size.cy += client.bottom - base.cy;
            }
            positions.push((
                control.window,
                crate::layout::rect(pos.cx, pos.cy, size.cx.max(0), size.cy.max(0)),
            ));
        }
        place_all(&positions)
    }

    pub fn update_dpi(&mut self, target: u32) -> anyhow::Result<()> {
        if self.dpi.mode == Mode::Pmv1 && self.font_dpi != target {
            let mut font = self.font;
            unsafe {
                font.lfHeight = MulDiv(font.lfHeight, target as i32, self.initial_dpi as i32);
                font.lfWidth = MulDiv(font.lfWidth, target as i32, self.initial_dpi as i32);
                let new_font = CreateFontIndirectW(&font);
                if new_font.is_null() {
                    return Err(error("Scale dialog font"));
                }
                for control in &self.controls {
                    send(control.window, WM_SETFONT, new_font as usize, 0);
                }
                self.scaled_font = GdiObject(new_font);
                self.font_dpi = target;
            }
        }
        self.resize(target)
    }

    pub fn initial_size(&self, target: u32) -> anyhow::Result<SIZE> {
        if self.dpi.mode == Mode::Pmv1 {
            return Ok(SIZE {
                cx: unsafe { MulDiv(self.outer.cx, target as i32, self.initial_dpi as i32) },
                cy: unsafe { MulDiv(self.outer.cy, target as i32, self.initial_dpi as i32) },
            });
        }
        let size = self.map(self.baseline.cx, self.baseline.cy, target);
        let mut r = crate::layout::rect(0, 0, size.cx, size.cy);
        unsafe {
            let style = GetWindowLongPtrW(self.window, GWL_STYLE) as u32;
            let ex = GetWindowLongPtrW(self.window, GWL_EXSTYLE) as u32;
            let menu = i32::from(style & WS_CHILD == 0 && !GetMenu(self.window).is_null());
            let result = if self.dpi.per_monitor() {
                windows_sys::Win32::UI::HiDpi::AdjustWindowRectExForDpi(
                    &mut r, style, menu, ex, target,
                )
            } else {
                AdjustWindowRectEx(&mut r, style, menu, ex)
            };
            anyhow::ensure!(result != 0, "Measure dialog frame failed");
            let metric = |index| {
                if self.dpi.per_monitor() {
                    windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(index, target)
                } else {
                    GetSystemMetrics(index)
                }
            };
            if style & WS_VSCROLL != 0 {
                r.right += metric(SM_CXVSCROLL);
            }
            if style & WS_HSCROLL != 0 {
                r.bottom += metric(SM_CYHSCROLL);
            }
        }
        Ok(SIZE {
            cx: r.right - r.left,
            cy: r.bottom - r.top,
        })
    }

    pub unsafe fn message(&self, message: u32, w: usize, l: isize) -> anyhow::Result<bool> {
        let target = self.dpi.window(self.window);
        match message {
            WM_SIZE if w != SIZE_MINIMIZED as usize => {
                self.resize(target)?;
            }
            // A top-level WM_GETMINMAXINFO minimum can block a DPI downscale.
            // Use WM_SIZING for interactive limits; retain child minmax for layouts.
            WM_SIZING if self.minimum => {
                let minimum = self.initial_size(target)?;
                let r = unsafe { &mut *(l as *mut RECT) };
                crate::layout::limit_sizing(r, w as u32, minimum);
                return Ok(true);
            }
            WM_GETMINMAXINFO
                if self.minimum
                    && unsafe { GetWindowLongPtrW(self.window, GWL_STYLE) } as u32 & WS_CHILD
                        != 0 =>
            {
                let size = self.initial_size(target)?;
                unsafe {
                    (*(l as *mut MINMAXINFO)).ptMinTrackSize = POINT {
                        x: size.cx,
                        y: size.cy,
                    };
                }
            }
            _ => {}
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests;

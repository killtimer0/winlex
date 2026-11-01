use super::{dpi, read};
use crate::native::*;
use std::{cell::RefCell, mem::size_of, ptr};

owned_handle! {
    pub(super) struct CreationHook(HHOOK);
    UnhookWindowsHookEx, null = ptr::null_mut()
}
impl CreationHook {
    pub fn new() -> anyhow::Result<Self> {
        if dpi().mode != crate::dpi::Mode::Pmv1 {
            return Ok(Self(ptr::null_mut()));
        }
        let hook = unsafe {
            SetWindowsHookExW(
                WH_CALLWNDPROC,
                Some(creation_hook),
                ptr::null_mut(),
                windows_sys::Win32::System::Threading::GetCurrentThreadId(),
            )
        };
        if hook.is_null() {
            return Err(error("Observe property sheet creation"));
        }
        Ok(Self(hook))
    }
}
unsafe extern "system" fn creation_hook(code: i32, w: usize, l: isize) -> isize {
    unsafe {
        if code >= 0 {
            let message = &*(l as *const CWPSTRUCT);
            if message.message == WM_NCCREATE {
                let create = &*(message.lParam as *const CREATESTRUCTW);
                if create.style as u32 & WS_CHILD == 0 && create.hwndParent == read(|a| a.frame) {
                    dpi().enable_nonclient(message.hwnd);
                }
            }
        }
        CallNextHookEx(ptr::null_mut(), code, w, l)
    }
}

pub(super) struct Scale {
    initial_dpi: u32,
    units: SIZE,
    outer: SIZE,
    font: LOGFONTW,
    scaled_font: GdiObject,
    font_dpi: u32,
    controls: Vec<(HWND, RECT)>,
}
impl Scale {
    pub fn capture(window: HWND, excluded: &[HWND]) -> anyhow::Result<Self> {
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
            if GetObjectW(
                send(window, WM_GETFONT, 0, 0) as HFONT,
                size_of::<LOGFONTW>() as i32,
                (&mut font as *mut LOGFONTW).cast(),
            ) == 0
            {
                return Err(error("Read settings font"));
            }
        }
        // Non-client scaling may already have changed the client size at creation.
        let outer = bounds(window, ptr::null_mut());
        let mut controls = Vec::new();
        unsafe {
            let mut child = GetWindow(window, GW_CHILD);
            while !child.is_null() {
                if excluded.contains(&child) {
                    child = GetWindow(child, GW_HWNDNEXT);
                    continue;
                }
                let r = bounds(child, window);
                controls.push((
                    child,
                    RECT {
                        left: MulDiv(r.left, 4, units.right),
                        top: MulDiv(r.top, 8, units.bottom),
                        right: MulDiv(r.right - r.left, 4, units.right),
                        bottom: MulDiv(r.bottom - r.top, 8, units.bottom),
                    },
                ));
                child = GetWindow(child, GW_HWNDNEXT);
            }
        }
        Ok(Self {
            initial_dpi: dpi().system(),
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
        })
    }
    fn map(&self, x: i32, y: i32, target: u32) -> SIZE {
        let px = ((i64::from(x) * i64::from(self.units.cx) + 2) / 4) as i32;
        let py = ((i64::from(y) * i64::from(self.units.cy) + 4) / 8) as i32;
        SIZE {
            cx: unsafe { MulDiv(px, target as i32, self.initial_dpi as i32) },
            cy: unsafe { MulDiv(py, target as i32, self.initial_dpi as i32) },
        }
    }
    fn apply(&mut self, target: u32) -> anyhow::Result<()> {
        unsafe {
            if self.font_dpi != target {
                let mut font = self.font;
                font.lfHeight = MulDiv(font.lfHeight, target as i32, self.initial_dpi as i32);
                font.lfWidth = MulDiv(font.lfWidth, target as i32, self.initial_dpi as i32);
                let new_font = CreateFontIndirectW(&font);
                if new_font.is_null() {
                    return Err(error("Scale settings font"));
                }
                for &(child, _) in &self.controls {
                    send(child, WM_SETFONT, new_font as usize, 0);
                }
                self.scaled_font = GdiObject(new_font);
                self.font_dpi = target;
            }
            for &(child, r) in &self.controls {
                let position = self.map(r.left, r.top, target);
                let size = self.map(r.right, r.bottom, target);
                set_bounds(
                    child,
                    crate::layout::rect(position.cx, position.cy, size.cx, size.cy),
                )?;
            }
        }
        Ok(())
    }
    pub fn initial_size(&self, target: u32) -> SIZE {
        SIZE {
            cx: unsafe { MulDiv(self.outer.cx, target as i32, self.initial_dpi as i32) },
            cy: unsafe { MulDiv(self.outer.cy, target as i32, self.initial_dpi as i32) },
        }
    }
}

pub(super) fn update(scales: &RefCell<Vec<Scale>>, target: u32) -> anyhow::Result<()> {
    // Layout sends native messages; leave the shared state unborrowed during reentry.
    let mut owned = scales.take();
    let result = owned.iter_mut().try_for_each(|scale| scale.apply(target));
    *scales.borrow_mut() = owned;
    result
}

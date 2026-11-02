use super::{CreationHook, Layout};
use crate::{
    dpi::{Dpi, Mode},
    native::*,
};
use std::{
    cell::{Cell, RefCell},
    ptr,
};

const PREPARE: u32 = WM_APP + 41;

struct Sheet {
    dpi: Dpi,
    owner: HWND,
    baseline: Vec<SIZE>,
    pages: RefCell<Vec<HWND>>,
    layouts: RefCell<Vec<Layout>>,
    prepared: Cell<bool>,
    callback: PFNPROPSHEETCALLBACK,
    failure: RefCell<Option<anyhow::Error>>,
}
impl Sheet {
    fn fail(&self, hwnd: HWND, error: anyhow::Error) {
        *self.failure.borrow_mut() = Some(error);
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }

    fn with_layouts(
        &self,
        action: impl FnOnce(&mut Vec<Layout>) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        // Layout sends native messages; leave the shared state unborrowed during reentry.
        let mut layouts = self.layouts.take();
        let result = action(&mut layouts);
        *self.layouts.borrow_mut() = layouts;
        result
    }

    fn align_pages(&self, hwnd: HWND) -> anyhow::Result<()> {
        let tab = send(hwnd, PSM_GETTABCONTROL, 0, 0) as HWND;
        let mut area = client(tab);
        send(tab, TCM_ADJUSTRECT, 0, &mut area as *mut RECT as isize);
        unsafe {
            MapWindowPoints(tab, hwnd, (&mut area as *mut RECT).cast(), 2);
        }
        let pages = self.pages.borrow().clone();
        place_all(
            &pages
                .into_iter()
                .map(|page| (page, area))
                .collect::<Vec<_>>(),
        )
    }

    fn prepare(&self, hwnd: HWND) -> anyhow::Result<()> {
        let initial = !self.prepared.get();
        let target = self.dpi.window(hwnd);
        let manual = self.dpi.mode == Mode::Pmv1;
        if initial && manual {
            let mut pages = Vec::new();
            let mut layouts = Vec::new();
            for (index, &baseline) in self.baseline.iter().enumerate() {
                let page = send(hwnd, PSM_INDEXTOHWND, index, 0) as HWND;
                anyhow::ensure!(!page.is_null(), "Property sheet page was not created");
                pages.push(page);
                layouts.push(Layout::capture(page, self.dpi, baseline, &[], &[], false)?);
                anyhow::ensure!(
                    unsafe { set_subclass(page, Some(page_proc), 1, 0) } != 0,
                    "Subclass property sheet page failed"
                );
            }
            // The shell template is resized by comctl32 to fit the supplied pages.
            // Capture only after that native layout, not from PSCB_PRECREATE.
            let mut units = crate::layout::rect(0, 0, 4, 8);
            anyhow::ensure!(
                unsafe { MapDialogRect(hwnd, &mut units) } != 0
                    && units.right > 0
                    && units.bottom > 0,
                "Read property sheet units failed"
            );
            let client = client(hwnd);
            let baseline = SIZE {
                cx: unsafe { MulDiv(client.right, 4, units.right) },
                cy: unsafe { MulDiv(client.bottom, 8, units.bottom) },
            };
            layouts.push(Layout::capture(
                hwnd,
                self.dpi,
                baseline,
                &[],
                &pages,
                false,
            )?);
            *self.pages.borrow_mut() = pages;
            *self.layouts.borrow_mut() = layouts;
        }
        self.prepared.set(true);
        // PMv1 needs explicit initial adaptation to the destination DPI.
        if manual {
            self.with_layouts(|layouts| {
                for layout in layouts {
                    layout.update_dpi(target)?;
                }
                Ok(())
            })?;
        }
        if initial {
            let outer = bounds(hwnd, ptr::null_mut());
            let size = if manual {
                self.layouts.borrow().last().unwrap().initial_size(target)?
            } else {
                SIZE {
                    cx: outer.right - outer.left,
                    cy: outer.bottom - outer.top,
                }
            };
            let owner = bounds(self.owner, ptr::null_mut());
            let mut x = owner.left + (owner.right - owner.left - size.cx) / 2;
            let mut y = owner.top + (owner.bottom - owner.top - size.cy) / 2;
            unsafe {
                let monitor = MonitorFromWindow(self.owner, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..std::mem::zeroed()
                };
                if GetMonitorInfoW(monitor, &mut info) != 0 {
                    x = x.clamp(
                        info.rcWork.left,
                        (info.rcWork.right - size.cx).max(info.rcWork.left),
                    );
                    y = y.clamp(
                        info.rcWork.top,
                        (info.rcWork.bottom - size.cy).max(info.rcWork.top),
                    );
                }
            }
            set_bounds(hwnd, crate::layout::rect(x, y, size.cx, size.cy))?;
        }
        if manual {
            self.align_pages(hwnd)?;
            if initial {
                // WM_SETFONT defers repainting until the complete layout is positioned.
                // The sheet may already be visible when this posted preparation runs.
                unsafe {
                    RedrawWindow(
                        hwnd,
                        ptr::null(),
                        ptr::null_mut(),
                        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                    );
                }
            }
        }
        Ok(())
    }
}
thread_local! { static CURRENT: Cell<*const Sheet> = const { Cell::new(ptr::null()) }; }

pub fn show(
    header: &mut PROPSHEETHEADERW_V2,
    dpi: Dpi,
    baseline: Vec<SIZE>,
) -> anyhow::Result<isize> {
    anyhow::ensure!(
        header.dwFlags & PSH_MODELESS == 0 && header.nPages as usize == baseline.len(),
        "Invalid modal property sheet configuration"
    );
    let state = Sheet {
        dpi,
        owner: header.hwndParent,
        baseline,
        pages: RefCell::new(Vec::new()),
        layouts: RefCell::new(Vec::new()),
        prepared: Cell::new(false),
        callback: header.pfnCallback,
        failure: RefCell::new(None),
    };
    let _hook = CreationHook::new(dpi, header.hwndParent)?;
    let previous = CURRENT.with(|current| current.replace(&state));
    let flags = header.dwFlags;
    header.dwFlags |= PSH_USECALLBACK;
    header.pfnCallback = Some(sheet_callback);
    let result = unsafe { PropertySheetW(header) };
    CURRENT.with(|current| current.set(previous));
    header.pfnCallback = state.callback;
    header.dwFlags = flags;
    if let Some(error) = state.failure.into_inner() {
        return Err(error);
    }
    anyhow::ensure!(result != -1, "Create property sheet failed");
    Ok(result)
}

unsafe extern "system" fn sheet_callback(hwnd: HWND, message: u32, l: isize) -> i32 {
    unsafe {
        CURRENT.with(|current| {
            let Some(state) = current.get().as_ref() else {
                return 0;
            };
            let result = state
                .callback
                .map_or(0, |callback| callback(hwnd, message, l));
            if message == PSCB_INITIALIZED {
                if set_subclass(hwnd, Some(sheet_proc), 1, current.get() as usize) == 0 {
                    state.fail(hwnd, error("Subclass property sheet"));
                } else {
                    PostMessageW(hwnd, PREPARE, 0, 0);
                }
            }
            result
        })
    }
}

unsafe extern "system" fn page_proc(
    hwnd: HWND,
    message: u32,
    w: usize,
    l: isize,
    _: usize,
    _: usize,
) -> isize {
    unsafe {
        if message == WM_NOTIFY && (*(l as *const NMHDR)).code == PSN_SETACTIVE {
            PostMessageW(GetParent(hwnd), PREPARE, 0, 0);
        }
        if message == WM_NCDESTROY {
            remove_subclass(hwnd, Some(page_proc), 1);
        }
        default_subclass(hwnd, message, w, l)
    }
}

unsafe extern "system" fn sheet_proc(
    hwnd: HWND,
    message: u32,
    w: usize,
    l: isize,
    _: usize,
    data: usize,
) -> isize {
    unsafe {
        let state = &*(data as *const Sheet);
        if message == WM_NCDESTROY {
            remove_subclass(hwnd, Some(sheet_proc), 1);
        }
        let result = match message {
            PREPARE => state.prepare(hwnd).map(|_| true),
            WM_DPICHANGED if state.dpi.mode == Mode::Pmv1 && state.prepared.get() => {
                let target = (w & 0xffff) as u32;
                // PMv2 prepares child dialogs bottom-up during BEFOREPARENT,
                // then handles the top-level dialog during DPICHANGED.
                // Mirror that order for PMv1: layouts stores pages before the shell;
                // update their fonts and controls before applying the suggested bounds.
                state
                    .with_layouts(|layouts| {
                        for layout in layouts {
                            layout.update_dpi(target)?;
                        }
                        set_bounds(hwnd, *(l as *const RECT))
                    })
                    .and_then(|_| state.align_pages(hwnd))
                    .map(|_| {
                        // Repaint the subtree after final positioning.
                        RedrawWindow(
                            hwnd,
                            ptr::null(),
                            ptr::null_mut(),
                            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                        );
                        true
                    })
            }
            WM_SIZE | WM_SIZING | WM_GETMINMAXINFO if state.prepared.get() => {
                let mut handled = false;
                state
                    .with_layouts(|layouts| {
                        if let Some(layout) = layouts.last() {
                            handled = layout.message(message, w, l)?;
                        }
                        Ok(())
                    })
                    .map(|_| handled)
            }
            _ => Ok(false),
        };
        match result {
            Ok(true) => return if message == WM_SIZING { 1 } else { 0 },
            Err(error) => {
                state.fail(hwnd, error);
                return 0;
            }
            _ => {}
        }
        // Native PMv2 property sheets can accumulate height loss across DPI changes:
        // page and tab adjustments are applied incrementally to the current window size.
        default_subclass(hwnd, message, w, l)
    }
}

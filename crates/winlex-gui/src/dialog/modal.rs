use super::{CreationHook, Layout, template_size};
use crate::{
    dpi::{Dpi, Mode},
    native::*,
};
use std::{cell::RefCell, ptr};

struct Dialog<'a> {
    dpi: Dpi,
    owner: HWND,
    baseline: SIZE,
    rules: &'a [(i32, u32)],
    layout: RefCell<Option<Layout>>,
    callback: &'a dyn Fn(HWND, u32, usize, isize) -> anyhow::Result<()>,
    failure: RefCell<Option<anyhow::Error>>,
    prepare: &'a dyn Fn(HWND, &mut SIZE) -> anyhow::Result<()>,
}

pub fn show(
    owner: HWND,
    dpi: Dpi,
    template: &[u8],
    rules: &[(i32, u32)],
    callback: &dyn Fn(HWND, u32, usize, isize) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    show_prepared(owner, dpi, template, rules, &|_, _| Ok(()), callback)
}

pub fn show_prepared(
    owner: HWND,
    dpi: Dpi,
    template: &[u8],
    rules: &[(i32, u32)],
    prepare: &dyn Fn(HWND, &mut SIZE) -> anyhow::Result<()>,
    callback: &dyn Fn(HWND, u32, usize, isize) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let state = Dialog {
        dpi,
        owner,
        baseline: template_size(template)?,
        rules,
        layout: RefCell::new(None),
        callback,
        failure: RefCell::new(None),
        prepare,
    };
    let _hook = CreationHook::new(dpi, owner)?;
    let result = unsafe {
        DialogBoxIndirectParamW(
            instance(),
            template.as_ptr().cast(),
            owner,
            Some(dialog_proc),
            &state as *const Dialog as isize,
        )
    };
    if let Some(error) = state.failure.into_inner() {
        return Err(error);
    }
    anyhow::ensure!(result != -1, "Create dialog failed");
    Ok(())
}

impl Dialog<'_> {
    fn process(&self, hwnd: HWND, message: u32, w: usize, l: isize) -> anyhow::Result<bool> {
        // Native positioning sends messages synchronously. Keep the shared slot empty
        // during layout so reentrant WM_SIZE cannot borrow or apply a half-updated layout.
        let mut layout = self.layout.take();
        if message == WM_SIZE && layout.is_none() {
            // DPI updates and preparation synchronously resize the window. Let
            // the outer notification finish before callbacks arrange dynamic rows.
            return Ok(false);
        }
        let result = (|| {
            if message == WM_INITDIALOG {
                let mut baseline = self.baseline;
                (self.prepare)(hwnd, &mut baseline)?;
                let mut initial =
                    Layout::capture(hwnd, self.dpi, self.baseline, self.rules, &[], true)?;
                initial.rebase(baseline);
                let target = self.dpi.window(hwnd);
                initial.update_dpi(target)?;
                let size = initial.initial_size(target)?;
                set_bounds(hwnd, super::centered(self.owner, size))?;
                initial.resize(target)?;
                layout = Some(initial);
            } else if let Some(layout) = layout.as_mut() {
                if message == WM_DPICHANGED && self.dpi.mode == Mode::Pmv1 {
                    let target = (w & 0xffff) as u32;
                    layout.update_dpi(target)?;
                    unsafe {
                        set_bounds(hwnd, *(l as *const RECT))?;
                    }
                    layout.resize(target)?;
                } else if unsafe { layout.message(message, w, l)? } {
                    return Ok(true);
                }
            }
            (self.callback)(hwnd, message, w, l)?;
            let initialized = message == WM_INITDIALOG;
            let rescaled = message == WM_DPICHANGED && self.dpi.mode == Mode::Pmv1;
            if initialized {
                crate::theme::apply(hwnd);
            }
            if initialized || rescaled {
                // Font changes intentionally defer repaint until all controls are positioned.
                unsafe {
                    RedrawWindow(
                        hwnd,
                        ptr::null(),
                        ptr::null_mut(),
                        RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                    );
                }
                return Ok(true);
            }
            Ok(false)
        })();
        *self.layout.borrow_mut() = layout;
        result
    }
}

unsafe extern "system" fn dialog_proc(hwnd: HWND, message: u32, w: usize, l: isize) -> isize {
    unsafe {
        if message == WM_INITDIALOG {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, l as _);
        }
        let Some(state) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Dialog).as_ref() else {
            return 0;
        };
        if message == WM_CLOSE
            || (message == WM_COMMAND && matches!(w as u16 as i32, IDOK | IDCANCEL))
        {
            EndDialog(hwnd, IDCANCEL as isize);
            return 1;
        }
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return 0;
        }
        match state.process(hwnd, message, w, l) {
            Ok(handled) => {
                if message == WM_INITDIALOG {
                    let button = GetDlgItem(hwnd, IDOK);
                    focus(if button.is_null() {
                        GetNextDlgTabItem(hwnd, ptr::null_mut(), 0)
                    } else {
                        button
                    });
                    return 0;
                }
                isize::from(handled)
            }
            Err(error) => {
                *state.failure.borrow_mut() = Some(error);
                EndDialog(hwnd, -1);
                1
            }
        }
    }
}

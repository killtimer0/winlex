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
}

pub fn show(
    owner: HWND,
    dpi: Dpi,
    template: &[u8],
    rules: &[(i32, u32)],
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
        let result = (|| {
            if message == WM_INITDIALOG {
                let mut initial =
                    Layout::capture(hwnd, self.dpi, self.baseline, self.rules, &[], true)?;
                let target = self.dpi.window(hwnd);
                initial.update_dpi(target)?;
                let size = initial.initial_size(target)?;
                let owner = bounds(self.owner, ptr::null_mut());
                let mut x = owner.left + (owner.right - owner.left - size.cx) / 2;
                let mut y = owner.top + (owner.bottom - owner.top - size.cy) / 2;
                unsafe {
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..std::mem::zeroed()
                    };
                    if GetMonitorInfoW(
                        MonitorFromWindow(self.owner, MONITOR_DEFAULTTONEAREST),
                        &mut info,
                    ) != 0
                    {
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
            if message == WM_INITDIALOG || (message == WM_DPICHANGED && self.dpi.mode == Mode::Pmv1)
            {
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
                    focus(GetDlgItem(hwnd, IDOK));
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

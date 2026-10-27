//! Window placement and settings commands, using the single process configuration root.
use super::{catalog, dpi, read, write};
use crate::{
    config::{Language, Window},
    layout,
    native::*,
    resources::{self, *},
};
use std::ptr;

fn monitor(hwnd: HWND) -> Option<MONITORINFO> {
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0)
            .then_some(info)
    }
}

pub(super) fn restore_window(hwnd: HWND) {
    let value = dpi().window(hwnd);
    let saved = read(|a| a.config.window);
    let mut bounds = crate::native::bounds(hwnd, ptr::null_mut());
    let (mut width, mut height) = match saved {
        Some(w) => {
            bounds.left = w.x;
            bounds.top = w.y;
            (
                unsafe { MulDiv(w.width, value as i32, w.dpi as i32) },
                unsafe { MulDiv(w.height, value as i32, w.dpi as i32) },
            )
        }
        None => (layout::scale(1000, value), layout::scale(680, value)),
    };
    width = width.max(layout::scale(480, value));
    height = height.max(layout::scale(300, value));
    if let Some(info) = monitor(hwnd) {
        let work = info.rcWork;
        width = width.min(work.right - work.left);
        height = height.min(work.bottom - work.top);
        bounds.left = bounds.left.clamp(work.left, work.right - width);
        bounds.top = bounds.top.clamp(work.top, work.bottom - height);
    }
    unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            bounds.left,
            bounds.top,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub(super) fn save_window(hwnd: HWND) -> Result<(), String> {
    unsafe {
        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement) == 0 {
            return Err(error("Read window placement"));
        }
        let mut r = placement.rcNormalPosition;
        // WINDOWPLACEMENT uses workspace coordinates for ordinary top-level windows.
        if let Some(info) = monitor(hwnd) {
            let x = info.rcWork.left - info.rcMonitor.left;
            let y = info.rcWork.top - info.rcMonitor.top;
            r.left += x;
            r.right += x;
            r.top += y;
            r.bottom += y;
        }
        let window = Window {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            dpi: dpi().window(hwnd),
            maximized: IsZoomed(hwnd) != 0
                || (IsIconic(hwnd) != 0 && placement.flags & WPF_RESTORETOMAXIMIZED != 0),
        };
        read(|a| {
            a.config.save_layout(
                window,
                a.sidebar.width,
                a.sidebar.active,
                a.sidebar.collapsed,
            )
        })
    }
}

pub(super) fn update_language_menu() {
    let (hwnd, language) = read(|a| (a.frame, a.config.language));
    let selected = match language {
        Language::Auto => LANGUAGE_AUTO,
        Language::English => LANGUAGE_ENGLISH,
        Language::Chinese => LANGUAGE_CHINESE,
    };
    unsafe {
        CheckMenuRadioItem(
            GetMenu(hwnd),
            LANGUAGE_AUTO as u32,
            LANGUAGE_CHINESE as u32,
            selected as u32,
            MF_BYCOMMAND,
        );
    }
}
pub(super) fn language(id: u16) {
    let language = match id {
        LANGUAGE_ENGLISH => Language::English,
        LANGUAGE_CHINESE => Language::Chinese,
        _ => Language::Auto,
    };
    match write(|a| a.config.set_language(language)) {
        Ok(()) => {
            update_language_menu();
            catalog::set_status(resources::text(IDS_LANGUAGE_RESTART));
        }
        Err(error) => catalog::set_status(error),
    }
}

//! Small UI helpers. All HWND operations stay on the UI thread.
use std::ptr;
pub use windows_sys::Win32::UI::Shell::{
    DefSubclassProc as default_subclass, RemoveWindowSubclass as remove_subclass,
    SetWindowSubclass as set_subclass,
};
pub use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, WindowsProgramming::MulDiv},
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
pub use windows_sys::w;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn instance() -> HINSTANCE {
    unsafe { GetModuleHandleW(ptr::null()) }
}
pub fn error(action: &str) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::last_os_error()).context(action.to_owned())
}
pub fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&mut contrast as *mut HIGHCONTRASTW).cast(),
            0,
        ) == 0
            || contrast.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

pub fn child(
    parent: HWND,
    class: *const u16,
    text: &[u16],
    style: u32,
    id: u16,
) -> anyhow::Result<HWND> {
    child_ex(parent, class, text, style, 0, id)
}
pub fn child_ex(
    parent: HWND,
    class: *const u16,
    text: &[u16],
    style: u32,
    ex_style: u32,
    id: u16,
) -> anyhow::Result<HWND> {
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            class,
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            parent,
            id as usize as HMENU,
            instance(),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        let failure = std::io::Error::last_os_error();
        Err(anyhow::Error::new(failure).context(format!("Create child control {id}")))
    } else {
        Ok(hwnd)
    }
}
pub fn client(hwnd: HWND) -> RECT {
    let mut r = unsafe { std::mem::zeroed() };
    unsafe {
        GetClientRect(hwnd, &mut r);
    }
    r
}
pub fn bounds(hwnd: HWND, parent: HWND) -> RECT {
    let mut r = unsafe { std::mem::zeroed() };
    unsafe {
        GetWindowRect(hwnd, &mut r);
        MapWindowPoints(ptr::null_mut(), parent, (&mut r as *mut RECT).cast(), 2);
    }
    r
}
pub fn place(hwnd: HWND, r: RECT) {
    place_with_flags(hwnd, r, 0);
}
pub fn place_with_flags(hwnd: HWND, r: RECT, flags: SET_WINDOW_POS_FLAGS) {
    if hwnd.is_null() {
        return;
    }
    let old = bounds(hwnd, unsafe { GetParent(hwnd) });
    if (old.left, old.top, old.right, old.bottom) == (r.left, r.top, r.right, r.bottom) {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            r.left,
            r.top,
            (r.right - r.left).max(0),
            (r.bottom - r.top).max(0),
            SWP_NOZORDER | SWP_NOACTIVATE | flags,
        );
    }
}
pub fn text(hwnd: HWND) -> String {
    let value = text_wide(hwnd);
    String::from_utf16_lossy(&value[..value.len() - 1])
}
pub fn text_wide(hwnd: HWND) -> Vec<u16> {
    unsafe {
        let mut buf = vec![0; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        buf.truncate(n.max(0) as usize + 1);
        buf
    }
}
pub fn set_text(hwnd: HWND, text: &str) {
    unsafe {
        SetWindowTextW(hwnd, wide(text).as_ptr());
    }
}
pub fn set_status_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    send(hwnd, SB_SETTEXTW, 0, text.as_ptr() as isize);
    send(hwnd, SB_SETTIPTEXTW, 0, text.as_ptr() as isize);
}
pub fn copy_unicode(hwnd: HWND, text: &[u16]) -> anyhow::Result<()> {
    use windows_sys::Win32::System::{DataExchange::*, Memory::*};
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return Err(error("Open clipboard"));
        }
        let result = (|| {
            let memory = GlobalAlloc(GMEM_MOVEABLE, (text.len() + 1) * std::mem::size_of::<u16>());
            if memory.is_null() {
                return Err(error("Allocate clipboard text"));
            }
            let target = GlobalLock(memory).cast::<u16>();
            if target.is_null() {
                let failure = error("Lock clipboard text");
                GlobalFree(memory);
                return Err(failure);
            }
            std::ptr::copy_nonoverlapping(text.as_ptr(), target, text.len());
            *target.add(text.len()) = 0;
            GlobalUnlock(memory);
            // CF_UNICODETEXT = 13. Successful SetClipboardData transfers ownership.
            if EmptyClipboard() == 0 || SetClipboardData(13, memory).is_null() {
                let failure = error("Copy Unicode text");
                GlobalFree(memory);
                return Err(failure);
            }
            Ok(())
        })();
        CloseClipboard();
        result
    }
}
pub fn send(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe { SendMessageW(hwnd, msg, w, l) }
}
pub fn focus(hwnd: HWND) {
    if !hwnd.is_null() {
        unsafe {
            SetFocus(hwnd);
        }
    }
}
pub fn show(hwnd: HWND, visible: bool) {
    unsafe {
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
}

pub fn list_column(hwnd: HWND, index: usize, title: &str, width: i32) {
    let mut title = wide(title);
    let col = LVCOLUMNW {
        mask: LVCF_TEXT | LVCF_WIDTH,
        cx: width,
        pszText: title.as_mut_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    send(hwnd, LVM_INSERTCOLUMNW, index, &col as *const _ as isize);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_creation_preserves_error_before_later_native_calls() {
        let failure = child(
            unsafe { GetDesktopWindow() },
            w!("Winlex.Test.UnregisteredControl"),
            &[0],
            0,
            123,
        )
        .unwrap_err();
        unsafe {
            SetLastError(ERROR_ACCESS_DENIED);
        }
        assert_eq!(
            failure
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .raw_os_error(),
            Some(ERROR_CANNOT_FIND_WND_CLASS as i32)
        );
        assert!(format!("{failure:#}").starts_with("Create child control 123: "));
    }
}

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
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}
pub fn place_all(positions: &[(HWND, RECT)]) -> anyhow::Result<()> {
    let changed: Vec<_> = positions
        .iter()
        .filter(|(hwnd, r)| {
            let old = bounds(*hwnd, unsafe { GetParent(*hwnd) });
            (old.left, old.top, old.right, old.bottom) != (r.left, r.top, r.right, r.bottom)
        })
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    unsafe {
        let mut batch = BeginDeferWindowPos(changed.len() as i32);
        if batch.is_null() {
            return Err(error("Begin layout"));
        }
        for &(hwnd, r) in changed {
            batch = DeferWindowPos(
                batch,
                hwnd,
                ptr::null_mut(),
                r.left,
                r.top,
                (r.right - r.left).max(0),
                (r.bottom - r.top).max(0),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            if batch.is_null() {
                return Err(error("Defer layout"));
            }
        }
        if EndDeferWindowPos(batch) == 0 {
            return Err(error("Commit layout"));
        }
    }
    Ok(())
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
    fn sibling_layout_is_committed_before_resize_notifications() {
        struct Observation {
            sibling: HWND,
            edges: Vec<(i32, i32)>,
        }
        unsafe extern "system" fn observe(
            h: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _: usize,
            data: usize,
        ) -> isize {
            unsafe {
                if msg == WM_SIZE {
                    let observation = &mut *(data as *mut Observation);
                    let parent = GetParent(h);
                    observation.edges.push((
                        bounds(h, parent).right,
                        bounds(observation.sibling, parent).left,
                    ));
                }
                default_subclass(h, msg, w, l)
            }
        }
        unsafe {
            let parent = CreateWindowExW(
                0,
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                0,
                0,
                700,
                300,
                ptr::null_mut(),
                ptr::null_mut(),
                instance(),
                ptr::null(),
            );
            assert!(!parent.is_null());
            let edit = child(parent, w!("EDIT"), &[0], 0, 1).unwrap();
            let sibling = child(parent, w!("STATIC"), &[0], 0, 2).unwrap();
            place(edit, crate::layout::rect(10, 10, 200, 28));
            place(sibling, crate::layout::rect(220, 10, 100, 28));
            let mut observation = Observation {
                sibling,
                edges: vec![],
            };
            assert_ne!(
                set_subclass(edit, Some(observe), 1, &mut observation as *mut _ as usize),
                0
            );
            for width in [400, 150, 500, 200] {
                let positions = [
                    (edit, crate::layout::rect(10, 10, width, 28)),
                    (sibling, crate::layout::rect(width + 20, 10, 100, 28)),
                ];
                place_all(&positions).unwrap();
                let count = observation.edges.len();
                place_all(&positions).unwrap();
                assert_eq!(
                    observation.edges.len(),
                    count,
                    "unchanged layout must not resize controls"
                );
            }
            remove_subclass(edit, Some(observe), 1);
            DestroyWindow(parent);
            assert_eq!(
                observation.edges,
                [(410, 420), (160, 170), (510, 520), (210, 220)]
            );
        }
    }

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

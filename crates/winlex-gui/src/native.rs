//! Small UI helpers. All HWND operations stay on the UI thread.
use std::{mem, ptr};
pub use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, WindowsProgramming::MulDiv},
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn instance() -> HINSTANCE {
    unsafe { GetModuleHandleW(ptr::null()) }
}
pub fn error(action: &str) -> String {
    format!("{action}: {}", std::io::Error::last_os_error())
}

pub unsafe fn procedure<T: Copy>(module: HMODULE, name: &[u8]) -> Option<T> {
    unsafe {
        GetProcAddress(module, name.as_ptr()).map(|p| {
            assert_eq!(mem::size_of::<T>(), mem::size_of_val(&p));
            mem::transmute_copy(&p)
        })
    }
}

pub fn child(parent: HWND, class: &str, text: &str, style: u32, id: u16) -> HWND {
    unsafe {
        CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(text).as_ptr(),
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
pub fn text(hwnd: HWND) -> String {
    unsafe {
        let mut buf = vec![0; GetWindowTextLengthW(hwnd).max(0) as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}
pub fn set_text(hwnd: HWND, text: &str) {
    unsafe {
        SetWindowTextW(hwnd, wide(text).as_ptr());
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
pub fn list_row(hwnd: HWND, index: usize, values: &[&str]) {
    for (column, value) in values.iter().enumerate() {
        let mut value = wide(value);
        let item = LVITEMW {
            mask: LVIF_TEXT,
            iItem: index as i32,
            iSubItem: column as i32,
            pszText: value.as_mut_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        send(
            hwnd,
            if column == 0 {
                LVM_INSERTITEMW
            } else {
                LVM_SETITEMTEXTW
            },
            index,
            &item as *const _ as isize,
        );
    }
}

type SubclassProc =
    Option<unsafe extern "system" fn(HWND, u32, usize, isize, usize, usize) -> isize>;
type SetSubclass = unsafe extern "system" fn(HWND, SubclassProc, usize, usize) -> BOOL;
type RemoveSubclass = unsafe extern "system" fn(HWND, SubclassProc, usize) -> BOOL;
type DefaultSubclass = unsafe extern "system" fn(HWND, u32, usize, isize) -> isize;
struct SubclassApi {
    set: SetSubclass,
    remove: RemoveSubclass,
    default: DefaultSubclass,
}
static SUBCLASS: std::sync::OnceLock<SubclassApi> = std::sync::OnceLock::new();

pub fn initialize_subclass() -> Result<(), String> {
    if SUBCLASS.get().is_some() {
        return Ok(());
    }
    unsafe {
        // XP RTM exports these helpers only by ordinal. Keep the DLL loaded for the callbacks.
        let module = LoadLibraryW(wide("comctl32.dll").as_ptr());
        if module.is_null() {
            return Err(error("Load common controls"));
        }
        let resolve = |ordinal: usize| {
            GetProcAddress(module, ordinal as *const u8)
                .ok_or_else(|| error("Resolve common-control subclass API"))
        };
        let api = SubclassApi {
            set: mem::transmute::<unsafe extern "system" fn() -> isize, SetSubclass>(resolve(410)?),
            remove: mem::transmute::<unsafe extern "system" fn() -> isize, RemoveSubclass>(
                resolve(412)?,
            ),
            default: mem::transmute::<unsafe extern "system" fn() -> isize, DefaultSubclass>(
                resolve(413)?,
            ),
        };
        let _ = SUBCLASS.set(api);
    }
    Ok(())
}
pub unsafe fn set_subclass(hwnd: HWND, callback: SubclassProc, id: usize, data: usize) -> BOOL {
    unsafe { (SUBCLASS.get().unwrap().set)(hwnd, callback, id, data) }
}
pub unsafe fn remove_subclass(hwnd: HWND, callback: SubclassProc, id: usize) {
    unsafe {
        (SUBCLASS.get().unwrap().remove)(hwnd, callback, id);
    }
}
pub unsafe fn default_subclass(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe { (SUBCLASS.get().unwrap().default)(hwnd, msg, w, l) }
}

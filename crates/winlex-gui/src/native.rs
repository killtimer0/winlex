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

macro_rules! owned_handle {
    ($(#[$attr:meta])* $vis:vis struct $name:ident($field_vis:vis $handle:ty);
        $destroy:path $(, null = $null:expr)?
    ) => {
        $(#[$attr])*
        $vis struct $name($field_vis $handle);
        impl Drop for $name {
            #[allow(clippy::cmp_null, reason = "Handles may be pointers or integers.")]
            fn drop(&mut self) {
                $(if self.0 == $null {
                    return;
                })?
                unsafe {
                    $destroy(self.0);
                }
            }
        }
    };
}
pub(crate) use owned_handle;

owned_handle! {
    #[derive(Default)]
    pub struct GdiObject(pub HGDIOBJ); DeleteObject, null = ptr::null_mut()
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn instance() -> HINSTANCE {
    unsafe { GetModuleHandleW(ptr::null()) }
}
pub fn monitor(hwnd: HWND) -> Option<MONITORINFO> {
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0)
            .then_some(info)
    }
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

/// # Safety
/// `class` must point to a terminated UTF-16 class name or a valid class atom.
pub unsafe fn child(
    parent: HWND,
    class: *const u16,
    text: &[u16],
    style: u32,
    id: u16,
) -> anyhow::Result<HWND> {
    unsafe { child_ex(parent, class, text, style, 0, id) }
}
/// # Safety
/// `class` must point to a terminated UTF-16 class name or a valid class atom.
pub unsafe fn child_ex(
    parent: HWND,
    class: *const u16,
    text: &[u16],
    style: u32,
    ex_style: u32,
    id: u16,
) -> anyhow::Result<HWND> {
    anyhow::ensure!(text.contains(&0), "Child text must be NUL-terminated");
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
pub fn tooltip(owner: HWND, tool: &TTTOOLINFOW) -> anyhow::Result<HWND> {
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            ptr::null(),
            WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
            0,
            0,
            0,
            0,
            owner,
            ptr::null_mut(),
            instance(),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err(error("Create tooltip"));
    }
    if send_ref(hwnd, TTM_ADDTOOLW, 0, tool) == 0 {
        unsafe { DestroyWindow(hwnd) };
        anyhow::bail!("Register tooltip failed");
    }
    crate::theme::apply(hwnd);
    Ok(hwnd)
}

pub fn with_window_font<T>(hwnd: HWND, font: HFONT, action: impl FnOnce(HDC) -> T) -> T {
    unsafe {
        let dc = GetDC(hwnd);
        let previous = if dc.is_null() {
            ptr::null_mut()
        } else {
            SelectObject(dc, font)
        };
        let result = action(dc);
        if !dc.is_null() {
            SelectObject(dc, previous);
            ReleaseDC(hwnd, dc);
        }
        result
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
    let _ = set_bounds(hwnd, r);
}
pub fn set_bounds(hwnd: HWND, r: RECT) -> anyhow::Result<()> {
    if unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            r.left,
            r.top,
            (r.right - r.left).max(0),
            (r.bottom - r.top).max(0),
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    } == 0
    {
        return Err(error("Position window"));
    }
    Ok(())
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

pub fn open_url(owner: HWND, target: &str) -> anyhow::Result<()> {
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            owner,
            w!("open"),
            wide(target).as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    anyhow::ensure!(result > 32, "Open online reference failed ({result})");
    Ok(())
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
pub fn send_ref<T>(hwnd: HWND, msg: u32, w: usize, value: &T) -> isize {
    send(hwnd, msg, w, value as *const T as isize)
}
pub fn send_mut<T>(hwnd: HWND, msg: u32, w: usize, value: &mut T) -> isize {
    send(hwnd, msg, w, value as *mut T as isize)
}

owned_handle! { pub struct Popup(HMENU); DestroyMenu }
impl Popup {
    pub fn new() -> anyhow::Result<Self> {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return Err(error("Create popup menu"));
        }
        Ok(Self(menu))
    }
    pub fn item(&self, id: u16, label: &[u16], enabled: bool) -> anyhow::Result<()> {
        anyhow::ensure!(label.contains(&0), "Menu label must be NUL-terminated");
        self.append(
            MF_STRING | if enabled { 0 } else { MF_GRAYED },
            id,
            label.as_ptr(),
        )
    }
    pub fn separator(&self) -> anyhow::Result<()> {
        self.append(MF_SEPARATOR, 0, ptr::null())
    }
    fn append(&self, flags: u32, id: u16, label: *const u16) -> anyhow::Result<()> {
        if unsafe { AppendMenuW(self.0, flags, id as usize, label) } == 0 {
            return Err(error("Append popup item"));
        }
        Ok(())
    }
    pub fn track(&self, owner: HWND, point: POINT) -> u16 {
        unsafe {
            TrackPopupMenu(
                self.0,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                owner,
                ptr::null(),
            ) as u16
        }
    }
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
    send_ref(hwnd, LVM_INSERTCOLUMNW, index, &col);
}

#[cfg(test)]
pub unsafe fn test_window(class: *const u16, title: *const u16, style: u32, area: RECT) -> HWND {
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class,
            title,
            style,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
            ptr::null_mut(),
            ptr::null_mut(),
            instance(),
            ptr::null(),
        )
    };
    assert!(!hwnd.is_null());
    hwnd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_handles_clean_up_on_replacement_and_scope_exit() {
        thread_local! {
            static CLEANED: std::cell::RefCell<Vec<usize>> = const {
                std::cell::RefCell::new(Vec::new())
            };
        }
        unsafe fn destroy_integer(handle: usize) {
            CLEANED.with(|cleaned| cleaned.borrow_mut().push(handle));
        }
        unsafe fn destroy_pointer(handle: *mut u8) {
            unsafe { destroy_integer(handle as usize) };
        }
        owned_handle! { struct Integer(usize); destroy_integer }
        owned_handle! {
            #[derive(Default)]
            struct Pointer(pub *mut u8); destroy_pointer, null = ptr::null_mut()
        }

        {
            let mut integer = Integer(1);
            assert_eq!(integer.0, 1);
            integer = Integer(2);
            assert_eq!(integer.0, 2);
            CLEANED.with(|cleaned| assert_eq!(*cleaned.borrow(), [1]));
            let _zero = Integer(0);
            let _null = Pointer::default();
            let pointer = Pointer(std::ptr::without_provenance_mut(3));
            assert_eq!(pointer.0 as usize, 3);
        }
        CLEANED.with(|cleaned| assert_eq!(*cleaned.borrow(), [1, 3, 0, 2]));
    }

    struct ResourceWindow(HWND);
    impl ResourceWindow {
        fn new() -> Self {
            unsafe {
                let class = WNDCLASSW {
                    style: CS_OWNDC,
                    lpfnWndProc: Some(DefWindowProcW),
                    hInstance: instance(),
                    lpszClassName: w!("Winlex.Test.ResourceWindow"),
                    ..std::mem::zeroed()
                };
                assert!(
                    RegisterClassW(&class) != 0 || GetLastError() == ERROR_CLASS_ALREADY_EXISTS
                );
                let hwnd = test_window(
                    class.lpszClassName,
                    ptr::null(),
                    WS_POPUP,
                    crate::layout::rect(0, 0, 300, 200),
                );
                Self(hwnd)
            }
        }
        fn tooltips(&self) -> Vec<HWND> {
            unsafe extern "system" fn collect(hwnd: HWND, data: isize) -> i32 {
                unsafe {
                    let (owner, windows) = &mut *(data as *mut (HWND, Vec<HWND>));
                    let mut class = [0; 64];
                    let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
                    if GetWindow(hwnd, GW_OWNER) == *owner
                        && class[..length as usize]
                            == "tooltips_class32".encode_utf16().collect::<Vec<_>>()
                    {
                        windows.push(hwnd);
                    }
                    1
                }
            }
            let mut data = (self.0, Vec::new());
            unsafe {
                EnumThreadWindows(
                    windows_sys::Win32::System::Threading::GetCurrentThreadId(),
                    Some(collect),
                    &mut data as *mut _ as isize,
                );
            }
            data.1
        }
    }
    impl Drop for ResourceWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn window_font_restores_selection_and_retains_callback_errors() {
        let window = ResourceWindow::new();
        unsafe {
            let baseline = GetStockObject(SYSTEM_FONT);
            let selected = GetStockObject(DEFAULT_GUI_FONT);
            let dc = GetDC(window.0);
            let original = SelectObject(dc, baseline);
            ReleaseDC(window.0, dc);
            let failure = with_window_font(window.0, selected, |dc| -> anyhow::Result<()> {
                assert_eq!(GetCurrentObject(dc, OBJ_FONT as u32), selected);
                with_window_font(window.0, baseline, |nested| {
                    assert_eq!(GetCurrentObject(nested, OBJ_FONT as u32), baseline);
                });
                assert_eq!(GetCurrentObject(dc, OBJ_FONT as u32), selected);
                SelectObject(dc, GetStockObject(SYSTEM_FIXED_FONT));
                SetLastError(ERROR_ACCESS_DENIED);
                Err(error("Measure test label"))
            })
            .unwrap_err();
            assert_eq!(
                failure
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(ERROR_ACCESS_DENIED as i32)
            );
            assert!(format!("{failure:#}").starts_with("Measure test label: "));
            let dc = GetDC(window.0);
            assert_eq!(GetCurrentObject(dc, OBJ_FONT as u32), baseline);
            SelectObject(dc, original);
            ReleaseDC(window.0, dc);
        }
    }

    #[test]
    fn tooltip_registration_retains_size_and_owner_and_cleans_up_failure() {
        let window = ResourceWindow::new();
        unsafe {
            let controls = INITCOMMONCONTROLSEX {
                dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
                dwICC: ICC_WIN95_CLASSES,
            };
            assert_ne!(InitCommonControlsEx(&controls), 0);
            for size in [
                std::mem::offset_of!(TTTOOLINFOW, lParam) as u32,
                std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
            ] {
                let mut tool = TTTOOLINFOW {
                    cbSize: size,
                    uFlags: TTF_IDISHWND | TTF_SUBCLASS,
                    hwnd: window.0,
                    uId: window.0 as usize,
                    lpszText: w!("resource test").cast_mut(),
                    ..std::mem::zeroed()
                };
                let tip = tooltip(window.0, &tool).unwrap();
                assert_eq!(window.tooltips(), [tip]);
                assert_eq!(send(tip, TTM_GETTOOLCOUNT, 0, 0), 1);
                assert_eq!(
                    GetWindowLongW(tip, GWL_STYLE) as u32
                        & (WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX),
                    WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX
                );
                assert_ne!(GetWindowLongW(tip, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST, 0);
                assert_ne!(DestroyWindow(tip), 0);
                tool.cbSize = u32::MAX;
                let failure = tooltip(window.0, &tool).unwrap_err();
                assert_eq!(format!("{failure:#}"), "Register tooltip failed");
                assert!(window.tooltips().is_empty());
            }
            let tool = TTTOOLINFOW {
                cbSize: std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
                uFlags: TTF_IDISHWND | TTF_SUBCLASS,
                hwnd: window.0,
                uId: window.0 as usize,
                lpszText: w!("owned tooltip").cast_mut(),
                ..std::mem::zeroed()
            };
            let tip = tooltip(window.0, &tool).unwrap();
            drop(window);
            assert_eq!(IsWindow(tip), 0);
        }
    }

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
            let parent = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                crate::layout::rect(0, 0, 700, 300),
            );
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
        let failure = unsafe {
            child(
                GetDesktopWindow(),
                w!("Winlex.Test.UnregisteredControl"),
                &[0],
                0,
                123,
            )
        }
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
#[test]
fn native_text_inputs_reject_unterminated_slices() {
    let parent = unsafe { GetDesktopWindow() };
    for text in [&[][..], &[b'x' as u16][..]] {
        assert!(unsafe { child(parent, w!("STATIC"), text, 0, 1) }.is_err());
        assert!(unsafe { child_ex(parent, w!("STATIC"), text, 0, 0, 1) }.is_err());
        let menu = Popup::new().unwrap();
        assert!(menu.item(1, text, true).is_err());
        menu.item(1, &[0], true).unwrap();
    }
}

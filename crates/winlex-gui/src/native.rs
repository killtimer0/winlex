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

/// Owns a native theme handle; dropping closes it exactly once.
pub struct Theme(std::cell::Cell<HTHEME>);
impl Theme {
    pub const fn empty() -> Self {
        Self(std::cell::Cell::new(0))
    }
    pub fn open(hwnd: HWND, class: *const u16) -> Self {
        Self(std::cell::Cell::new(unsafe { OpenThemeData(hwnd, class) }))
    }
    pub fn get(&self, hwnd: HWND, class: *const u16) -> HTHEME {
        if self.0.get() == 0 {
            self.0.set(unsafe { OpenThemeData(hwnd, class) });
        }
        self.0.get()
    }
    pub fn raw(&self) -> HTHEME {
        self.0.get()
    }
    pub fn clear(&self) {
        let theme = self.0.replace(0);
        if theme != 0 {
            unsafe { CloseThemeData(theme) };
        }
    }
}
impl Drop for Theme {
    fn drop(&mut self) {
        self.clear();
    }
}

/// XP accepts the older TTTOOLINFOW layout without the trailing reserved field.
pub const TOOLINFO_SIZE: u32 = std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32;

/// A tool that uses its control handle as both the tool id and the tracked window.
pub fn tooltip_info(control: HWND, text: *mut u16) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: TOOLINFO_SIZE,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: control,
        uId: control as usize,
        lpszText: text,
        ..unsafe { std::mem::zeroed() }
    }
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
/// Signed client coordinates from a message's low/high words.
pub fn point_from_lparam(value: isize) -> POINT {
    POINT {
        x: value as i16 as i32,
        y: (value >> 16) as i16 as i32,
    }
}
pub fn window_placement(hwnd: HWND, action: &str) -> anyhow::Result<WINDOWPLACEMENT> {
    let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        Err(error(action))
    } else {
        Ok(placement)
    }
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
/// Set the closed height of a combo box to `rows` text rows, keeping its width.
pub fn collapse_combo(hwnd: HWND, row: i32, rows: i32) -> anyhow::Result<()> {
    let mut rect = bounds(hwnd, unsafe { GetParent(hwnd) });
    rect.bottom = rect.top + row * rows;
    set_bounds(hwnd, rect)
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
            // Resizing can reflow content inside the old client area as well.
            // Let each control repaint instead of preserving stale pixels.
            batch = DeferWindowPos(
                batch,
                hwnd,
                ptr::null_mut(),
                r.left,
                r.top,
                (r.right - r.left).max(0),
                (r.bottom - r.top).max(0),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
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

/// Forward the shared context-menu gestures of a child control to its owner:
/// right-click, the menu key and Shift+F10. Returns the handled result.
pub fn forward_context_menu(hwnd: HWND, msg: u32, w: usize, l: isize) -> Option<isize> {
    unsafe {
        if msg == WM_CONTEXTMENU {
            return Some(send(GetParent(hwnd), msg, hwnd as usize, l));
        }
        let menu_key =
            w == VK_APPS as usize || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0;
        (menu_key && (msg == WM_KEYDOWN || msg == WM_KEYUP)).then(|| {
            if msg == WM_KEYDOWN {
                send(GetParent(hwnd), WM_CONTEXTMENU, hwnd as usize, -1)
            } else {
                0
            }
        })
    }
}

/// Ask for one `WM_MOUSELEAVE` after the pointer leaves `hwnd`.
pub fn track_leave(hwnd: HWND) {
    let mut event = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        TrackMouseEvent(&mut event);
    }
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

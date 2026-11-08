use crate::native::*;
use std::sync::Arc;
use windows_sys::Win32::UI::Shell::SUBCLASSPROC;
use winlex_core::Catalog;

include!("../../../tests/rust_search.rs");

// The observer must stay alive and unmoved until this subclass is removed.
pub(crate) unsafe fn observe_messages<F>(hwnd: HWND, id: usize, observer: &F) -> SUBCLASSPROC
where
    F: Fn(HWND, u32, usize, isize) -> Option<isize>,
{
    unsafe extern "system" fn dispatch<F>(
        hwnd: HWND,
        message: u32,
        w: usize,
        l: isize,
        _: usize,
        data: usize,
    ) -> isize
    where
        F: Fn(HWND, u32, usize, isize) -> Option<isize>,
    {
        unsafe {
            (&*(data as *const F))(hwnd, message, w, l)
                .unwrap_or_else(|| default_subclass(hwnd, message, w, l))
        }
    }
    let callback: SUBCLASSPROC = Some(dispatch::<F>);
    assert_ne!(
        unsafe { set_subclass(hwnd, callback, id, observer as *const F as usize) },
        0
    );
    callback
}

pub(crate) fn initialize_controls(classes: u32) {
    let controls = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: classes,
    };
    assert_ne!(unsafe { InitCommonControlsEx(&controls) }, 0);
}

pub(crate) fn catalog() -> Arc<Catalog> {
    Arc::new(
        Catalog::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../inputs/Windows.Win32.winmd"),
        )
        .unwrap(),
    )
}

pub(crate) const SURFACE_MARKER: u32 = 0x00123456;

pub(crate) fn mark_surface(dc: HDC, area: RECT) {
    unsafe {
        let brush = GdiObject(CreateSolidBrush(SURFACE_MARKER));
        FillRect(dc, &area, brush.0);
    }
}

pub(crate) fn assert_surface_clipped(dc: HDC) {
    assert_eq!(
        unsafe { GetPixel(dc, 5, 5) },
        SURFACE_MARKER,
        "painting must not change pixels outside rcPaint"
    );
}

pub(crate) fn assert_surface_reused(before: (HDC, HBITMAP), after: (HDC, HBITMAP)) {
    assert_eq!(
        after, before,
        "same-size repaints must reuse the GDI surface"
    );
    assert_surface_clipped(after.0);
}

pub(crate) fn bitmap_size(bitmap: HBITMAP) -> (i32, i32) {
    unsafe {
        let mut info: BITMAP = std::mem::zeroed();
        assert_ne!(
            GetObjectW(
                bitmap,
                std::mem::size_of::<BITMAP>() as i32,
                (&mut info as *mut BITMAP).cast()
            ),
            0
        );
        (info.bmWidth, info.bmHeight)
    }
}

pub(crate) fn resize_surface_window(hwnd: HWND, width: i32, height: i32) {
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            width,
            height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub(crate) fn move_offscreen(hwnd: HWND) {
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

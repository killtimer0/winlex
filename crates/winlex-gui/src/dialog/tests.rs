use super::*;
use windows_sys::Win32::UI::HiDpi::*;

fn set_thread_dpi_context(context: DPI_AWARENESS_CONTEXT) -> Option<DPI_AWARENESS_CONTEXT> {
    unsafe {
        let module = GetModuleHandleW(w!("user32.dll"));
        let address = GetProcAddress(module, c"SetThreadDpiAwarenessContext".as_ptr().cast())?;
        let set: unsafe extern "system" fn(DPI_AWARENESS_CONTEXT) -> DPI_AWARENESS_CONTEXT =
            std::mem::transmute(address);
        let previous = set(context);
        assert!(!previous.is_null());
        Some(previous)
    }
}

struct Window {
    hwnd: HWND,
    context: Option<DPI_AWARENESS_CONTEXT>,
}
impl Window {
    fn new() -> Self {
        unsafe {
            let context = set_thread_dpi_context(DPI_AWARENESS_CONTEXT_UNAWARE);
            let mut template = vec![0u16; 13];
            let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | DS_SETFONT as u32;
            template[0] = style as u16;
            template[1] = (style >> 16) as u16;
            template[4] = 3;
            template[7] = 200;
            template[8] = 120;
            template[12] = 8;
            template.extend("MS Shell Dlg\0".encode_utf16());
            for id in 1..=3 {
                if !template.len().is_multiple_of(2) {
                    template.push(0);
                }
                let style = WS_CHILD | WS_VISIBLE;
                template.extend([
                    style as u16,
                    (style >> 16) as u16,
                    0,
                    0,
                    10,
                    10 + id * 25,
                    60,
                    14,
                    id,
                    0xffff,
                    0x0080,
                    0,
                    0,
                ]);
            }
            unsafe extern "system" fn proc(_: HWND, _: u32, _: usize, _: isize) -> isize {
                0
            }
            let hwnd = CreateDialogIndirectParamW(
                instance(),
                template.as_ptr().cast(),
                ptr::null_mut(),
                Some(proc),
                0,
            );
            assert!(!hwnd.is_null());
            Self { hwnd, context }
        }
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            if let Some(context) = self.context {
                assert!(set_thread_dpi_context(context).is_some());
            }
        }
    }
}

#[test]
fn dialog_template_sizes_accept_both_formats_and_reject_truncation() {
    for (offset, extended) in [(14, false), (22, true)] {
        let mut bytes = vec![0; offset + 4];
        if extended {
            bytes[..4].copy_from_slice(&[1, 0, 255, 255]);
        }
        bytes[offset..offset + 4].copy_from_slice(&[200, 0, 120, 0]);
        let size = template_size(&bytes).unwrap();
        assert_eq!((size.cx, size.cy), (200, 120));
        assert!(template_size(&bytes[..offset + 3]).is_err());
        bytes[offset..offset + 2].copy_from_slice(&[255, 255]);
        assert!(template_size(&bytes).is_err());
    }
}

#[test]
fn dialog_rules_preserve_anchors_and_restore_geometry_after_dpi_round_trips() {
    let window = Window::new();
    let hwnd = window.hwnd;
    let mut layout = Layout::capture(
        hwnd,
        Dpi { mode: Mode::Pmv1 },
        SIZE { cx: 200, cy: 120 },
        &[(1, SIZE_X | SIZE_Y), (2, MOVE_X | MOVE_Y)],
        &[],
        true,
    )
    .unwrap();
    layout.update_dpi(96).unwrap();
    let children: Vec<_> = (1..=3).map(|id| unsafe { GetDlgItem(hwnd, id) }).collect();
    let original: Vec<_> = children.iter().map(|&child| bounds(child, hwnd)).collect();
    let outer = bounds(hwnd, ptr::null_mut());
    let larger = crate::layout::rect(
        0,
        0,
        outer.right - outer.left + 100,
        outer.bottom - outer.top + 60,
    );
    set_bounds(hwnd, larger).unwrap();
    unsafe {
        layout.message(WM_SIZE, 0, 0).unwrap();
    }
    let resized: Vec<_> = children.iter().map(|&child| bounds(child, hwnd)).collect();
    assert_eq!(
        (resized[0].left, resized[0].top),
        (original[0].left, original[0].top)
    );
    assert_eq!(
        (
            resized[0].right - original[0].right,
            resized[0].bottom - original[0].bottom
        ),
        (100, 60)
    );
    assert_eq!(
        (
            resized[1].left - original[1].left,
            resized[1].top - original[1].top
        ),
        (100, 60)
    );
    for dpi in [144, 192, 120, 96, 144, 96] {
        let size = layout.initial_size(dpi).unwrap();
        set_bounds(hwnd, crate::layout::rect(0, 0, size.cx, size.cy)).unwrap();
        layout.update_dpi(dpi).unwrap();
    }
    for (&child, expected) in children.iter().zip(&original) {
        let actual = bounds(child, hwnd);
        assert_eq!(
            (actual.left, actual.top, actual.right, actual.bottom),
            (expected.left, expected.top, expected.right, expected.bottom)
        );
    }
}

#[test]
fn dialog_minimum_constrains_user_sizing_without_blocking_top_level_dpi_downscale() {
    let window = Window::new();
    let layout = Layout::capture(
        window.hwnd,
        Dpi { mode: Mode::Pmv1 },
        SIZE { cx: 200, cy: 120 },
        &[],
        &[],
        true,
    )
    .unwrap();
    let minimum = layout.initial_size(96).unwrap();
    for edge in [
        WMSZ_LEFT,
        WMSZ_RIGHT,
        WMSZ_TOP,
        WMSZ_BOTTOM,
        WMSZ_TOPLEFT,
        WMSZ_TOPRIGHT,
        WMSZ_BOTTOMLEFT,
        WMSZ_BOTTOMRIGHT,
    ] {
        let mut r = crate::layout::rect(100, 100, 20, 20);
        unsafe {
            assert!(
                layout
                    .message(WM_SIZING, edge as usize, &mut r as *mut RECT as isize)
                    .unwrap()
            );
        }
        assert_eq!(
            (r.right - r.left, r.bottom - r.top),
            (minimum.cx, minimum.cy)
        );
        if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
            assert_eq!(r.right, 120);
        }
        if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
            assert_eq!(r.bottom, 120);
        }
    }
    let mut limits: MINMAXINFO = unsafe { std::mem::zeroed() };
    unsafe {
        layout
            .message(WM_GETMINMAXINFO, 0, &mut limits as *mut MINMAXINFO as isize)
            .unwrap();
    }
    assert_eq!((limits.ptMinTrackSize.x, limits.ptMinTrackSize.y), (0, 0));
    unsafe {
        let style = GetWindowLongPtrW(window.hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(
            window.hwnd,
            GWL_STYLE,
            ((style & !WS_POPUP) | WS_CHILD) as _,
        );
        layout
            .message(WM_GETMINMAXINFO, 0, &mut limits as *mut MINMAXINFO as isize)
            .unwrap();
        assert_eq!(
            (limits.ptMinTrackSize.x, limits.ptMinTrackSize.y),
            (minimum.cx, minimum.cy)
        );
        SetWindowLongPtrW(window.hwnd, GWL_STYLE, style as _);
    }
}

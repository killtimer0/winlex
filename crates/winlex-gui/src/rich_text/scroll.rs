use crate::native::*;
use windows::Win32::UI::Controls::RichEdit::{
    EM_EXSETSEL, EM_SETCHARFORMAT, EM_SETPARAFORMAT, EM_SETSCROLLPOS, EM_STREAMIN,
};

pub(super) fn install(hwnd: HWND) -> anyhow::Result<()> {
    if unsafe { set_subclass(hwnd, Some(procedure), 1, 0) } == 0 {
        anyhow::bail!("Subclass RichEdit scrolling failed");
    }
    Ok(())
}

pub(super) fn position(hwnd: HWND) -> POINT {
    let mut origin = POINT { x: 0, y: 0 };
    let mut area: RECT = unsafe { std::mem::zeroed() };
    send(hwnd, EM_POSFROMCHAR, &mut origin as *mut _ as usize, 0);
    send(hwnd, EM_GETRECT, 0, &mut area as *mut _ as isize);
    // GETSCROLLPOS scales long documents to a 16-bit range in RichEdit20.
    POINT {
        x: (area.left - origin.x).max(0),
        y: area.top - origin.y,
    }
}

pub(super) fn line_height(hwnd: HWND) -> Option<i32> {
    let index = send(hwnd, EM_LINEINDEX, 1, 0);
    if index < 0 {
        return None;
    }
    let mut first = POINT { x: 0, y: 0 };
    let mut second = POINT { x: 0, y: 0 };
    send(hwnd, EM_POSFROMCHAR, &mut first as *mut _ as usize, 0);
    send(hwnd, EM_POSFROMCHAR, &mut second as *mut _ as usize, index);
    let height = second.y - first.y;
    (height > 0).then_some(height)
}

fn displacement(remainder: isize, delta: i16, pixels: i64) -> (i64, isize) {
    let amount = remainder as i64 - i64::from(delta) * pixels;
    (amount / 120, (amount % 120) as isize)
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    id: usize,
    data: usize,
) -> isize {
    unsafe {
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(procedure), id);
        } else if msg == WM_MOUSEWHEEL && w & 0xffff == 0 && GetKeyState(VK_MENU as i32) >= 0 {
            let mut lines = 0u32;
            if SystemParametersInfoW(
                SPI_GETWHEELSCROLLLINES,
                0,
                (&mut lines as *mut u32).cast(),
                0,
            ) != 0
            {
                let pixels = if lines == u32::MAX {
                    i64::from(client(hwnd).bottom.max(0))
                } else {
                    i64::from(lines) * i64::from(line_height(hwnd).unwrap_or(0))
                }
                .min(i64::from(i32::MAX));
                let (step, mut remainder) = displacement(data as isize, (w >> 16) as i16, pixels);
                if step != 0 {
                    let current = position(hwnd);
                    let target = (i64::from(current.y) + step).clamp(0, i32::MAX as i64) as i32;
                    let point = POINT {
                        x: current.x,
                        y: target,
                    };
                    send(hwnd, EM_SETSCROLLPOS, 0, &point as *const _ as isize);
                    if target == 0 || position(hwnd).y != target {
                        remainder = 0;
                    }
                }
                // Subclass reference data holds only a signed subpixel numerator, not a pointer.
                set_subclass(hwnd, Some(procedure), id, remainder as usize);
                return 0;
            }
            set_subclass(hwnd, Some(procedure), id, 0);
        } else if matches!(
            msg,
            WM_MOUSEWHEEL
                | WM_MOUSEHWHEEL
                | WM_KEYDOWN
                | WM_LBUTTONDOWN
                | WM_RBUTTONDOWN
                | WM_VSCROLL
                | WM_HSCROLL
                | WM_SIZE
                | WM_SETFONT
                | WM_SETTINGCHANGE
                | WM_KILLFOCUS
                | WM_CANCELMODE
                | WM_CAPTURECHANGED
                | WM_DPICHANGED
                | WM_SETTEXT
                | EM_SETCHARFORMAT
                | EM_SETPARAFORMAT
                | EM_STREAMIN
                | EM_SETSCROLLPOS
                | EM_LINESCROLL
                | EM_SETSEL
                | EM_SCROLL
                | EM_SCROLLCARET
                | EM_EXSETSEL
        ) {
            set_subclass(hwnd, Some(procedure), id, 0);
        }
        default_subclass(hwnd, msg, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::{Content, ViewState, create, set_content};
    use windows::Win32::UI::Controls::RichEdit::{
        CFM_SIZE, CHARFORMATW, CHARRANGE, EM_EXGETSEL, SCF_ALL,
    };

    struct Editor(HWND, HWND);

    impl Editor {
        fn new() -> Self {
            let parent = unsafe {
                CreateWindowExW(
                    0,
                    w!("STATIC"),
                    std::ptr::null(),
                    WS_POPUP | WS_VISIBLE,
                    -10000,
                    -10000,
                    900,
                    600,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance(),
                    std::ptr::null(),
                )
            };
            assert!(!parent.is_null());
            let hwnd = create(parent, 1).unwrap();
            place(hwnd, crate::layout::rect(0, 0, 750, 500));
            set_content(
                hwnd,
                &Content::new("const UINT32 value = 42;\n".repeat(15000)),
            )
            .unwrap();
            Self(parent, hwnd)
        }

        fn scroll_to(&self, y: i32) {
            let point = POINT { x: 0, y };
            send(self.1, EM_SETSCROLLPOS, 0, &point as *const _ as isize);
        }

        fn wheel(&self, delta: i16) {
            send(self.1, WM_MOUSEWHEEL, usize::from(delta as u16) << 16, 0);
        }
    }

    impl Drop for Editor {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn reading_position_survives_replacement_deep_scroll_wrapping_and_font_changes() {
        let editor = Editor::new();
        let hwnd = editor.1;
        let format = CHARFORMATW {
            cbSize: std::mem::size_of::<CHARFORMATW>() as u32,
            dwMask: CFM_SIZE,
            yHeight: 300,
            ..Default::default()
        };
        send(
            hwnd,
            EM_SETCHARFORMAT,
            SCF_ALL as usize,
            &format as *const _ as isize,
        );
        let content = Content::new(
            "const UINT32 value😀 = 42; // a long declaration with a stable reading anchor\n"
                .repeat(4000),
        );
        set_content(hwnd, &content).unwrap();
        let selection = CHARRANGE {
            cpMin: 70009,
            cpMax: 70023,
        };
        send(hwnd, EM_EXSETSEL, 0, &selection as *const _ as isize);
        // Finish fallback-font layout before establishing the starting viewport.
        send(hwnd, EM_GETLINECOUNT, 0, 0);
        editor.scroll_to(70003);
        let original = position(hwnd).y;
        assert!(
            original > 65535,
            "deep scroll: {original}, lines: {}",
            send(hwnd, EM_GETLINECOUNT, 0, 0)
        );
        let before = ViewState::capture(hwnd);
        assert_eq!(
            position(hwnd).y,
            original,
            "capturing a reading position must preserve the viewport: {before:?}"
        );
        set_content(hwnd, &Content::new("Querying".into())).unwrap();
        set_content(hwnd, &content).unwrap();
        before.restore(hwnd);
        assert_eq!(position(hwnd).y, original);
        let mut selected = CHARRANGE::default();
        send(hwnd, EM_EXGETSEL, 0, &mut selected as *mut _ as isize);
        assert_eq!((selected.cpMin, selected.cpMax), (70009, 70023));
        let format = CHARFORMATW {
            cbSize: std::mem::size_of::<CHARFORMATW>() as u32,
            dwMask: CFM_SIZE,
            yHeight: 420,
            ..Default::default()
        };
        send(
            hwnd,
            EM_SETCHARFORMAT,
            SCF_ALL as usize,
            &format as *const _ as isize,
        );
        place(hwnd, crate::layout::rect(0, 0, 470, 500));
        send(hwnd, EM_GETLINECOUNT, 0, 0);
        before.restore(hwnd);
        send(hwnd, EM_EXGETSEL, 0, &mut selected as *mut _ as isize);
        assert_eq!((selected.cpMin, selected.cpMax), (70009, 70023));
        let mut anchor = POINT { x: 0, y: 0 };
        let mut area: RECT = unsafe { std::mem::zeroed() };
        send(
            hwnd,
            EM_POSFROMCHAR,
            &mut anchor as *mut _ as usize,
            before.anchor as isize,
        );
        send(hwnd, EM_GETRECT, 0, &mut area as *mut _ as isize);
        assert_eq!(area.top - anchor.y, unsafe {
            MulDiv(
                before.offset,
                line_height(hwnd).unwrap(),
                before.line_height,
            )
        });
    }

    #[test]
    fn native_wheel_is_immediate_precise_and_keeps_selection_in_long_documents() {
        let editor = Editor::new();
        let hwnd = editor.1;
        let range = CHARRANGE {
            cpMin: 10,
            cpMax: 18,
        };
        send(hwnd, EM_EXSETSEL, 0, &range as *const _ as isize);
        for y in [0, 1, 70000, 120000] {
            editor.scroll_to(y);
            assert_eq!(position(hwnd).y, y);
        }
        editor.scroll_to(70000);
        let mut lines = 0u32;
        assert_ne!(
            unsafe {
                SystemParametersInfoW(
                    SPI_GETWHEELSCROLLLINES,
                    0,
                    (&mut lines as *mut u32).cast(),
                    0,
                )
            },
            0
        );
        let pixels = if lines == u32::MAX {
            i64::from(client(hwnd).bottom)
        } else {
            i64::from(lines) * i64::from(line_height(hwnd).unwrap())
        };
        // Use the actual system preference without changing a global desktop setting.
        let mut expected = 70000i64;
        let mut remainder = 0;
        for delta in [-1, -1, -5, -40, -120, 120, 40, 5, 1, 1] {
            let (step, next) = displacement(remainder, delta, pixels);
            editor.wheel(delta);
            expected = (expected + step).clamp(0, i32::MAX as i64);
            let actual = i64::from(position(hwnd).y);
            if pixels <= 10000 {
                assert_eq!(actual, expected, "delta={delta}");
            }
            remainder = if actual == expected && actual != 0 {
                next
            } else {
                0
            };
            expected = actual;
        }
        let mut selected = CHARRANGE::default();
        send(hwnd, EM_EXGETSEL, 0, &mut selected as *mut _ as isize);
        assert_eq!((selected.cpMin, selected.cpMax), (10, 18));
        editor.scroll_to(0);
        editor.wheel(120);
        assert_eq!(position(hwnd).y, 0);
        editor.scroll_to(i32::MAX);
        let bottom = position(hwnd).y;
        editor.wheel(-120);
        assert_eq!(position(hwnd).y, bottom);
        for body in ["", "short"] {
            set_content(hwnd, &Content::new(body.into())).unwrap();
            editor.wheel(-120);
            assert_eq!(position(hwnd).y, 0);
        }
    }

    #[test]
    fn wheel_remeasures_font_and_resets_fraction_on_other_interactions() {
        let editor = Editor::new();
        let hwnd = editor.1;
        let original = line_height(hwnd).unwrap();
        let format = CHARFORMATW {
            cbSize: std::mem::size_of::<CHARFORMATW>() as u32,
            dwMask: CFM_SIZE,
            yHeight: 420,
            ..Default::default()
        };
        send(
            hwnd,
            EM_SETCHARFORMAT,
            SCF_ALL as usize,
            &format as *const _ as isize,
        );
        assert!(line_height(hwnd).unwrap() > original);
        editor.scroll_to(1000);
        let start = position(hwnd).y;
        editor.wheel(-1);
        let single_step = position(hwnd).y - start;
        for message in [WM_SETTINGCHANGE, WM_CANCELMODE, WM_KILLFOCUS] {
            editor.scroll_to(1000);
            editor.wheel(-1);
            send(hwnd, message, 0, 0);
            let before = position(hwnd).y;
            editor.wheel(-1);
            assert_eq!(position(hwnd).y - before, single_step);
            // Explicit positioning and line navigation must remain native.
            editor.scroll_to(1000);
            let before = position(hwnd).y;
            send(hwnd, EM_LINESCROLL, 0, 1);
            assert!(position(hwnd).y > before);
        }
        editor.scroll_to(70000);
        send(hwnd, WM_KEYDOWN, VK_HOME as usize, 0);
        send(hwnd, WM_KEYUP, VK_HOME as usize, 0);
        let mut range = CHARRANGE::default();
        send(hwnd, EM_EXGETSEL, 0, &mut range as *mut _ as isize);
        assert_eq!(range.cpMin, range.cpMax);
    }

    #[test]
    fn fractional_wheel_input_preserves_distance_and_reversal() {
        let mut remainder = 0;
        let mut distance = 0;
        for delta in [-1; 120] {
            let (step, next) = displacement(remainder, delta, 105);
            distance += step;
            remainder = next;
        }
        assert_eq!((distance, remainder), (105, 0));
        assert_eq!(displacement(105, 1, 105), (0, 0));
        assert_eq!(displacement(-105, -1, 105), (0, 0));
        assert_eq!(displacement(0, -120, 636), (636, 0));
        assert_eq!(displacement(0, 120, 0), (0, 0));
        assert_eq!(
            displacement(0, i16::MIN, i64::from(u32::MAX) * 100),
            (
                i64::from(u32::MAX) * 100 * 32768 / 120,
                (i64::from(u32::MAX) * 100 * 32768 % 120) as isize
            )
        );
    }

    #[test]
    fn modified_and_horizontal_wheels_keep_the_native_path() {
        unsafe extern "system" fn observe(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _id: usize,
            data: usize,
        ) -> isize {
            unsafe {
                if matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
                    *(data as *mut (u32, usize, isize)) = (msg, w, l);
                    return 77;
                }
                default_subclass(hwnd, msg, w, l)
            }
        }
        let editor = Editor::new();
        let hwnd = editor.1;
        let mut received = (0u32, 0usize, 0isize);
        unsafe {
            remove_subclass(hwnd, Some(procedure), 1);
            assert_ne!(
                set_subclass(hwnd, Some(observe), 2, &mut received as *mut _ as usize),
                0
            );
        }
        install(hwnd).unwrap();
        for (message, keys) in [
            (WM_MOUSEWHEEL, 8),
            (WM_MOUSEWHEEL, 4),
            (WM_MOUSEWHEEL, 1),
            (WM_MOUSEHWHEEL, 0),
        ] {
            let w = (120usize << 16) | keys;
            assert_eq!(send(hwnd, message, w, 0x123456), 77);
            assert_eq!(received, (message, w, 0x123456));
        }
        received = (0, 0, 0);
        editor.wheel(-120);
        assert_eq!(received, (0, 0, 0));
        let mut original = [0u8; 256];
        assert_ne!(unsafe { GetKeyboardState(original.as_mut_ptr()) }, 0);
        let mut alt = original;
        alt[VK_MENU as usize] = 0x80;
        assert_ne!(unsafe { SetKeyboardState(alt.as_ptr()) }, 0);
        let result = send(hwnd, WM_MOUSEWHEEL, 120 << 16, 0);
        assert_ne!(unsafe { SetKeyboardState(original.as_ptr()) }, 0);
        assert_eq!(result, 77);
        assert_eq!(received, (WM_MOUSEWHEEL, 120 << 16, 0));
        unsafe { remove_subclass(hwnd, Some(observe), 2) };
    }
}

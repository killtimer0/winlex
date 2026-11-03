use crate::native::*;
#[cfg(test)]
use windows::Win32::UI::Controls::RichEdit::EM_SETSCROLLPOS;

pub(super) fn position(hwnd: HWND) -> POINT {
    let mut point = offset(hwnd, 0);
    point.x = point.x.max(0);
    point
}

pub(super) fn offset(hwnd: HWND, index: i32) -> POINT {
    let mut origin = POINT { x: 0, y: 0 };
    let mut area: RECT = unsafe { std::mem::zeroed() };
    send(
        hwnd,
        EM_POSFROMCHAR,
        &mut origin as *mut _ as usize,
        index as isize,
    );
    send_mut(hwnd, EM_GETRECT, 0, &mut area);
    // GETSCROLLPOS scales long documents to a 16-bit range in RichEdit20.
    POINT {
        x: area.left - origin.x,
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

pub(super) fn wheel(hwnd: HWND, delta: i16, lines: u32) {
    let pages = lines == u32::MAX;
    let units = if pages { 1 } else { u64::from(lines) };
    let count = (u64::from(delta.unsigned_abs()) * units + 60) / 120;
    let direction = match (pages, delta > 0) {
        (true, true) => SB_PAGEUP,
        (true, false) => SB_PAGEDOWN,
        (false, true) => SB_LINEUP,
        (false, false) => SB_LINEDOWN,
    };
    for _ in 0..count {
        send(hwnd, WM_VSCROLL, direction as usize, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::{Content, Editor, ViewState, set_content};

    impl Editor {
        fn long_document() -> Self {
            let editor = Self::new(
                WS_POPUP | WS_VISIBLE,
                crate::layout::rect(-10000, -10000, 900, 600),
            );
            let hwnd = editor.1;
            place(hwnd, crate::layout::rect(0, 0, 750, 500));
            set_content(
                hwnd,
                &Content::new("const UINT32 value = 42;\n".repeat(15000)),
            )
            .unwrap();
            editor
        }

        fn scroll_to(&self, y: i32) {
            let point = POINT { x: 0, y };
            send(self.1, EM_SETSCROLLPOS, 0, &point as *const _ as isize);
        }

        fn wheel(&self, delta: i16) {
            send(self.1, WM_MOUSEWHEEL, usize::from(delta as u16) << 16, 0);
        }
    }

    #[test]
    fn reading_position_survives_replacement_deep_scroll_wrapping_and_font_changes() {
        let editor = Editor::long_document();
        let hwnd = editor.1;
        editor.font_size(300);
        let content = Content::new(
            "const UINT32 value😀 = 42; // a long declaration with a stable reading anchor\n"
                .repeat(4000),
        );
        set_content(hwnd, &content).unwrap();
        editor.select(70009, 70023);
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
        for _ in 0..2 {
            crate::rich_text::recolor(hwnd, &content).unwrap();
            assert_eq!(ViewState::capture(hwnd), before);
            assert_eq!(position(hwnd).y, original);
        }
        set_content(hwnd, &Content::new("Querying".into())).unwrap();
        set_content(hwnd, &content).unwrap();
        before.restore(hwnd);
        assert_eq!(position(hwnd).y, original);
        assert_eq!(editor.selection(), (70009, 70023));
        editor.font_size(420);
        place(hwnd, crate::layout::rect(0, 0, 470, 500));
        send(hwnd, EM_GETLINECOUNT, 0, 0);
        before.restore(hwnd);
        assert_eq!(editor.selection(), (70009, 70023));
        let anchor = editor.character_position(before.anchor);
        let mut area: RECT = unsafe { std::mem::zeroed() };
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
    fn wheel_endpoint_matches_native_scrollbar() {
        for rows in [3, 80, 15000] {
            let editor = Editor::long_document();
            let hwnd = editor.1;
            set_content(hwnd, &Content::new("line\n".repeat(rows))).unwrap();
            send(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
            let bottom = position(hwnd).y;
            if rows == 3 {
                assert_eq!(bottom, 0);
            } else {
                assert!(bottom > 0);
            }
            for _ in 0..30 {
                editor.wheel(-120);
                assert_eq!(
                    position(hwnd).y,
                    bottom,
                    "rows={rows}: wheel must stay at native bottom"
                );
            }
            editor.wheel(120);
            let reversed = position(hwnd).y;
            send(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
            editor.wheel(120);
            assert_eq!(
                position(hwnd).y,
                reversed,
                "reversal must start at the native bottom"
            );
            editor.wheel(-120);
            assert!(position(hwnd).y <= bottom);
            editor.wheel(-120);
            assert_eq!(position(hwnd).y, bottom);
            editor.scroll_to((bottom - 1).max(0));
            wheel(hwnd, -120, u32::MAX);
            assert_eq!(position(hwnd).y, bottom);
            wheel(hwnd, 120, u32::MAX);
            assert!(position(hwnd).y <= bottom);
            wheel(hwnd, -240, u32::MAX);
            assert_eq!(position(hwnd).y, bottom);
        }
    }

    #[test]
    fn native_wheel_is_immediate_and_keeps_selection_in_long_documents() {
        let editor = Editor::long_document();
        let hwnd = editor.1;
        editor.select(10, 18);
        for y in [0, 1, 70000, 120000] {
            editor.scroll_to(y);
            assert_eq!(position(hwnd).y, y);
        }
        send(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
        let bottom = position(hwnd).y;
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
        if lines == u32::MAX {
            send(hwnd, WM_VSCROLL, SB_PAGEDOWN as usize, 0);
        } else {
            for _ in 0..lines {
                send(hwnd, WM_VSCROLL, SB_LINEDOWN as usize, 0);
            }
        }
        let expected = position(hwnd).y;
        if lines == u32::MAX {
            send(hwnd, WM_VSCROLL, SB_PAGEUP as usize, 0);
        } else {
            for _ in 0..lines {
                send(hwnd, WM_VSCROLL, SB_LINEUP as usize, 0);
            }
        }
        let reversed = position(hwnd).y;
        editor.scroll_to(70000);
        // Use the actual system preference without changing a global desktop setting.
        editor.wheel(-120);
        let whole = position(hwnd).y;
        assert_eq!(whole, expected);
        editor.wheel(120);
        assert_eq!(position(hwnd).y, reversed);
        assert_eq!(editor.selection(), (10, 18));
        editor.scroll_to(0);
        editor.wheel(120);
        assert_eq!(position(hwnd).y, 0);
        send(hwnd, WM_VSCROLL, SB_BOTTOM as usize, 0);
        editor.wheel(-120);
        assert_eq!(position(hwnd).y, bottom);
        for body in ["", "short"] {
            set_content(hwnd, &Content::new(body.into())).unwrap();
            editor.wheel(-120);
            assert_eq!(position(hwnd).y, 0);
        }
    }

    #[test]
    fn wheel_uses_current_layout_after_font_changes() {
        let editor = Editor::long_document();
        let hwnd = editor.1;
        let original = line_height(hwnd).unwrap();
        editor.font_size(420);
        assert!(line_height(hwnd).unwrap() > original);
        send(hwnd, EM_GETLINECOUNT, 0, 0);
        editor.scroll_to(100 * line_height(hwnd).unwrap());
        let before = send(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0);
        wheel(hwnd, -120, 3);
        assert_eq!(send(hwnd, EM_GETFIRSTVISIBLELINE, 0, 0), before + 3);
        editor.scroll_to(70000);
        send(hwnd, WM_KEYDOWN, VK_HOME as usize, 0);
        send(hwnd, WM_KEYUP, VK_HOME as usize, 0);
        let (start, end) = editor.selection();
        assert_eq!(start, end);
    }

    #[test]
    fn wheel_rounds_each_message_and_respects_line_and_page_preferences() {
        unsafe extern "system" fn observe(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _id: usize,
            data: usize,
        ) -> isize {
            unsafe {
                if msg == WM_VSCROLL {
                    assert_eq!(l, 0);
                    (*(data as *mut Vec<usize>)).push(w);
                    return 0;
                }
                default_subclass(hwnd, msg, w, l)
            }
        }
        let editor = Editor::long_document();
        let hwnd = editor.1;
        let mut commands = Vec::<usize>::new();
        unsafe {
            assert_ne!(
                set_subclass(hwnd, Some(observe), 2, &mut commands as *mut _ as usize),
                0
            )
        };
        for (delta, lines, direction, count) in [
            (0, 3, SB_LINEDOWN, 0),
            (-120, 0, SB_LINEDOWN, 0),
            (-19, 3, SB_LINEDOWN, 0),
            (-20, 3, SB_LINEDOWN, 1),
            (19, 3, SB_LINEUP, 0),
            (20, 3, SB_LINEUP, 1),
            (-59, 1, SB_LINEDOWN, 0),
            (-60, 1, SB_LINEDOWN, 1),
            (-120, 3, SB_LINEDOWN, 3),
            (120, 3, SB_LINEUP, 3),
            (-240, 3, SB_LINEDOWN, 6),
            (240, 3, SB_LINEUP, 6),
            (-59, u32::MAX, SB_PAGEDOWN, 0),
            (-60, u32::MAX, SB_PAGEDOWN, 1),
            (60, u32::MAX, SB_PAGEUP, 1),
            (-240, u32::MAX, SB_PAGEDOWN, 2),
            (i16::MIN, 1, SB_LINEDOWN, 273),
        ] {
            commands.clear();
            wheel(hwnd, delta, lines);
            assert_eq!(
                commands,
                vec![direction as usize; count],
                "delta={delta}, lines={lines}"
            );
        }
        commands.clear();
        for delta in [-1, 1] {
            for _ in 0..120 {
                wheel(hwnd, delta, 3);
            }
            assert!(
                commands.is_empty(),
                "subthreshold messages must not accumulate"
            );
        }
        unsafe { remove_subclass(hwnd, Some(observe), 2) };
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
        let editor = Editor::long_document();
        let hwnd = editor.1;
        let mut received = (0u32, 0usize, 0isize);
        unsafe {
            remove_subclass(hwnd, Some(crate::rich_text::procedure), 1);
            assert_ne!(
                set_subclass(hwnd, Some(observe), 2, &mut received as *mut _ as usize),
                0
            );
        }
        assert_ne!(
            unsafe { set_subclass(hwnd, Some(crate::rich_text::procedure), 1, 0) },
            0
        );
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

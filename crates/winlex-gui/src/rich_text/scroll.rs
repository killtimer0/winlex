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

#[derive(Default)]
pub(super) struct WheelState {
    remainder: i64,
    lines: u32,
}

impl WheelState {
    pub(super) fn step(&mut self, delta: i16, lines: u32) -> (i32, u64) {
        if self.lines != lines || self.remainder.signum() != i64::from(delta).signum() {
            self.remainder = 0;
        }
        self.lines = lines;
        let pages = lines == u32::MAX;
        let units = if pages { 1 } else { i64::from(lines) };
        let amount = self.remainder + i64::from(delta) * units;
        self.remainder = amount % 120;
        let direction = match (pages, delta > 0) {
            (true, true) => SB_PAGEUP,
            (true, false) => SB_PAGEDOWN,
            (false, true) => SB_LINEUP,
            (false, false) => SB_LINEDOWN,
        };
        (direction, (amount / 120).unsigned_abs())
    }
}

pub(super) fn wheel(hwnd: HWND, direction: i32, count: u64) {
    if count == 0 {
        return;
    }
    // Visible RichEdit20 synchronously copies pixels for every native scroll.
    // Keep native line/page bounds, but paint the completed wheel step once.
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    if visible {
        send(hwnd, WM_SETREDRAW, 0, 0);
    }
    for _ in 0..count {
        send(hwnd, WM_VSCROLL, direction as usize, 0);
    }
    if visible {
        send(hwnd, WM_SETREDRAW, 1, 0);
        unsafe {
            RedrawWindow(
                hwnd,
                std::ptr::null(),
                std::ptr::null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_FRAME,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rich_text::{Content, Editor, EditorState, ViewState, set_content};

    fn editor_data(hwnd: HWND) -> usize {
        let mut data = 0;
        assert_ne!(
            unsafe {
                windows_sys::Win32::UI::Shell::GetWindowSubclass(
                    hwnd,
                    Some(crate::rich_text::procedure),
                    1,
                    &mut data,
                )
            },
            0
        );
        data
    }

    fn wheel(hwnd: HWND, delta: i16, lines: u32) {
        let state = unsafe { &mut *(editor_data(hwnd) as *mut EditorState) };
        let (direction, count) = state.wheel.step(delta, lines);
        super::wheel(hwnd, direction, count);
    }

    impl Editor {
        fn long_document() -> Self {
            let editor = Self::new(
                WS_POPUP | WS_VISIBLE,
                crate::layout::rect(-10000, -10000, 900, 600),
            );
            let hwnd = editor.1;
            set_bounds(hwnd, crate::layout::rect(0, 0, 750, 500)).unwrap();
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
        set_bounds(hwnd, crate::layout::rect(0, 0, 470, 500)).unwrap();
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
    fn wheel_preserves_hidden_control_visibility_and_selection() {
        let editor = Editor::long_document();
        let hwnd = editor.1;
        editor.select(10, 18);
        unsafe { ShowWindow(hwnd, SW_HIDE) };
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
        editor.wheel(-120);
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(editor.selection(), (10, 18));
        unsafe { ShowWindow(hwnd, SW_SHOWNA) };
        assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0);
        editor.wheel(-120);
        assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(editor.selection(), (10, 18));
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
        let native_scroll = |line, page| {
            if lines == u32::MAX {
                send(hwnd, WM_VSCROLL, page as usize, 0);
            } else {
                for _ in 0..lines {
                    send(hwnd, WM_VSCROLL, line as usize, 0);
                }
            }
        };
        native_scroll(SB_LINEDOWN, SB_PAGEDOWN);
        let expected = position(hwnd).y;
        native_scroll(SB_LINEUP, SB_PAGEUP);
        let reversed = position(hwnd).y;
        editor.scroll_to(70000);
        // Use the actual system preference without changing a global desktop setting.
        editor.wheel(-120);
        let whole = position(hwnd).y;
        assert_eq!(whole, expected);
        editor.scroll_to(70000);
        for _ in 0..120 {
            editor.wheel(-1);
        }
        assert_eq!(position(hwnd).y, whole);
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
    fn fractional_wheel_input_accumulates_per_control_and_respects_preferences() {
        let first = Editor::long_document();
        let second = Editor::long_document();
        first.select(10, 18);
        for editor in [&first, &second] {
            editor.scroll_to(70000);
        }
        let start = position(first.1).y;
        wheel(first.1, -20, 3);
        wheel(second.1, -20, 3);
        assert_eq!(position(first.1).y, start);
        assert_eq!(position(second.1).y, start);
        wheel(first.1, -20, 3);
        send(second.1, WM_VSCROLL, SB_LINEDOWN as usize, 0);
        assert_eq!(position(first.1).y, position(second.1).y);
        wheel(first.1, 20, 3);
        wheel(first.1, -20, 3);
        let before = position(first.1).y;
        wheel(first.1, 20, 3);
        assert_eq!(position(first.1).y, before);
        wheel(first.1, 20, 3);
        send(second.1, WM_VSCROLL, SB_LINEUP as usize, 0);
        let reversed = position(second.1).y;
        assert!(reversed < before);
        assert_eq!(position(first.1).y, reversed);
        wheel(first.1, -60, 1);
        wheel(first.1, -60, u32::MAX);
        assert_eq!(position(first.1).y, reversed);
        wheel(first.1, -60, u32::MAX);
        send(second.1, WM_VSCROLL, SB_PAGEDOWN as usize, 0);
        assert_eq!(position(first.1).y, position(second.1).y);
        wheel(first.1, -120, 0);
        assert_eq!(position(first.1).y, position(second.1).y);
        assert_eq!(first.selection(), (10, 18));

        send(first.1, WM_VSCROLL, SB_BOTTOM as usize, 0);
        let bottom = position(first.1).y;
        for _ in 0..200 {
            wheel(first.1, -1, 3);
        }
        assert_eq!(position(first.1).y, bottom);
        wheel(first.1, 40, 3);
        assert!(position(first.1).y < bottom);
    }

    #[test]
    fn wheel_amount_handles_large_delta_and_preference() {
        let mut state = WheelState::default();
        assert_eq!(state.step(i16::MIN, 1), (SB_LINEDOWN, 273));
        assert_eq!(state.step(-120, 0), (SB_LINEDOWN, 0));
        assert_eq!(
            state.step(i16::MIN, u32::MAX - 1),
            (SB_LINEDOWN, 32768 * u64::from(u32::MAX - 1) / 120)
        );
    }

    #[test]
    fn modified_and_horizontal_wheels_keep_the_native_path() {
        let editor = Editor::long_document();
        let hwnd = editor.1;
        let received = std::cell::Cell::new((0u32, 0usize, 0isize));
        let observe = |_, msg, w, l| {
            if matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
                received.set((msg, w, l));
                return Some(77);
            }
            None
        };
        let data = editor_data(hwnd);
        let callback;
        unsafe {
            remove_subclass(hwnd, Some(crate::rich_text::procedure), 1);
            callback = crate::test_support::observe_messages(hwnd, 2, &observe);
        }
        assert_ne!(
            unsafe { set_subclass(hwnd, Some(crate::rich_text::procedure), 1, data) },
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
            assert_eq!(received.get(), (message, w, 0x123456));
        }
        received.set((0, 0, 0));
        editor.wheel(-120);
        assert_eq!(received.get(), (0, 0, 0));
        let mut original = [0u8; 256];
        assert_ne!(unsafe { GetKeyboardState(original.as_mut_ptr()) }, 0);
        let mut alt = original;
        alt[VK_MENU as usize] = 0x80;
        assert_ne!(unsafe { SetKeyboardState(alt.as_ptr()) }, 0);
        let result = send(hwnd, WM_MOUSEWHEEL, 120 << 16, 0);
        assert_ne!(unsafe { SetKeyboardState(original.as_ptr()) }, 0);
        assert_eq!(result, 77);
        assert_eq!(received.get(), (WM_MOUSEWHEEL, 120 << 16, 0));
        unsafe { remove_subclass(hwnd, callback, 2) };
    }
}

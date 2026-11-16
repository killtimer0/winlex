use crate::native::*;

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

use crate::native::*;

pub fn scale(value: i32, dpi: u32) -> i32 {
    unsafe { MulDiv(value, dpi as i32, USER_DEFAULT_SCREEN_DPI as i32) }
}
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w.max(0),
        bottom: y + h.max(0),
    }
}

pub fn inset(area: RECT, margin: i32) -> RECT {
    let x = margin.min((area.right - area.left) / 2);
    let y = margin.min((area.bottom - area.top) / 2);
    rect(
        area.left + x,
        area.top + y,
        area.right - area.left - x * 2,
        area.bottom - area.top - y * 2,
    )
}
pub fn take_top(area: &mut RECT, height: i32) -> RECT {
    let top = area.top;
    area.top = (top + height).min(area.bottom);
    rect(area.left, top, area.right - area.left, area.top - top)
}
pub fn take_left(area: &mut RECT, width: i32) -> RECT {
    let left = area.left;
    area.left = (left + width).min(area.right);
    rect(left, area.top, area.left - left, area.bottom - area.top)
}

/// A single font/DPI snapshot for measurement and placement; does not own the font.
pub struct Metrics {
    pub dpi: u32,
    pub font: HFONT,
    pub height: i32,
}
impl Metrics {
    pub fn px(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }
    pub fn row(&self) -> i32 {
        self.height + self.px(12)
    }
    pub fn label_width<T: AsRef<[u16]>>(
        &self,
        labels: impl Iterator<Item = T>,
        minimum: i32,
    ) -> i32 {
        let mut width = self.px(minimum);
        unsafe {
            let dc = GetDC(std::ptr::null_mut());
            let old = SelectObject(dc, self.font);
            for label in labels {
                let mut bounds = rect(0, 0, 0, 0);
                DrawTextW(
                    dc,
                    // DT_MODIFYSTRING is not set; DrawTextW only reads the label.
                    label.as_ref().as_ptr().cast_mut(),
                    -1,
                    &mut bounds,
                    DT_CALCRECT | DT_SINGLELINE,
                );
                width = width.max(bounds.right + self.px(24));
            }
            SelectObject(dc, old);
            ReleaseDC(std::ptr::null_mut(), dc);
        }
        width
    }
    pub fn buttons(&self, area: &mut RECT, controls: impl IntoIterator<Item = HWND>) {
        let controls: Vec<_> = controls.into_iter().collect();
        if controls.is_empty() {
            return;
        }
        let gap = self.px(4);
        let width = self.label_width(controls.iter().map(|&h| text_wide(h)), 75);
        let available = (area.right - area.left).max(0);
        let columns = ((available + gap) / (width + gap)).max(1);
        let width = width.min(available);
        let row = self.row();
        let bounds = take_top(
            area,
            (controls.len() as i32 + columns - 1) / columns * (row + gap),
        );
        for (i, h) in controls.into_iter().enumerate() {
            let i = i as i32;
            let y = (bounds.top + i / columns * (row + gap)).min(bounds.bottom);
            place(
                h,
                rect(
                    bounds.left + i % columns * (width + gap),
                    y,
                    width,
                    row.min(bounds.bottom - y),
                ),
            );
        }
    }
}

/// Stable coordinates recorded only at user-operation boundaries.
#[derive(Clone, Copy)]
pub struct Placement {
    pub bounds: RECT,
    pub dpi: u32,
}
impl Placement {
    pub fn at(self, dpi: u32) -> RECT {
        let convert = |v| unsafe { MulDiv(v, dpi as i32, self.dpi as i32) };
        RECT {
            left: convert(self.bounds.left),
            top: convert(self.bounds.top),
            right: convert(self.bounds.right),
            bottom: convert(self.bounds.bottom),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    Free,
    Columns,
    Rows,
}

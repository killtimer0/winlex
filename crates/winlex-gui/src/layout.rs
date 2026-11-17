use crate::native::*;

pub const DEFAULT_WIDTH: i32 = 1000;
pub const DEFAULT_HEIGHT: i32 = 680;
pub const MIN_WIDTH: i32 = 480;
pub const MIN_HEIGHT: i32 = 300;

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
pub fn center_vertical(mut row: RECT, height: i32) -> RECT {
    let height = height.min(row.bottom - row.top).max(0);
    row.top += (row.bottom - row.top - height) / 2;
    row.bottom = row.top + height;
    row
}

pub struct FlowItem {
    pub size: SIZE,
    pub stretch: bool,
}

/// Pack measured items into rows; share spare width among stretching items.
/// Items stay in order, are limited to the available width, and consume a trailing row gap.
pub fn flow(area: &mut RECT, items: &[FlowItem], column_gap: i32, row_gap: i32) -> Vec<RECT> {
    let available = (area.right - area.left).max(0);
    let mut positions = Vec::with_capacity(items.len());
    let mut start = 0;
    while start < items.len() {
        let (mut end, mut used, mut height, mut stretching) = (start, 0, 0, 0);
        while end < items.len() {
            let item = &items[end];
            let width = item.size.cx.max(0).min(available);
            let gap = if end == start { 0 } else { column_gap };
            if end != start && used + gap + width > available {
                break;
            }
            used += gap + width;
            height = height.max(item.size.cy);
            stretching += i32::from(item.stretch);
            end += 1;
        }
        let mut row = take_top(area, height);
        let mut spare = available - used;
        for item in &items[start..end] {
            let mut width = item.size.cx.max(0).min(available);
            if item.stretch {
                let extra = spare / stretching;
                width += extra;
                spare -= extra;
                stretching -= 1;
            }
            positions.push(take_left(&mut row, width));
            take_left(&mut row, column_gap);
        }
        take_top(area, row_gap);
        start = end;
    }
    positions
}

pub fn limit_sizing(area: &mut RECT, edge: u32, minimum: SIZE) {
    if area.right - area.left < minimum.cx {
        if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
            area.left = area.right - minimum.cx;
        } else {
            area.right = area.left + minimum.cx;
        }
    }
    if area.bottom - area.top < minimum.cy {
        if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
            area.top = area.bottom - minimum.cy;
        } else {
            area.bottom = area.top + minimum.cy;
        }
    }
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
    pub fn edit_height(&self, hwnd: HWND, dpi: crate::dpi::Dpi) -> i32 {
        let outer = dpi.child_outer(hwnd, rect(0, 0, 1, self.height + self.px(4)));
        outer.bottom - outer.top
    }
    pub fn text_width(&self, text: &[u16]) -> i32 {
        let length = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        let mut bounds = rect(0, 0, 0, 0);
        with_window_font(std::ptr::null_mut(), self.font, |dc| unsafe {
            if length != 0 {
                DrawTextW(
                    dc,
                    text.as_ptr().cast_mut(),
                    i32::try_from(length).expect("label length fits DrawTextW"),
                    &mut bounds,
                    DT_CALCRECT | DT_SINGLELINE,
                );
            }
        });
        bounds.right
    }
    pub fn button_size(&self, hwnd: HWND) -> SIZE {
        let mut size = SIZE { cx: 0, cy: 0 };
        if send_mut(hwnd, BCM_GETIDEALSIZE, 0, &mut size) == 0 || size.cx <= 0 || size.cy <= 0 {
            size.cx = self.text_width(&text_wide(hwnd)) + self.px(24);
            size.cy = self.row();
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as i32 } & BS_TYPEMASK;
            if matches!(
                style,
                BS_CHECKBOX | BS_AUTOCHECKBOX | BS_3STATE | BS_AUTO3STATE
            ) {
                let glyph = unsafe {
                    windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXMENUCHECK, self.dpi)
                };
                size.cx = self.text_width(&text_wide(hwnd)) + glyph + self.px(8);
                size.cy = size.cy.max(glyph);
            }
        }
        size
    }
    /// Preferred closed width for the current items, independent of selection.
    pub fn combo_width(&self, hwnd: HWND) -> i32 {
        // Legacy child nonclient metrics can remain at system DPI after a move.
        let arrow = unsafe {
            GetSystemMetrics(SM_CXVSCROLL).max(
                windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXVSCROLL, self.dpi),
            )
        };
        let mut width = 0;
        with_window_font(hwnd, self.font, |dc| {
            for index in 0..send(hwnd, CB_GETCOUNT, 0, 0).max(0) {
                let length = send(hwnd, CB_GETLBTEXTLEN, index as usize, 0);
                if length < 0 {
                    continue;
                }
                let mut text = vec![0u16; length as usize + 1];
                let length = send(
                    hwnd,
                    CB_GETLBTEXT,
                    index as usize,
                    text.as_mut_ptr() as isize,
                );
                let mut size = SIZE { cx: 0, cy: 0 };
                if length > 0 {
                    // Combo items display ampersands literally, unlike button captions.
                    unsafe { GetTextExtentPoint32W(dc, text.as_ptr(), length as i32, &mut size) };
                }
                width = width.max(size.cx);
            }
        });
        // A conservative allowance for borders and text padding, not an exact native inset.
        width + arrow + self.px(8)
    }
    pub fn buttons(
        &self,
        area: &mut RECT,
        controls: impl IntoIterator<Item = HWND>,
        positions: &mut Vec<(HWND, RECT)>,
    ) {
        let controls: Vec<_> = controls.into_iter().collect();
        if controls.is_empty() {
            return;
        }
        let gap = self.px(4);
        let size = self.button_group_size(&controls);
        let items: Vec<_> = controls
            .iter()
            .map(|_| FlowItem {
                size,
                stretch: false,
            })
            .collect();
        positions.extend(controls.into_iter().zip(flow(area, &items, gap, gap)));
    }
    pub fn button_group_size(&self, controls: &[HWND]) -> SIZE {
        controls.iter().map(|&h| self.button_size(h)).fold(
            SIZE {
                cx: self.px(75),
                cy: 0,
            },
            |group, button| SIZE {
                cx: group.cx.max(button.cx),
                cy: group.cy.max(button.cy),
            },
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(width: i32, height: i32, stretch: bool) -> FlowItem {
        FlowItem {
            size: SIZE {
                cx: width,
                cy: height,
            },
            stretch,
        }
    }

    #[test]
    fn flow_wraps_whole_items_and_shares_spare_width() {
        let mut area = rect(10, 20, 101, 100);
        let items = [
            item(30, 10, false),
            item(20, 20, true),
            item(20, 15, true),
            item(60, 12, false),
            item(60, 8, false),
        ];
        let positions: Vec<_> = flow(&mut area, &items, 5, 3)
            .into_iter()
            .map(|r| (r.left, r.top, r.right, r.bottom))
            .collect();
        assert_eq!(
            positions,
            [
                (10, 20, 40, 40),
                (45, 20, 75, 40),
                (80, 20, 111, 40),
                (10, 43, 70, 55),
                (10, 58, 70, 66),
            ]
        );
        assert_eq!(area.top, 69);
    }

    #[test]
    fn flow_limits_oversized_items_and_handles_exhausted_space() {
        let mut area = rect(10, 20, 15, 10);
        let positions: Vec<_> = flow(&mut area, &[item(120, 20, true), item(30, 10, false)], 5, 3)
            .into_iter()
            .map(|r| (r.left, r.top, r.right, r.bottom))
            .collect();
        assert_eq!(positions, [(10, 20, 25, 30), (10, 30, 25, 30)]);
        assert_eq!(area.top, 30);
        let mut empty = rect(10, 20, 0, 0);
        assert!(flow(&mut empty, &[], 5, 3).is_empty());
        let positions = flow(&mut empty, &[item(30, 10, true)], 5, 3);
        assert_eq!(positions.len(), 1);
        assert_eq!(
            (
                positions[0].left,
                positions[0].top,
                positions[0].right,
                positions[0].bottom
            ),
            (10, 20, 10, 20)
        );
    }
}

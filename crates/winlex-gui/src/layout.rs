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
    pub fn button_size(&self, hwnd: HWND) -> SIZE {
        let mut size = SIZE { cx: 0, cy: 0 };
        if send(hwnd, BCM_GETIDEALSIZE, 0, &mut size as *mut _ as isize) == 0 {
            size.cx = self.label_width(std::iter::once(text_wide(hwnd)), 0);
            size.cy = self.row();
        }
        size
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
                let label = label.as_ref();
                let length = label.iter().position(|&c| c == 0).unwrap_or(label.len());
                let mut bounds = rect(0, 0, 0, 0);
                if length != 0 {
                    DrawTextW(
                        dc,
                        // DT_MODIFYSTRING is not set; DrawTextW only reads the label.
                        label.as_ptr().cast_mut(),
                        i32::try_from(length).expect("label length fits DrawTextW"),
                        &mut bounds,
                        DT_CALCRECT | DT_SINGLELINE,
                    );
                }
                width = width.max(bounds.right + self.px(24));
            }
            SelectObject(dc, old);
            ReleaseDC(std::ptr::null_mut(), dc);
        }
        width
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
        let size = controls.iter().map(|&h| self.button_size(h)).fold(
            SIZE {
                cx: self.px(75),
                cy: 0,
            },
            |group, button| SIZE {
                cx: group.cx.max(button.cx),
                cy: group.cy.max(button.cy),
            },
        );
        let width = size.cx;
        let available = (area.right - area.left).max(0);
        let columns = ((available + gap) / (width + gap)).max(1);
        let width = width.min(available);
        let row = size.cy;
        let bounds = take_top(
            area,
            (controls.len() as i32 + columns - 1) / columns * (row + gap),
        );
        for (i, h) in controls.into_iter().enumerate() {
            let i = i as i32;
            let y = (bounds.top + i / columns * (row + gap)).min(bounds.bottom);
            positions.push((
                h,
                rect(
                    bounds.left + i % columns * (width + gap),
                    y,
                    width,
                    row.min(bounds.bottom - y),
                ),
            ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_size_falls_back_only_when_query_returns_false() {
        unsafe extern "system" fn successful_query(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _id: usize,
            _data: usize,
        ) -> isize {
            if msg == BCM_GETIDEALSIZE {
                1
            } else {
                unsafe { default_subclass(hwnd, msg, w, l) }
            }
        }
        unsafe {
            let caption = wide("More information… 更多信息");
            let hwnd = CreateWindowExW(
                0,
                w!("STATIC"),
                caption.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance(),
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            for dpi in [96, 168] {
                let m = Metrics {
                    dpi,
                    font: GetStockObject(SYSTEM_FONT),
                    height: 16,
                };
                let size = m.button_size(hwnd);
                assert_eq!(size.cx, m.label_width(std::iter::once(&caption), 0));
                assert_eq!(size.cy, m.row());
            }
            assert_ne!(set_subclass(hwnd, Some(successful_query), 1, 0), 0);
            let m = Metrics {
                dpi: 96,
                font: GetStockObject(SYSTEM_FONT),
                height: 16,
            };
            let size = m.button_size(hwnd);
            assert_eq!((size.cx, size.cy), (0, 0));
            remove_subclass(hwnd, Some(successful_query), 1);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn button_group_uses_maximum_sizes_and_preserves_wrapping() {
        unsafe extern "system" fn ideal_size(
            hwnd: HWND,
            msg: u32,
            w: usize,
            l: isize,
            _id: usize,
            data: usize,
        ) -> isize {
            if msg == BCM_GETIDEALSIZE {
                unsafe {
                    *(l as *mut SIZE) = *(data as *const SIZE);
                }
                1
            } else {
                unsafe { default_subclass(hwnd, msg, w, l) }
            }
        }
        unsafe {
            let controls: Vec<_> = ["Open files", "Reload", "Close source"]
                .into_iter()
                .map(|caption| {
                    let hwnd = CreateWindowExW(
                        0,
                        w!("STATIC"),
                        wide(caption).as_ptr(),
                        WS_POPUP,
                        0,
                        0,
                        0,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        instance(),
                        std::ptr::null(),
                    );
                    assert!(!hwnd.is_null());
                    hwnd
                })
                .collect();
            let sizes = [SIZE { cx: 100, cy: 20 }, SIZE { cx: 140, cy: 35 }];
            for (hwnd, size) in controls.iter().zip(&sizes) {
                assert_ne!(
                    set_subclass(*hwnd, Some(ideal_size), 1, size as *const SIZE as usize),
                    0
                );
            }
            let m = Metrics {
                dpi: 96,
                font: GetStockObject(SYSTEM_FONT),
                height: 16,
            };
            let width = m
                .px(75)
                .max(140)
                .max(m.label_width(std::iter::once(text_wide(controls[2])), 0));
            let height = 35.max(m.row());
            let gap = m.px(4);
            for columns in [3, 2, 1] {
                let mut area = rect(0, 0, width * columns + gap * (columns - 1), 500);
                let mut positions = Vec::new();
                m.buttons(&mut area, controls.iter().copied(), &mut positions);
                assert_eq!(positions.len(), controls.len());
                for (index, (hwnd, bounds)) in positions.iter().enumerate() {
                    assert_eq!(*hwnd, controls[index]);
                    assert_eq!(bounds.right - bounds.left, width);
                    assert_eq!(bounds.bottom - bounds.top, height);
                    assert_eq!(bounds.left, index as i32 % columns * (width + gap));
                    assert_eq!(bounds.top, index as i32 / columns * (height + gap));
                    assert!(bounds.bottom <= area.top);
                }
            }
            for hwnd in controls {
                DestroyWindow(hwnd);
            }
        }
    }

    #[test]
    fn label_measurement_accepts_bounded_utf16() {
        let m = Metrics {
            dpi: 96,
            font: unsafe { GetStockObject(SYSTEM_FONT) },
            height: 16,
        };
        let text: Vec<_> = "中文 label".encode_utf16().collect();
        let mut terminated = text.clone();
        terminated.push(0);
        let width = m.label_width(std::iter::once(&text), 0);
        assert!(width > m.px(24));
        assert_eq!(width, m.label_width(std::iter::once(&terminated), 0));
        assert_eq!(m.label_width(std::iter::once(&[]), 0), m.px(24));
    }
}

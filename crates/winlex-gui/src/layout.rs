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
            size.cx = self.label_width(std::iter::once(text_wide(hwnd)), 0);
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
    pub fn label_width<T: AsRef<[u16]>>(
        &self,
        labels: impl Iterator<Item = T>,
        minimum: i32,
    ) -> i32 {
        labels.fold(self.px(minimum), |width, label| {
            width.max(self.text_width(label.as_ref()) + self.px(24))
        })
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

    #[test]
    fn sizing_limits_keep_the_opposite_edge_fixed_for_every_drag_direction() {
        for edge in WMSZ_LEFT..=WMSZ_BOTTOMRIGHT {
            let mut area = rect(10, 20, 30, 40);
            limit_sizing(&mut area, edge, SIZE { cx: 100, cy: 80 });
            assert_eq!(area.right - area.left, 100);
            assert_eq!(area.bottom - area.top, 80);
            if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
                assert_eq!(area.right, 40);
            } else {
                assert_eq!(area.left, 10);
            }
            if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
                assert_eq!(area.bottom, 60);
            } else {
                assert_eq!(area.top, 20);
            }
            let previous = (area.left, area.top, area.right, area.bottom);
            limit_sizing(&mut area, edge, SIZE { cx: 90, cy: 70 });
            assert_eq!((area.left, area.top, area.right, area.bottom), previous);
        }
    }

    #[test]
    fn button_size_falls_back_when_query_returns_false() {
        unsafe {
            let caption = wide("More information… 更多信息");
            let hwnd = test_window(w!("STATIC"), caption.as_ptr(), WS_POPUP, rect(0, 0, 0, 0));
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
                    test_window(
                        w!("STATIC"),
                        wide(caption).as_ptr(),
                        WS_POPUP,
                        rect(0, 0, 0, 0),
                    )
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
            for (width, rows) in [(500, 1), (300, 2), (150, 3)] {
                let mut area = rect(0, 0, width, 500);
                let mut positions = Vec::new();
                m.buttons(&mut area, controls.iter().copied(), &mut positions);
                assert_eq!(positions.len(), controls.len());
                for control in &controls {
                    assert_eq!(
                        positions.iter().filter(|(hwnd, _)| hwnd == control).count(),
                        1
                    );
                }
                let mut tops = std::collections::HashSet::new();
                for (index, (_, bounds)) in positions.iter().enumerate() {
                    assert_eq!(bounds.right - bounds.left, 140);
                    assert_eq!(bounds.bottom - bounds.top, 35);
                    assert!(bounds.left >= 0 && bounds.right <= width);
                    assert!(bounds.bottom <= area.top);
                    tops.insert(bounds.top);
                    for (_, other) in &positions[index + 1..] {
                        assert!(
                            bounds.right <= other.left
                                || other.right <= bounds.left
                                || bounds.bottom <= other.top
                                || other.bottom <= bounds.top
                        );
                    }
                }
                assert_eq!(tops.len(), rows);
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
        assert_eq!(m.text_width(&text) + m.px(24), width);
        assert_eq!(m.text_width(&[]), 0);
        assert_eq!(width, m.label_width(std::iter::once(&terminated), 0));
        assert_eq!(m.label_width(std::iter::once(&[]), 0), m.px(24));
    }

    #[test]
    fn edit_measurement_preserves_content_height_with_native_borders() {
        unsafe {
            let dpi = crate::dpi::Dpi {
                mode: crate::dpi::Mode::System,
            };
            let m = Metrics {
                dpi: dpi.system(),
                font: GetStockObject(SYSTEM_FONT),
                height: 16,
            };
            for style in [WS_POPUP, WS_POPUP | WS_BORDER] {
                let hwnd = test_window(w!("EDIT"), w!(""), style, rect(0, 0, 100, 100));
                let height = m.edit_height(hwnd, dpi);
                set_bounds(hwnd, rect(0, 0, 100, height)).unwrap();
                let content = client(hwnd);
                assert_eq!(content.bottom - content.top, m.height + m.px(4));
                DestroyWindow(hwnd);
            }
        }
    }
}

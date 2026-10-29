use crate::{dpi::Font, icons::Images, layout, native::*, results::Display};
use std::{ops::Range, ptr};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};

pub struct Visuals {
    heading: Font,
    body: HFONT,
    body_height: i32,
    images: Images,
    icon_size: i32,
    padding: i32,
    gap: i32,
    high_contrast: bool,
}
impl Visuals {
    pub fn new(m: &layout::Metrics) -> anyhow::Result<Self> {
        let mut contrast = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                (&mut contrast as *mut HIGHCONTRASTW).cast(),
                0,
            );
        }
        Ok(Self {
            heading: Font::heading(m.dpi, m.font)?,
            body: m.font,
            body_height: m.height,
            images: Images::new(m.px(16))?,
            icon_size: m.px(16),
            padding: m.px(4),
            gap: m.px(2),
            high_contrast: contrast.dwFlags & HCF_HIGHCONTRASTON != 0,
        })
    }
    pub fn row_height(&self) -> i32 {
        self.heading.height.max(self.icon_size) + self.body_height + self.gap + self.padding * 2
    }
    pub fn draw(&self, dc: HDC, mut area: RECT, display: &Display, selected: bool) {
        unsafe {
            let saved = SaveDC(dc);
            let text = GetTextColor(dc);
            let system = selected || self.high_contrast;
            let accent = if system {
                text
            } else {
                GetSysColor(COLOR_HOTLIGHT)
            };
            area.left += self.padding;
            area.right -= self.padding;
            area.top += self.padding;
            let height = self.heading.height.max(self.icon_size);
            self.images.draw(
                display.icon,
                dc,
                area.left,
                area.top + (height - self.icon_size) / 2,
                system,
                text,
            );
            area.left += self.icon_size + self.padding;
            let mut title = area;
            title.bottom = title.top + height;
            SelectObject(dc, self.heading.handle);
            draw_text(
                dc,
                title,
                &display.title,
                &display.title_matches,
                text,
                accent,
                system,
            );
            area.top = title.bottom + self.gap;
            area.bottom = area.top + self.body_height;
            SelectObject(dc, self.body);
            let secondary = if system {
                text
            } else {
                blend(text, GetSysColor(COLOR_WINDOW))
            };
            draw_text(
                dc,
                area,
                &display.detail,
                &display.detail_matches,
                secondary,
                accent,
                system,
            );
            RestoreDC(dc, saved);
        }
    }
}

fn blend(text: u32, background: u32) -> u32 {
    let channel = |shift: u32| (((text >> shift) & 255) * 3 + ((background >> shift) & 255)) / 4;
    channel(0) | channel(8) << 8 | channel(16) << 16
}

unsafe fn draw_text(
    dc: HDC,
    area: RECT,
    text: &[u16],
    matches: &[Range<usize>],
    normal: u32,
    accent: u32,
    underline: bool,
) {
    if text.is_empty() || area.right <= area.left {
        return;
    }
    unsafe {
        let flags = DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX | DT_VCENTER;
        let mut shown = text.to_vec();
        shown.resize(text.len() + 5, 0);
        SetTextColor(dc, normal);
        DrawTextW(
            dc,
            shown.as_mut_ptr(),
            text.len() as i32,
            &mut { area },
            flags | DT_MODIFYSTRING,
        );
        if matches.is_empty() {
            return;
        }
        let length = shown.iter().position(|&ch| ch == 0).unwrap_or(text.len());
        let visible = if shown[..length] == *text {
            length
        } else {
            length.saturating_sub(3)
        };
        if visible == 0 {
            return;
        }
        let mut advances = vec![0; text.len()];
        let mut size: SIZE = std::mem::zeroed();
        if GetTextExtentExPointW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            i32::MAX,
            ptr::null_mut(),
            advances.as_mut_ptr(),
            &mut size,
        ) == 0
        {
            return;
        }
        for range in matches {
            let end = range.end.min(visible);
            if range.start >= end {
                continue;
            }
            let left = area.left
                + if range.start == 0 {
                    0
                } else {
                    advances[range.start - 1]
                };
            let right = (area.left + advances[end - 1]).min(area.right);
            let saved = SaveDC(dc);
            IntersectClipRect(dc, left, area.top, right, area.bottom);
            SetTextColor(dc, accent);
            DrawTextW(dc, shown.as_mut_ptr(), length as i32, &mut { area }, flags);
            if underline {
                SelectObject(dc, GetStockObject(DC_PEN));
                SetDCPenColor(dc, accent);
                let y = area.top + (area.bottom - area.top + size.cy) / 2 - 1;
                MoveToEx(dc, left, y, ptr::null_mut());
                LineTo(dc, right, y);
            }
            RestoreDC(dc, saved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_font_is_larger_and_bold_at_each_scale() {
        for dpi in [96, 120, 144, 192, 288] {
            let body = Font::new(dpi).unwrap();
            let heading = Font::heading(dpi, body.handle).unwrap();
            assert!(heading.height > body.height);
            let mut definition: LOGFONTW = unsafe { std::mem::zeroed() };
            unsafe {
                assert_ne!(
                    GetObjectW(
                        heading.handle,
                        std::mem::size_of::<LOGFONTW>() as i32,
                        (&mut definition as *mut LOGFONTW).cast()
                    ),
                    0
                );
            }
            assert_eq!(definition.lfWeight, FW_BOLD as i32);
        }
    }

    #[test]
    fn matched_text_remains_emphasized_in_system_colors_and_when_truncated() {
        unsafe {
            let screen = GetDC(ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, 200, 80);
            ReleaseDC(ptr::null_mut(), screen);
            assert!(!dc.is_null() && !bitmap.is_null());
            let old_bitmap = SelectObject(dc, bitmap);
            let old_font = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
            let area = layout::rect(0, 0, 100, 40);
            SetBkMode(dc, TRANSPARENT as i32);
            let text: Vec<_> = "Window😀WindowWindow".encode_utf16().collect();
            for system in [false, true] {
                FillRect(dc, &area, GetSysColorBrush(COLOR_WINDOW));
                let normal = GetSysColor(COLOR_WINDOWTEXT);
                let accent = if system {
                    normal
                } else {
                    GetSysColor(COLOR_HOTLIGHT)
                };
                draw_text(
                    dc,
                    area,
                    &text,
                    &[0..6, 8..text.len()],
                    normal,
                    accent,
                    system,
                );
                assert!((0..100).any(|x| (0..40).any(|y| GetPixel(dc, x, y) == accent)));
            }
            SelectObject(dc, old_font);
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
        }
    }
}

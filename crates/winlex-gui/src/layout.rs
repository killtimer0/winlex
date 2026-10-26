use crate::native::*;

pub fn scale(value: i32, dpi: u32) -> i32 {
    unsafe { MulDiv(value, dpi as i32, 96) }
}
pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w.max(0),
        bottom: y + h.max(0),
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

pub fn tile(area: RECT, arrangement: Arrangement, index: usize, count: usize) -> RECT {
    let index = index as i32;
    let count = count as i32;
    if arrangement == Arrangement::Columns {
        let x = area.right * index / count;
        rect(x, 0, area.right * (index + 1) / count - x, area.bottom)
    } else {
        let y = area.bottom * index / count;
        rect(0, y, area.right, area.bottom * (index + 1) / count - y)
    }
}

pub struct FrameLayout {
    pub edit: RECT,
    pub button: RECT,
    pub results: RECT,
    pub splitter: RECT,
    pub documents: RECT,
    pub mdi: RECT,
}
pub fn frame(size: RECT, dpi: u32, font_height: i32, sidebar: i32) -> FrameLayout {
    let margin = scale(8, dpi);
    let gap = scale(6, dpi);
    let row = font_height + scale(12, dpi);
    let button = scale(88, dpi).min((size.right - margin * 2).max(0));
    let y = margin + row + gap;
    let width = scale(sidebar, dpi).min((size.right - scale(160, dpi)).max(0));
    let bar = scale(5, dpi);
    let workspace_x = width + bar;
    let workspace_width = (size.right - workspace_x).max(0);
    let tab_height = row + scale(4, dpi);
    FrameLayout {
        edit: rect(margin, margin, size.right - margin * 2 - gap - button, row),
        button: rect(size.right - margin - button, margin, button, row),
        results: rect(0, y, width, size.bottom - y),
        splitter: rect(width, y, bar, size.bottom - y),
        documents: rect(workspace_x, y, workspace_width, tab_height),
        mdi: rect(
            workspace_x,
            y + tab_height,
            workspace_width,
            size.bottom - y - tab_height,
        ),
    }
}

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

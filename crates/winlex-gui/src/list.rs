use crate::{
    dpi::Font,
    icons::{Icon, Images},
    layout,
    native::*,
    platform::WindowsVersion,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    ptr,
};
use windows_sys::Win32::UI::Shell::GetWindowSubclass;

#[derive(Default)]
pub struct Display {
    pub title: Vec<u16>,
    pub detail: Vec<u16>,
    pub title_matches: Vec<Range<usize>>,
    pub detail_matches: Vec<Range<usize>>,
    pub icon: Option<Icon>,
    pub detail_folds: Vec<Fold>,
    pub tooltip: Vec<u16>,
}

pub struct Fold {
    pub range: Range<usize>,
    pub kind: FoldKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Guid,
    Path,
    Text,
}

fn fold_text(
    text: &[u16],
    matches: &[Range<usize>],
    folds: &[Fold],
    width: i32,
    measure: impl Fn(&[u16]) -> i32,
) -> (Vec<u16>, Vec<Range<usize>>) {
    let mut shown = text.to_vec();
    let mut highlighted = matches.to_vec();
    let mut replacements: Vec<(Range<usize>, usize)> = Vec::new();
    let mut ordered: Vec<&Fold> = folds.iter().collect();
    ordered.sort_by_key(|fold| fold.range.start);
    let mut ordinary = Vec::new();
    let mut start = 0;
    for fold in &ordered {
        if start < fold.range.start {
            ordinary.push(Fold {
                range: start..fold.range.start,
                kind: FoldKind::Text,
            });
        }
        start = fold.range.end;
    }
    if start < text.len() {
        ordinary.push(Fold {
            range: start..text.len(),
            kind: FoldKind::Text,
        });
    }
    for kind in [FoldKind::Guid, FoldKind::Path, FoldKind::Text] {
        for fold in folds
            .iter()
            .chain(ordinary.iter().rev())
            .filter(|fold| fold.kind == kind)
        {
            if measure(&shown) <= width {
                return (shown, highlighted);
            }
            let shift: usize = replacements
                .iter()
                .filter(|(range, _)| range.end <= fold.range.start)
                .map(|(range, length)| range.len() - length)
                .sum();
            let range = fold.range.start - shift..fold.range.end - shift;
            let value = &shown[range.clone()];
            let replacement = match kind {
                FoldKind::Guid => "{…}".encode_utf16().collect::<Vec<_>>(),
                FoldKind::Text => {
                    let outside = measure(&shown[..range.start]) + measure(&shown[range.end..]);
                    let mut end = value.len();
                    loop {
                        let candidate: Vec<u16> = value[..end]
                            .iter()
                            .copied()
                            .chain("…".encode_utf16())
                            .collect();
                        if outside + measure(&candidate) <= width || end == 0 {
                            break candidate;
                        }
                        end -= 1;
                        if end > 0 && (0xDC00..=0xDFFF).contains(&value[end]) {
                            end -= 1;
                        }
                    }
                }
                FoldKind::Path => {
                    let separator = |ch: &u16| *ch == b'\\' as u16 || *ch == b'/' as u16;
                    let Some(last) = value.iter().rposition(separator) else {
                        continue;
                    };
                    let first = if value.starts_with(&[b'\\' as u16, b'\\' as u16]) {
                        1
                    } else {
                        value.iter().position(separator).unwrap()
                    };
                    if first >= last {
                        continue;
                    }
                    let outside = measure(&shown[..range.start]) + measure(&shown[range.end..]);
                    let mut end = first + 1;
                    loop {
                        let candidate: Vec<u16> = value[..first + 1]
                            .iter()
                            .copied()
                            .chain("…".encode_utf16())
                            .chain(value[end..].iter().copied())
                            .collect();
                        if measure(&candidate) + outside <= width || end == last {
                            break candidate;
                        }
                        end += 1;
                        if end < last && (0xDC00..=0xDFFF).contains(&value[end]) {
                            end += 1;
                        }
                    }
                }
            };
            if replacement.len() >= range.len() {
                continue;
            }
            let prefix = value
                .iter()
                .zip(&replacement)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = value[prefix..]
                .iter()
                .rev()
                .zip(replacement[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let changed = range.start + prefix..range.end - suffix;
            let removed = range.len() - replacement.len();
            highlighted = highlighted
                .into_iter()
                .flat_map(|matched| {
                    let before = matched.start..matched.end.min(changed.start);
                    let after = matched.start.max(changed.end) - removed
                        ..matched.end.saturating_sub(removed);
                    [before, after]
                        .into_iter()
                        .filter(|part| part.start < part.end)
                })
                .collect();
            replacements.push((fold.range.clone(), replacement.len()));
            shown.splice(range, replacement);
        }
    }
    (shown, highlighted)
}

pub fn tooltip_field(text: &mut Vec<u16>, label: u16, value: impl IntoIterator<Item = u16>) {
    text.extend("\r\n".encode_utf16());
    text.extend(crate::resources::text(label).encode_utf16());
    text.extend(value);
}

type ReadRow = fn(usize, &mut dyn FnMut(&Display));

pub struct List {
    pub hwnd: HWND,
    visuals: RefCell<Option<Visuals>>,
    row: ReadRow,
    text: fn(usize) -> Vec<u16>,
    open: Option<fn(isize, bool)>,
    native_background: bool,
    tooltip_item: Cell<i32>,
}
impl List {
    pub fn new(
        frame: HWND,
        id: u16,
        row: ReadRow,
        text: fn(usize) -> Vec<u16>,
        open: Option<fn(isize, bool)>,
        version: WindowsVersion,
    ) -> anyhow::Result<Self> {
        let hwnd = child(
            frame,
            w!("SysListView32"),
            &[0],
            WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_NOCOLUMNHEADER,
            id,
        )?;
        unsafe {
            SetWindowTheme(hwnd, w!("Explorer"), ptr::null());
        }
        if unsafe { set_subclass(hwnd, Some(list_proc), 1, palette(hwnd)) } == 0 {
            anyhow::bail!("Subclass list control {id} failed");
        }
        list_column(hwnd, 0, "", 100);
        send(
            hwnd,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            0,
            (LVS_EX_FULLROWSELECT | LVS_EX_INFOTIP | LVS_EX_DOUBLEBUFFER) as isize,
        );
        Ok(Self {
            hwnd,
            visuals: RefCell::new(None),
            row,
            text,
            open,
            native_background: version.at_least(WindowsVersion::VISTA),
            tooltip_item: Cell::new(-1),
        })
    }
    pub fn prepare(&self, m: &layout::Metrics) -> anyhow::Result<()> {
        let visuals = Visuals::new(m)?;
        unsafe {
            let image = ImageList_Create(1, visuals.row_height(), ILC_COLOR32, 1, 1);
            if image == 0 {
                return Err(error("List row height"));
            }
            let old = send(
                self.hwnd,
                LVM_SETIMAGELIST,
                LVSIL_SMALL as usize,
                image as isize,
            );
            if old != 0 {
                ImageList_Destroy(old as _);
            }
        }
        self.visuals.replace(Some(visuals));
        Ok(())
    }
    pub fn layout(&self, area: RECT) {
        place(self.hwnd, area);
        fit(self.hwnd);
    }
    pub fn recolor(&self) -> anyhow::Result<()> {
        refresh_palette(self.hwnd);
        if let Some(visuals) = self.visuals.borrow_mut().as_mut() {
            visuals.images = Images::new(visuals.icon_size)?;
        }
        Ok(())
    }
    pub fn refresh_tooltip(&self) {
        let item = self.tooltip_item.get();
        if item < 0 {
            return;
        }
        let mut text = Vec::new();
        (self.row)(item as usize, &mut |row| {
            text.extend_from_slice(&row.tooltip)
        });
        text.push(0);
        let info = LVSETINFOTIP {
            cbSize: std::mem::size_of::<LVSETINFOTIP>() as u32,
            pszText: text.as_mut_ptr(),
            iItem: item,
            ..unsafe { std::mem::zeroed() }
        };
        send(self.hwnd, LVM_SETINFOTIP, 0, &info as *const _ as isize);
        let tooltip = send(self.hwnd, LVM_GETTOOLTIPS, 0, 0) as HWND;
        if !tooltip.is_null() {
            send(tooltip, TTM_POP, 0, 0);
        }
    }
    pub unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if hdr.hwndFrom != self.hwnd {
                return None;
            }
            match hdr.code {
                LVN_GETDISPINFOW | LVN_GETINFOTIPW => {
                    let (row, buffer, capacity) = if hdr.code == LVN_GETDISPINFOW {
                        let i = &mut (*(l as *mut NMLVDISPINFOW)).item;
                        if i.mask & LVIF_TEXT == 0 {
                            return Some(0);
                        }
                        (i.iItem, i.pszText, i.cchTextMax)
                    } else {
                        let i = &mut *(l as *mut NMLVGETINFOTIPW);
                        self.tooltip_item.set(i.iItem);
                        (i.iItem, i.pszText, i.cchTextMax)
                    };
                    if !buffer.is_null() && capacity > 0 {
                        *buffer = 0;
                        let copy = |text: &[u16]| {
                            let mut count = text.len().min(capacity as usize - 1);
                            if count < text.len()
                                && count > 0
                                && (0xD800..=0xDBFF).contains(&text[count - 1])
                                && (0xDC00..=0xDFFF).contains(&text[count])
                            {
                                count -= 1;
                            }
                            ptr::copy_nonoverlapping(text.as_ptr(), buffer, count);
                            *buffer.add(count) = 0;
                        };
                        if hdr.code == LVN_GETINFOTIPW {
                            (self.row)(row as usize, &mut |display| copy(&display.tooltip));
                        } else {
                            copy(&(self.text)(row as usize));
                        }
                    }
                    Some(0)
                }
                NM_RCLICK => {
                    let mut point = (*(l as *const NMITEMACTIVATE)).ptAction;
                    ClientToScreen(self.hwnd, &mut point);
                    send(
                        GetParent(self.hwnd),
                        WM_CONTEXTMENU,
                        self.hwnd as usize,
                        ((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize,
                    );
                    Some(1)
                }
                NM_CLICK | NM_DBLCLK => {
                    let i = &*(l as *const NMITEMACTIVATE);
                    if i.iItem >= 0
                        && let Some(open) = self.open
                    {
                        open(i.iItem as isize, hdr.code == NM_DBLCLK);
                    }
                    Some(0)
                }
                NM_CUSTOMDRAW => {
                    let d = &*(l as *const NMLVCUSTOMDRAW);
                    match d.nmcd.dwDrawStage {
                        CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as isize),
                        CDDS_ITEMPREPAINT if self.native_background => {
                            Some(CDRF_NOTIFYSUBITEMDRAW as isize)
                        }
                        stage if stage == CDDS_ITEMPREPAINT | CDDS_SUBITEM => {
                            let area = d.nmcd.rc;
                            if area.left == 0
                                && area.top == 0
                                && area.right == 0
                                && area.bottom == 0
                            {
                                return Some(CDRF_DODEFAULT as isize);
                            }
                            Some((CDRF_DOERASE | CDRF_NOTIFYPOSTPAINT) as isize)
                        }
                        stage
                            if stage == CDDS_ITEMPREPAINT
                                || stage == CDDS_ITEMPOSTPAINT | CDDS_SUBITEM =>
                        {
                            // XP custom-draw notifications do not supply usable item bounds.
                            let mut area = RECT {
                                left: LVIR_BOUNDS as i32,
                                ..std::mem::zeroed()
                            };
                            if send(
                                self.hwnd,
                                LVM_GETITEMRECT,
                                d.nmcd.dwItemSpec,
                                &mut area as *mut RECT as isize,
                            ) == 0
                            {
                                return Some(CDRF_DODEFAULT as isize);
                            }
                            let saved = SaveDC(d.nmcd.hdc);
                            IntersectClipRect(
                                d.nmcd.hdc,
                                area.left,
                                area.top,
                                area.right,
                                area.bottom,
                            );
                            let visuals = self.visuals.borrow();
                            let selected = selected(self.hwnd, d.nmcd.dwItemSpec);
                            if !self.native_background {
                                background(self.hwnd, d.nmcd.hdc, &area, selected);
                            }
                            SetBkMode(d.nmcd.hdc, TRANSPARENT as i32);
                            if let Some(visuals) = visuals.as_ref() {
                                let mut palette = 0;
                                GetWindowSubclass(self.hwnd, Some(list_proc), 1, &mut palette);
                                let system = palette & HIGH_CONTRAST != 0
                                    || selected
                                        && (!self.native_background || palette & THEMED == 0);
                                (self.row)(d.nmcd.dwItemSpec, &mut |row| {
                                    visuals.draw(d.nmcd.hdc, area, row, system);
                                });
                            }
                            if !self.native_background
                                && d.nmcd.uItemState & CDIS_FOCUS != 0
                                && GetFocus() == self.hwnd
                                && send(self.hwnd, WM_QUERYUISTATE, 0, 0) & UISF_HIDEFOCUS as isize
                                    == 0
                            {
                                DrawFocusRect(d.nmcd.hdc, &area);
                            }
                            RestoreDC(d.nmcd.hdc, saved);
                            Some(if self.native_background {
                                CDRF_DODEFAULT as isize
                            } else {
                                CDRF_SKIPDEFAULT as isize
                            })
                        }
                        _ => Some(CDRF_DODEFAULT as isize),
                    }
                }
                _ => None,
            }
        }
    }
}
pub fn fit(hwnd: HWND) {
    send(
        hwnd,
        LVM_SETCOLUMNWIDTH,
        0,
        client(hwnd).right.max(1) as isize,
    );
    unsafe {
        ShowScrollBar(hwnd, SB_HORZ, 0);
    }
}
pub struct State {
    pub indices: [isize; 4],
    previous: [isize; 2],
    top: isize,
    offset: i32,
}
impl State {
    pub fn capture(hwnd: HWND) -> Self {
        let top = send(hwnd, LVM_GETTOPINDEX, 0, 0);
        let mut rect = RECT {
            left: LVIR_BOUNDS as i32,
            ..unsafe { std::mem::zeroed() }
        };
        let has_top = send(
            hwnd,
            LVM_GETITEMRECT,
            top as usize,
            &mut rect as *mut _ as isize,
        ) != 0;
        let indices = [
            send(hwnd, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize),
            send(hwnd, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED as isize),
            send(hwnd, LVM_GETSELECTIONMARK, 0, 0),
            if has_top && top > 0 { top } else { -1 },
        ];
        Self {
            indices,
            previous: [indices[0], indices[1]],
            top,
            offset: rect.top,
        }
    }
    pub fn apply(&self, hwnd: HWND, count: usize) {
        let visible = unsafe { IsWindowVisible(hwnd) != 0 };
        if visible {
            send(hwnd, WM_SETREDRAW, 0, 0);
        }
        let mut state = LVITEMW {
            stateMask: LVIS_SELECTED | LVIS_FOCUSED,
            ..unsafe { std::mem::zeroed() }
        };
        for index in self.previous {
            if index >= 0 {
                send(
                    hwnd,
                    LVM_SETITEMSTATE,
                    index as usize,
                    &state as *const _ as isize,
                );
            }
        }
        send(
            hwnd,
            LVM_SETITEMCOUNT,
            count,
            (LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL) as isize,
        );
        for (index, flag) in [
            (self.indices[0], LVIS_SELECTED),
            (self.indices[1], LVIS_FOCUSED),
        ] {
            if index >= 0 {
                state.stateMask = flag;
                state.state = flag;
                send(
                    hwnd,
                    LVM_SETITEMSTATE,
                    index as usize,
                    &state as *const _ as isize,
                );
            }
        }
        send(hwnd, LVM_SETSELECTIONMARK, 0, self.indices[2]);
        if self.indices[3] >= 0 {
            send(hwnd, LVM_ENSUREVISIBLE, self.indices[3] as usize, 0);
            let mut now = RECT {
                left: LVIR_BOUNDS as i32,
                ..unsafe { std::mem::zeroed() }
            };
            if send(
                hwnd,
                LVM_GETITEMRECT,
                self.indices[3] as usize,
                &mut now as *mut _ as isize,
            ) != 0
            {
                send(hwnd, LVM_SCROLL, 0, (now.top - self.offset) as isize);
            }
        } else if self.top == 0 && count != 0 {
            send(hwnd, LVM_ENSUREVISIBLE, 0, 0);
        }
        if visible {
            send(hwnd, WM_SETREDRAW, 1, 0);
        }
        fit(hwnd);
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
}
fn selected(hwnd: HWND, item: usize) -> bool {
    send(hwnd, LVM_GETITEMSTATE, item, LVIS_SELECTED as isize) != 0
        && unsafe {
            GetFocus() == hwnd || GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & LVS_SHOWSELALWAYS != 0
        }
}
unsafe fn background(hwnd: HWND, dc: HDC, area: &RECT, selected: bool) {
    unsafe {
        let focused = GetFocus() == hwnd;
        let (background, foreground) = match (selected, focused) {
            (true, true) => (COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT),
            (true, false) => (COLOR_BTNFACE, COLOR_WINDOWTEXT),
            _ => (COLOR_WINDOW, COLOR_WINDOWTEXT),
        };
        FillRect(dc, area, GetSysColorBrush(background));
        SetTextColor(dc, GetSysColor(foreground));
    }
}
fn themed_items(hwnd: HWND) -> bool {
    unsafe {
        let theme = OpenThemeData(hwnd, w!("ListView"));
        if theme == 0 {
            return false;
        }
        let supported = IsThemePartDefined(theme, LVP_LISTITEM, 0) != 0;
        CloseThemeData(theme);
        supported
    }
}
const THEMED: usize = 1;
const HIGH_CONTRAST: usize = 2;
fn palette(hwnd: HWND) -> usize {
    (usize::from(themed_items(hwnd)) * THEMED) | (usize::from(high_contrast()) * HIGH_CONTRAST)
}
fn refresh_palette(hwnd: HWND) {
    unsafe {
        set_subclass(hwnd, Some(list_proc), 1, palette(hwnd));
        InvalidateRect(hwnd, ptr::null(), 0);
    }
}
unsafe extern "system" fn list_proc(
    h: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        if matches!(msg, WM_THEMECHANGED | WM_SETTINGCHANGE | WM_SYSCOLORCHANGE) {
            let result = default_subclass(h, msg, w, l);
            refresh_palette(h);
            return result;
        }
        if msg == WM_CONTEXTMENU {
            return send(GetParent(h), msg, h as usize, l);
        }
        if (msg == WM_KEYDOWN || msg == WM_KEYUP)
            && (w == VK_APPS as usize || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0)
        {
            return if msg == WM_KEYDOWN {
                send(GetParent(h), WM_CONTEXTMENU, h as usize, -1)
            } else {
                0
            };
        }
        if msg == WM_NCDESTROY {
            let image = send(h, LVM_SETIMAGELIST, LVSIL_SMALL as usize, 0);
            if image != 0 {
                ImageList_Destroy(image as _);
            }
            remove_subclass(h, Some(list_proc), 1);
        }
        default_subclass(h, msg, w, l)
    }
}

struct Visuals {
    heading: Font,
    body: HFONT,
    body_height: i32,
    images: Images,
    icon_size: i32,
    padding: i32,
    gap: i32,
}
impl Visuals {
    fn new(m: &layout::Metrics) -> anyhow::Result<Self> {
        Ok(Self {
            heading: Font::heading(m.dpi, m.font)?,
            body: m.font,
            body_height: m.height,
            images: Images::new(m.px(16))?,
            icon_size: m.px(16),
            padding: m.px(4),
            gap: m.px(2),
        })
    }
    fn row_height(&self) -> i32 {
        self.heading.height.max(self.icon_size) + self.body_height + self.gap + self.padding * 2
    }
    fn draw(&self, dc: HDC, mut area: RECT, display: &Display, system: bool) {
        unsafe {
            let text = if system {
                GetTextColor(dc)
            } else {
                GetSysColor(COLOR_WINDOWTEXT)
            };
            let accent = if system {
                text
            } else {
                GetSysColor(COLOR_HOTLIGHT)
            };
            area.left += self.padding;
            area.right -= self.padding;
            area.top += self.padding;
            let height = self.heading.height.max(self.icon_size);
            if let Some(icon) = display.icon {
                self.images.draw(
                    icon,
                    dc,
                    area.left,
                    area.top + (height - self.icon_size) / 2,
                    system,
                    text,
                );
                area.left += self.icon_size + self.padding;
            }
            let secondary = if system {
                text
            } else {
                blend(text, GetSysColor(COLOR_WINDOW))
            };
            for (index, (value, matches)) in [
                (&display.title, &display.title_matches),
                (&display.detail, &display.detail_matches),
            ]
            .into_iter()
            .enumerate()
            {
                let (font, height, color) = if index == 0 {
                    (self.heading.handle, height, text)
                } else {
                    (self.body, self.body_height, secondary)
                };
                area.bottom = area.top + height;
                SelectObject(dc, font);
                if index == 1 && !display.detail_folds.is_empty() {
                    let (shown, matches) = fold_text(
                        value,
                        matches,
                        &display.detail_folds,
                        (area.right - area.left).max(0),
                        |text| {
                            let mut size: SIZE = std::mem::zeroed();
                            GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
                            size.cx
                        },
                    );
                    draw_text(dc, area, &shown, &matches, color, accent, system);
                } else {
                    draw_text(dc, area, value, matches, color, accent, system);
                }
                area.top = area.bottom + self.gap;
            }
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
    fn detail_folds_follow_priority_and_preserve_surviving_matches() {
        let text: Vec<u16> = "label {12345678-9abc-def0-1234-56789abcdef0} C:\\long\\folder\\tail"
            .encode_utf16()
            .collect();
        let folds = [
            Fold {
                range: 45..text.len(),
                kind: FoldKind::Path,
            },
            Fold {
                range: 6..44,
                kind: FoldKind::Guid,
            },
        ];
        let measure = |value: &[u16]| value.len() as i32;
        let (wide, matches) = fold_text(&text, &[0..5, 6..44], &folds, 100, measure);
        assert_eq!(wide, text);
        assert_eq!(matches, [0..5, 6..44]);
        let (medium, matches) = fold_text(&text, &[0..5, 6..44], &folds, 40, measure);
        assert_eq!(
            String::from_utf16(&medium).unwrap(),
            "label {…} C:\\long\\folder\\tail"
        );
        assert_eq!(matches, [0..5, 6..7, 8..9]);
        let (narrow, _) = fold_text(&text, &[], &folds, 20, measure);
        assert!(String::from_utf16(&narrow).unwrap().ends_with("\\tail"));
        assert!(narrow.len() <= 20);
        let (small, _) = fold_text(&text, &[], &folds, 14, measure);
        assert!(String::from_utf16(&small).unwrap().ends_with("\\tail"));
        assert!(small.len() <= 14);
        let (shown, matches) = fold_text(
            &text,
            &[0..5, text.len() - 4..text.len()],
            &folds,
            16,
            measure,
        );
        assert_eq!(&shown[matches[0].clone()], &text[..matches[0].len()]);
        assert_eq!(
            String::from_utf16(&shown[matches.last().unwrap().clone()]).unwrap(),
            "tail"
        );
    }

    #[test]
    fn folded_paths_preserve_unc_prefixes_and_utf16_boundaries() {
        for path in [r"\\server\😀目录\文件.winmd", r"\\?\C:\😀目录\文件.winmd"] {
            let text: Vec<u16> = path.encode_utf16().collect();
            let folds = [Fold {
                range: 0..text.len(),
                kind: FoldKind::Path,
            }];
            for width in 0..text.len() {
                let (shown, _) =
                    fold_text(&text, &[], &folds, width as i32, |text| text.len() as i32);
                let shown = String::from_utf16(&shown).unwrap();
                assert!(shown.starts_with(r"\\"));
                assert!(shown.ends_with(r"\文件.winmd"));
            }
        }
    }

    #[test]
    fn infotip_text_is_separate_and_respects_utf16_buffer_boundaries() {
        let list = List {
            hwnd: ptr::null_mut(),
            visuals: RefCell::new(None),
            row: |_, visit| {
                visit(&Display {
                    tooltip: vec![65, 0xD83D, 0xDE00, 0xD800, 66],
                    ..Default::default()
                })
            },
            text: |_| vec![90],
            open: None,
            native_background: true,
            tooltip_item: Cell::new(-1),
        };
        for (capacity, expected) in [
            (1, vec![0]),
            (3, vec![65, 0]),
            (4, vec![65, 0xD83D, 0xDE00, 0]),
            (6, vec![65, 0xD83D, 0xDE00, 0xD800, 66, 0]),
        ] {
            let mut buffer = [0xFFFF; 8];
            let mut info = NMLVGETINFOTIPW {
                hdr: NMHDR {
                    code: LVN_GETINFOTIPW,
                    ..unsafe { std::mem::zeroed() }
                },
                pszText: buffer.as_mut_ptr(),
                cchTextMax: capacity,
                ..unsafe { std::mem::zeroed() }
            };
            unsafe {
                list.notify(&mut info as *mut _ as isize);
            }
            assert_eq!(&buffer[..expected.len()], &expected);
            assert!(
                buffer[expected.len()..]
                    .iter()
                    .all(|&value| value == 0xFFFF)
            );
        }
    }

    #[test]
    fn selection_palette_tracks_control_theme_changes() {
        unsafe {
            let frame = CreateWindowExW(
                0,
                w!("STATIC"),
                w!(""),
                0,
                0,
                0,
                320,
                200,
                ptr::null_mut(),
                ptr::null_mut(),
                instance(),
                ptr::null(),
            );
            assert!(!frame.is_null());
            let list = List::new(
                frame,
                1,
                |_, _| {},
                |_| Vec::new(),
                None,
                WindowsVersion::VISTA,
            )
            .unwrap();
            let cached = || {
                let mut value = 0;
                assert_ne!(
                    GetWindowSubclass(list.hwnd, Some(list_proc), 1, &mut value),
                    0
                );
                value
            };
            let original = cached();
            assert_eq!(SetWindowTheme(list.hwnd, w!(""), w!("")), 0);
            assert_eq!(cached() & THEMED, 0);
            assert_eq!(SetWindowTheme(list.hwnd, w!("Explorer"), ptr::null()), 0);
            assert_eq!(cached(), original);
            DestroyWindow(frame);
        }
    }

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

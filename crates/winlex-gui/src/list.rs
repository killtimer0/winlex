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
    pub detail: TextLine,
    pub title_matches: Vec<Range<usize>>,
    pub icon: Option<Icon>,
    pub tooltip: Vec<u16>,
}

#[derive(Default)]
pub struct TextLine {
    segments: Vec<TextSegment>,
}

pub struct TextSegment {
    pub text: Vec<u16>,
    pub matches: Vec<Range<usize>>,
    kind: FoldKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Guid,
    Path,
    Text,
}

impl TextLine {
    pub fn push(&mut self, segment: TextSegment) {
        if segment.text.is_empty() {
            return;
        }
        if segment.kind == FoldKind::Text
            && let Some(last) = self
                .segments
                .last_mut()
                .filter(|s| s.kind == FoldKind::Text)
        {
            let offset = last.text.len();
            last.matches.extend(
                segment
                    .matches
                    .into_iter()
                    .map(|r| offset + r.start..offset + r.end),
            );
            last.text.extend(segment.text);
        } else {
            self.segments.push(segment);
        }
        let matches = &mut self.segments.last_mut().unwrap().matches;
        matches.sort_unstable_by_key(|r| (r.start, r.end));
    }
    pub fn field(&mut self, segment: TextSegment) {
        if segment.text.is_empty() {
            return;
        }
        if !self.segments.is_empty() {
            self.push(TextSegment::plain(" · "));
        }
        self.push(segment);
    }
    pub fn text(&self) -> Vec<u16> {
        self.segments
            .iter()
            .flat_map(|s| s.text.iter().copied())
            .collect()
    }
    pub fn fit(
        &self,
        width: i32,
        measure: impl Fn(&[u16]) -> i32,
    ) -> (Vec<u16>, Vec<Range<usize>>) {
        let mut line = self.text();
        let mut cuts = vec![0..0; self.segments.len()];
        for kind in [FoldKind::Guid, FoldKind::Path, FoldKind::Text] {
            let mut order: Vec<_> = (0..cuts.len())
                .filter(|&i| self.segments[i].kind == kind)
                .collect();
            if kind == FoldKind::Text {
                order.reverse();
            }
            for index in order {
                if measure(&line) <= width {
                    break;
                }
                let start: usize = self.segments[..index]
                    .iter()
                    .zip(&cuts[..index])
                    .map(|(s, cut)| s.text.len() - cut.len() + usize::from(!cut.is_empty()))
                    .sum();
                let end = start + self.segments[index].text.len();
                let outside = measure(&line[..start]) + measure(&line[end..]);
                if let Some((replacement, cut)) =
                    self.segments[index].fold(width - outside, &measure)
                {
                    line.splice(start..end, replacement);
                    cuts[index] = cut;
                }
            }
        }
        let mut highlighted: Vec<Range<usize>> = Vec::new();
        let mut offset = 0;
        for (segment, cut) in self.segments.iter().zip(cuts) {
            let marker = usize::from(!cut.is_empty());
            for (kept, output) in [
                (0..cut.start, offset),
                (cut.end..segment.text.len(), offset + cut.start + marker),
            ] {
                for matched in &segment.matches {
                    let start = kept.start.max(matched.start);
                    let end = kept.end.min(matched.end);
                    if start < end {
                        let range = output + start - kept.start..output + end - kept.start;
                        if let Some(last) = highlighted.last_mut()
                            && range.start <= last.end
                        {
                            last.end = last.end.max(range.end);
                        } else {
                            highlighted.push(range);
                        }
                    }
                }
            }
            offset += segment.text.len() - cut.len() + marker;
        }
        (line, highlighted)
    }
}
impl TextSegment {
    pub fn new(text: impl IntoIterator<Item = u16>, kind: FoldKind) -> Self {
        Self {
            text: text.into_iter().collect(),
            matches: Vec::new(),
            kind,
        }
    }
    pub fn plain(text: &str) -> Self {
        Self::new(text.encode_utf16(), FoldKind::Text)
    }
    pub fn highlight(&mut self) {
        self.matches.push(0..self.text.len());
    }
    /// Returns the display text and the segment-local range replaced by the ellipsis.
    pub fn fold(
        &self,
        width: i32,
        measure: &impl Fn(&[u16]) -> i32,
    ) -> Option<(Vec<u16>, Range<usize>)> {
        let value = &self.text;
        if measure(value) <= width {
            return None;
        }
        if self.kind == FoldKind::Guid {
            return (value.len() > 3).then(|| ("{…}".encode_utf16().collect(), 1..value.len() - 1));
        }
        let (mut omitted, limit) = if self.kind == FoldKind::Path {
            let separator = |ch: &u16| *ch == b'\\' as u16 || *ch == b'/' as u16;
            let last = value.iter().rposition(separator)?;
            let first = if value.starts_with(&[b'\\' as u16, b'\\' as u16]) {
                1
            } else {
                value.iter().position(separator)?
            };
            if first >= last {
                return None;
            }
            (first + 1..first + 1, last)
        } else {
            (value.len()..value.len(), 0)
        };
        loop {
            let candidate: Vec<_> = value[..omitted.start]
                .iter()
                .copied()
                .chain("…".encode_utf16())
                .chain(value[omitted.end..].iter().copied())
                .collect();
            let done = if self.kind == FoldKind::Path {
                omitted.end == limit
            } else {
                omitted.start == 0
            };
            if measure(&candidate) <= width || done {
                return (candidate.len() < value.len()).then_some((candidate, omitted));
            }
            if self.kind == FoldKind::Path {
                omitted.end += 1;
                if omitted.end < limit && (0xDC00..=0xDFFF).contains(&value[omitted.end]) {
                    omitted.end += 1;
                }
            } else {
                omitted.start -= 1;
                if omitted.start > 0 && (0xDC00..=0xDFFF).contains(&value[omitted.start]) {
                    omitted.start -= 1;
                }
            }
        }
    }
}

pub fn tooltip_field(text: &mut Vec<u16>, label: u16, value: impl IntoIterator<Item = u16>) {
    text.extend("\r\n".encode_utf16());
    text.extend(crate::resources::text(label).encode_utf16());
    text.extend(value);
}

type RowReader = Box<dyn Fn(usize, &mut dyn FnMut(&Display))>;
type TextReader = Box<dyn Fn(usize) -> Vec<u16>>;
type OpenHandler = Box<dyn Fn(usize, bool)>;

pub struct List {
    pub hwnd: HWND,
    visuals: RefCell<Option<Visuals>>,
    row: RowReader,
    text: TextReader,
    open: Option<OpenHandler>,
    native_background: bool,
    tooltip_item: Cell<i32>,
}
impl List {
    pub fn new(
        frame: HWND,
        id: u16,
        row: impl Fn(usize, &mut dyn FnMut(&Display)) + 'static,
        text: impl Fn(usize) -> Vec<u16> + 'static,
        open: Option<OpenHandler>,
        version: WindowsVersion,
    ) -> anyhow::Result<Self> {
        let hwnd = unsafe {
            child(
                frame,
                w!("SysListView32"),
                &[0],
                WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_NOCOLUMNHEADER,
                id,
            )
        }?;
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
            row: Box::new(row),
            text: Box::new(text),
            open,
            native_background: version >= WindowsVersion::VISTA,
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
        send_ref(self.hwnd, LVM_SETINFOTIP, 0, &info);
        let tooltip = send(self.hwnd, LVM_GETTOOLTIPS, 0, 0) as HWND;
        if !tooltip.is_null() {
            send(tooltip, TTM_POP, 0, 0);
        }
    }
    pub fn activate_selected(&self) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        if let Some(row) = selected(self.hwnd) {
            open(row, true);
        }
        true
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
                        && let Some(open) = &self.open
                    {
                        open(i.iItem as usize, hdr.code == NM_DBLCLK);
                    }
                    Some(0)
                }
                NM_CUSTOMDRAW => {
                    let d = &*(l as *const NMLVCUSTOMDRAW);
                    match d.nmcd.dwDrawStage {
                        CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as isize),
                        CDDS_ITEMPREPAINT if self.native_background => {
                            // The control theme supplies light/dark state backgrounds and focus.
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
                            let Some(area) = item_rect(self.hwnd, d.nmcd.dwItemSpec) else {
                                return Some(CDRF_DODEFAULT as isize);
                            };
                            let saved = SaveDC(d.nmcd.hdc);
                            IntersectClipRect(
                                d.nmcd.hdc,
                                area.left,
                                area.top,
                                area.right,
                                area.bottom,
                            );
                            let visuals = self.visuals.borrow();
                            let selected = is_selected(self.hwnd, d.nmcd.dwItemSpec);
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
pub fn context_target(hwnd: HWND, location: isize) -> Option<(usize, POINT)> {
    let (index, point) = if location == -1 {
        let index = selected(hwnd)?;
        send(hwnd, LVM_ENSUREVISIBLE, index, 0);
        let rect = item_rect(hwnd, index)?;
        let mut point = POINT {
            x: rect.left,
            y: rect.bottom,
        };
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
        (index, point)
    } else {
        let point = point_from_lparam(location);
        let mut hit = LVHITTESTINFO {
            pt: point,
            ..unsafe { std::mem::zeroed() }
        };
        unsafe {
            ScreenToClient(hwnd, &mut hit.pt);
        }
        let index = send_mut(hwnd, LVM_HITTEST, 0, &mut hit);
        if index < 0 {
            return None;
        }
        set_item_state(
            hwnd,
            index,
            LVIS_SELECTED | LVIS_FOCUSED,
            LVIS_SELECTED | LVIS_FOCUSED,
        );
        send(hwnd, LVM_SETSELECTIONMARK, 0, index);
        (index as usize, point)
    };
    Some((index, point))
}
fn has_viewport(area: &RECT) -> bool {
    area.right > 0 && area.bottom > 0
}
pub fn fit(hwnd: HWND) {
    let area = client(hwnd);
    if !has_viewport(&area) {
        return;
    }
    let width = area.right;
    if send(hwnd, LVM_GETCOLUMNWIDTH, 0, 0) == width as isize {
        return;
    }
    send(hwnd, LVM_SETCOLUMNWIDTH, 0, width as isize);
}
pub fn item_rect(hwnd: HWND, index: usize) -> Option<RECT> {
    let mut area = RECT {
        left: LVIR_BOUNDS as i32,
        ..unsafe { std::mem::zeroed() }
    };
    (send_mut(hwnd, LVM_GETITEMRECT, index, &mut area) != 0).then_some(area)
}
pub fn selected(hwnd: HWND) -> Option<usize> {
    let index = send(hwnd, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    (index >= 0).then_some(index as usize)
}

pub struct State {
    pub indices: [isize; 4],
    previous: [isize; 2],
    offset: Option<i32>,
}
impl State {
    pub fn capture(hwnd: HWND) -> Self {
        let top = send(hwnd, LVM_GETTOPINDEX, 0, 0);
        let area = client(hwnd);
        // Restoring scroll in a zero-sized ListView can shift its origin before the
        // first layout. Capture only an existing reading position in a valid viewport.
        let rect = (has_viewport(&area) && top >= 0 && send(hwnd, LVM_GETITEMCOUNT, 0, 0) > 0)
            .then(|| item_rect(hwnd, top as usize))
            .flatten();
        let indices = [
            selected(hwnd).map_or(-1, |index| index as isize),
            send(hwnd, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED as isize),
            send(hwnd, LVM_GETSELECTIONMARK, 0, 0),
            if rect.is_some() && top > 0 { top } else { -1 },
        ];
        Self {
            indices,
            previous: [indices[0], indices[1]],
            offset: rect.map(|r| r.top),
        }
    }
    pub fn apply(&self, hwnd: HWND, count: usize) {
        let visible = unsafe { IsWindowVisible(hwnd) != 0 };
        if visible {
            send(hwnd, WM_SETREDRAW, 0, 0);
        }
        for index in self.previous {
            set_item_state(hwnd, index, LVIS_SELECTED | LVIS_FOCUSED, 0);
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
            set_item_state(hwnd, index, flag, flag);
        }
        send(hwnd, LVM_SETSELECTIONMARK, 0, self.indices[2]);
        if visible {
            send(hwnd, WM_SETREDRAW, 1, 0);
        }
        unsafe {
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
    /// Restore an existing reading position only after native layout and column sizing.
    pub fn restore_view(&self, hwnd: HWND) {
        let Some(offset) = self.offset else {
            return;
        };
        let area = client(hwnd);
        let count = send(hwnd, LVM_GETITEMCOUNT, 0, 0);
        if !has_viewport(&area) || count == 0 {
            return;
        }
        let top = self.indices[3].max(0).min(count - 1) as usize;
        if item_rect(hwnd, top).is_some_and(|rect| rect.top == offset) {
            return;
        }
        send(hwnd, LVM_ENSUREVISIBLE, top, 0);
        if let Some(now) = item_rect(hwnd, top) {
            send(hwnd, LVM_SCROLL, 0, (now.top - offset) as isize);
        }
    }
}
pub fn set_item_state(hwnd: HWND, index: isize, mask: u32, value: u32) {
    if index >= 0 {
        let state = LVITEMW {
            stateMask: mask,
            state: value,
            ..unsafe { std::mem::zeroed() }
        };
        send_ref(hwnd, LVM_SETITEMSTATE, index as usize, &state);
    }
}
fn is_selected(hwnd: HWND, item: usize) -> bool {
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
        FillRect(dc, area, crate::theme::brush(background));
        SetTextColor(dc, crate::theme::color(foreground));
    }
}
fn themed_items(hwnd: HWND) -> bool {
    let theme = Theme::open(hwnd, w!("ListView"));
    theme.raw() != 0 && unsafe { IsThemePartDefined(theme.raw(), LVP_LISTITEM, 0) } != 0
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
        if let Some(result) = forward_context_menu(h, msg, w, l) {
            return result;
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
                crate::theme::color(COLOR_WINDOWTEXT)
            };
            let accent = if system {
                text
            } else {
                crate::theme::color(COLOR_HOTLIGHT)
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
                blend(text, crate::theme::color(COLOR_WINDOW))
            };
            area.bottom = area.top + height;
            SelectObject(dc, self.heading.handle.0);
            draw_text(
                dc,
                area,
                &display.title,
                &display.title_matches,
                text,
                accent,
                system,
            );
            area.top = area.bottom + self.gap;
            area.bottom = area.top + self.body_height;
            SelectObject(dc, self.body);
            let (shown, matches) = display.detail.fit((area.right - area.left).max(0), |text| {
                let mut size: SIZE = std::mem::zeroed();
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
                size.cx
            });
            draw_text(dc, area, &shown, &matches, secondary, accent, system);
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

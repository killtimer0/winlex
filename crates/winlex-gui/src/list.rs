use crate::{
    dpi::Font,
    icons::{Icon, Images},
    layout,
    native::*,
    platform::WindowsVersion,
};
use std::{cell::RefCell, ops::Range, ptr};
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Shell::GetWindowSubclass;

#[derive(Default)]
pub struct Display {
    pub title: Vec<u16>,
    pub detail: Vec<u16>,
    pub title_matches: Vec<Range<usize>>,
    pub detail_matches: Vec<Range<usize>>,
    pub icon: Option<Icon>,
    pub path_start: u32,
}

type ReadRow = fn(usize, &mut dyn FnMut(&Display));

pub struct List {
    pub hwnd: HWND,
    visuals: RefCell<Option<Visuals>>,
    row: ReadRow,
    text: fn(usize) -> Vec<u16>,
    open: Option<fn(isize, bool)>,
    native_background: bool,
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
        if unsafe { set_subclass(hwnd, Some(list_proc), 1, themed_items(hwnd) as usize) } == 0 {
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
                        (i.iItem, i.pszText, i.cchTextMax)
                    };
                    if !buffer.is_null() && capacity > 0 {
                        let text = (self.text)(row as usize);
                        let count = text.len().min(capacity as usize - 1);
                        ptr::copy_nonoverlapping(text.as_ptr(), buffer, count);
                        *buffer.add(count) = 0;
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
                                let mut themed = 0;
                                GetWindowSubclass(self.hwnd, Some(list_proc), 1, &mut themed);
                                let system = visuals.high_contrast
                                    || selected && (!self.native_background || themed == 0);
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
            set_subclass(h, Some(list_proc), 1, themed_items(h) as usize);
            InvalidateRect(h, ptr::null(), 0);
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
    high_contrast: bool,
}
impl Visuals {
    fn new(m: &layout::Metrics) -> anyhow::Result<Self> {
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
                let path = display.path_start as usize;
                if index == 1 && path > 0 && path < value.len() {
                    let mut size: SIZE = std::mem::zeroed();
                    GetTextExtentPoint32W(dc, value.as_ptr(), path as i32, &mut size);
                    let mut prefix = area;
                    prefix.right = area.left + size.cx.min((area.right - area.left).max(0) * 2 / 3);
                    draw_text(dc, prefix, &value[..path], &[], color, accent, system);
                    let mut suffix = area;
                    suffix.left = prefix.right;
                    SetTextColor(dc, color);
                    DrawTextW(
                        dc,
                        value[path..].as_ptr(),
                        (value.len() - path) as i32,
                        &mut suffix,
                        DT_SINGLELINE | DT_PATH_ELLIPSIS | DT_NOPREFIX | DT_VCENTER,
                    );
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
            assert_eq!(cached(), 0);
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

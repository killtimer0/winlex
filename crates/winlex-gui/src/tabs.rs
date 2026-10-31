//! Content-independent tab strip. Notifications are requests; setters never notify the host.
use crate::native::*;
use std::{cell::RefCell, ptr, rc::Rc};
mod accessible;
#[cfg(test)]
mod tests;

pub const CLASS: *const u16 = w!("Winlex.Tabs");
const TOOLINFO_SIZE: u32 = std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32;
pub const SELECT: u32 = 0u32.wrapping_sub(800);
pub const CLOSE: u32 = 0u32.wrapping_sub(801);
pub const REORDER: u32 = 0u32.wrapping_sub(802);
pub const ACTIVATE: u32 = 0u32.wrapping_sub(803);
pub const CONTEXT: u32 = 0u32.wrapping_sub(804);

#[repr(C)]
pub struct Notification {
    pub header: NMHDR,
    pub id: u64,
    pub index: usize,
    pub point: POINT,
}

#[derive(Clone)]
pub struct Item {
    /// Stable host identity; must not be reused for a different item.
    pub id: u64,
    pub title: String,
    pub description: String,
    pub italic: bool,
    pub icon: HICON,
    pub closable: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Hit {
    id: u64,
}

struct OwnedIcon(HICON);
impl Drop for OwnedIcon {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DestroyIcon(self.0);
            }
        }
    }
}

struct ItalicFont(HFONT);
impl ItalicFont {
    fn new(base: HFONT) -> Self {
        unsafe {
            let mut definition: LOGFONTW = std::mem::zeroed();
            if GetObjectW(
                base,
                std::mem::size_of::<LOGFONTW>() as i32,
                (&mut definition as *mut LOGFONTW).cast(),
            ) == 0
            {
                return Self(ptr::null_mut());
            }
            definition.lfItalic = 1;
            Self(CreateFontIndirectW(&definition))
        }
    }
}
impl Drop for ItalicFont {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

struct State {
    hwnd: HWND,
    items: Vec<Item>,
    icons: Vec<OwnedIcon>,
    selected: Option<u64>,
    font: HFONT,
    italic_font: ItalicFont,
    unit: i32,
    rects: Vec<RECT>,
    offset: i32,
    hot: Option<Hit>,
    pressed: Option<Hit>,
    origin: POINT,
    dragging: bool,
    tooltip: HWND,
    tip: Vec<u16>,
    name: String,
    select_label: String,
    objects:
        std::collections::HashMap<accessible::Node, windows::Win32::UI::Accessibility::IAccessible>,
    child_ids: std::collections::HashMap<u64, i32>,
    next_child: i32,
}
impl State {
    fn item_font(&self, italic: bool) -> HFONT {
        if italic && !self.italic_font.0.is_null() {
            self.italic_font.0
        } else {
            self.font
        }
    }
    fn rect(&self, id: u64) -> Option<RECT> {
        let index = self.items.iter().position(|item| item.id == id)?;
        self.rects.get(index).copied()
    }
    fn hit(&self, p: POINT) -> Option<Hit> {
        if !contains(client(self.hwnd), p) {
            return None;
        }
        self.items
            .iter()
            .zip(&self.rects)
            .find_map(|(item, r)| contains(*r, p).then_some(Hit { id: item.id }))
    }
}
fn contains(r: RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}
fn state(hwnd: HWND) -> Option<Rc<RefCell<State>>> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Rc<RefCell<State>>;
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).clone() })
    }
}
pub fn create(parent: HWND, id: u16, name: String, select_label: String) -> anyhow::Result<HWND> {
    unsafe {
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            lpszClassName: CLASS,
            ..std::mem::zeroed()
        };
        if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(error("Register tabs"));
        }
    }
    let hwnd = child(
        parent,
        CLASS,
        &wide(&name),
        WS_TABSTOP | WS_CLIPSIBLINGS,
        id,
    )?;
    if let Some(s) = state(hwnd) {
        let mut s = s.borrow_mut();
        s.name = name;
        s.select_label = select_label;
    }
    Ok(hwnd)
}
pub fn set_items(hwnd: HWND, items: Vec<Item>) {
    let Some(s) = state(hwnd) else { return };
    let previous_selection = s.borrow().selected;
    let icons = items
        .iter()
        .map(|item| {
            OwnedIcon(if item.icon.is_null() {
                ptr::null_mut()
            } else {
                unsafe { CopyIcon(item.icon) }
            })
        })
        .collect();
    let (removed, added, renamed, described, reordered) = {
        let mut s = s.borrow_mut();
        let removed = s
            .items
            .iter()
            .filter(|old| !items.iter().any(|new| new.id == old.id))
            .map(|i| i.id)
            .collect::<Vec<_>>();
        let added = items
            .iter()
            .filter(|new| !s.items.iter().any(|old| new.id == old.id))
            .map(|i| i.id)
            .collect::<Vec<_>>();
        let renamed = items
            .iter()
            .filter(|new| {
                s.items
                    .iter()
                    .any(|old| old.id == new.id && old.title != new.title)
            })
            .map(|i| i.id)
            .collect::<Vec<_>>();
        let described = items
            .iter()
            .filter(|new| {
                s.items
                    .iter()
                    .any(|old| old.id == new.id && old.description != new.description)
            })
            .map(|i| i.id)
            .collect::<Vec<_>>();
        let reordered = s.items.iter().map(|i| i.id).ne(items.iter().map(|i| i.id));
        for id in &added {
            s.next_child += 1;
            let child = s.next_child;
            s.child_ids.insert(*id, child);
        }
        s.items = items;
        s.icons = icons;
        s.hot = None;
        if !s.items.iter().any(|i| Some(i.id) == s.selected) {
            s.selected = None;
        }
        s.objects.retain(|node, _| match node {
            accessible::Node::Root => true,
            accessible::Node::Tab(id) => !removed.contains(id),
        });
        (removed, added, renamed, described, reordered)
    };
    layout(&s, true);
    for id in removed {
        event(hwnd, EVENT_OBJECT_DESTROY, Some(id));
        s.borrow_mut().child_ids.remove(&id);
    }
    for id in added {
        event(hwnd, EVENT_OBJECT_CREATE, Some(id));
    }
    for id in renamed {
        event(hwnd, EVENT_OBJECT_NAMECHANGE, Some(id));
    }
    for id in described {
        event(hwnd, EVENT_OBJECT_DESCRIPTIONCHANGE, Some(id));
    }
    if reordered {
        event(hwnd, EVENT_OBJECT_REORDER, None);
    }
    let selection = s.borrow().selected;
    if previous_selection.is_some() && selection.is_none() {
        event(hwnd, EVENT_OBJECT_SELECTIONWITHIN, None);
    }
}
pub fn select(hwnd: HWND, id: Option<u64>) {
    let Some(s) = state(hwnd) else { return };
    let id = id.filter(|id| s.borrow().items.iter().any(|i| i.id == *id));
    let old = {
        let mut s = s.borrow_mut();
        let old = s.selected;
        s.selected = id;
        old
    };
    layout(&s, true);
    if old != id {
        if let Some(old) = old {
            event(hwnd, EVENT_OBJECT_STATECHANGE, Some(old));
        }
        if let Some(id) = id {
            event(hwnd, EVENT_OBJECT_SELECTION, Some(id));
            if unsafe { GetFocus() } == hwnd {
                event(hwnd, EVENT_OBJECT_FOCUS, Some(id));
            }
        }
    }
}
fn event(hwnd: HWND, event: u32, id: Option<u64>) {
    let child = match id {
        Some(id) => {
            let Some(child) = state(hwnd).and_then(|s| s.borrow().child_ids.get(&id).copied())
            else {
                return;
            };
            child
        }
        None => 0,
    };
    unsafe {
        windows_sys::Win32::UI::Accessibility::NotifyWinEvent(event, hwnd, OBJID_CLIENT, child);
    }
}
fn request(hwnd: HWND, code: u32, id: u64, index: usize, point: POINT) {
    if unsafe { IsWindowEnabled(hwnd) } == 0 {
        return;
    }
    if code == CLOSE
        && !state(hwnd).is_some_and(|s| s.borrow().items.iter().any(|i| i.id == id && i.closable))
    {
        return;
    }
    let n = Notification {
        header: NMHDR {
            hwndFrom: hwnd,
            idFrom: unsafe { GetDlgCtrlID(hwnd) } as usize,
            code,
        },
        id,
        index,
        point,
    };
    send(
        unsafe { GetParent(hwnd) },
        WM_NOTIFY,
        n.header.idFrom,
        &n as *const _ as isize,
    );
}
fn title_overhang(dc: HDC, text: &[u16]) -> i32 {
    let mut metrics: TEXTMETRICW = unsafe { std::mem::zeroed() };
    unsafe {
        GetTextMetricsW(dc, &mut metrics);
    }
    let mut overhang = metrics.tmOverhang.max(0);
    for ch in [text.last().copied().unwrap_or(0), b'.' as u16] {
        let mut abc: ABC = unsafe { std::mem::zeroed() };
        if unsafe { GetCharABCWidthsW(dc, ch as u32, ch as u32, &mut abc) } != 0 {
            overhang = overhang.max(abc.abcC.saturating_neg());
        }
    }
    overhang
}

fn draw_title(dc: HDC, text: &[u16], bounds: RECT, overhang: i32) {
    unsafe {
        let saved = SaveDC(dc);
        let mut area = bounds;
        area.right = (area.right - overhang).max(area.left);
        let mut flags = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX;
        if saved != 0
            && IntersectClipRect(dc, bounds.left, bounds.top, bounds.right, bounds.bottom) != 0
        {
            flags |= DT_NOCLIP;
        }
        DrawTextW(dc, text.as_ptr(), text.len() as i32, &mut area, flags);
        if saved != 0 {
            RestoreDC(dc, saved);
        }
    }
}

fn layout(shared: &Rc<RefCell<State>>, reveal: bool) {
    let hwnd = shared.borrow().hwnd;
    let area = client(hwnd);
    let (font, items) = {
        let s = shared.borrow();
        (s.font, s.items.clone())
    };
    let dc = unsafe { GetDC(hwnd) };
    let old = unsafe { SelectObject(dc, font) };
    let mut tm: TEXTMETRICW = unsafe { std::mem::zeroed() };
    unsafe {
        GetTextMetricsW(dc, &mut tm);
    }
    let unit = (tm.tmHeight / 2).max(6);
    let widths = items
        .iter()
        .map(|i| {
            let text = wide(&i.title);
            let mut size = SIZE { cx: 0, cy: 0 };
            unsafe {
                SelectObject(dc, shared.borrow().item_font(i.italic));
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
            }
            let overhang = title_overhang(dc, &text[..text.len() - 1]);
            (size.cx + overhang + unit * if i.icon.is_null() { 2 } else { 5 })
                .clamp(unit * 10, unit * 28)
        })
        .collect::<Vec<_>>();
    unsafe {
        SelectObject(dc, old);
        ReleaseDC(hwnd, dc);
    }
    let moved = {
        let mut s = shared.borrow_mut();
        s.unit = unit;
        let total: i32 = widths.iter().sum();
        if reveal && let Some(index) = s.items.iter().position(|i| Some(i.id) == s.selected) {
            let left: i32 = widths[..index].iter().sum();
            let right = left + widths[index];
            if left < s.offset {
                s.offset = left;
            }
            if right > s.offset + area.right {
                s.offset = (right - area.right).min(left);
            }
        }
        s.offset = s.offset.clamp(0, (total - area.right).max(0));
        let mut x = -s.offset;
        let rects: Vec<_> = widths
            .into_iter()
            .map(|width| {
                let r = RECT {
                    left: x,
                    top: 0,
                    right: x + width,
                    bottom: area.bottom,
                };
                x += width;
                r
            })
            .collect();
        let moved = s
            .items
            .iter()
            .zip(&rects)
            .enumerate()
            .filter_map(|(index, (item, r))| {
                let changed = s.rects.get(index).is_none_or(|old| {
                    (old.left, old.top, old.right, old.bottom) != (r.left, r.top, r.right, r.bottom)
                });
                changed.then_some(item.id)
            })
            .collect::<Vec<_>>();
        s.rects = rects;
        moved
    };
    unsafe {
        InvalidateRect(hwnd, ptr::null(), 0);
    }
    if !moved.is_empty() {
        event(hwnd, EVENT_OBJECT_LOCATIONCHANGE, None);
        for id in moved {
            event(hwnd, EVENT_OBJECT_LOCATIONCHANGE, Some(id));
        }
    }
}
fn paint(shared: &Rc<RefCell<State>>) {
    let s = shared.borrow();
    unsafe {
        let mut ps = std::mem::zeroed();
        let dc = BeginPaint(s.hwnd, &mut ps);
        FillRect(dc, &client(s.hwnd), GetSysColorBrush(COLOR_BTNFACE));
        let old = SelectObject(dc, s.font);
        SetBkMode(dc, TRANSPARENT as i32);
        for ((item, rect), icon) in s.items.iter().zip(&s.rects).zip(&s.icons) {
            let selected = Some(item.id) == s.selected;
            let hot = s.hot.is_some_and(|h| h.id == item.id);
            FillRect(
                dc,
                rect,
                GetSysColorBrush(if selected {
                    COLOR_WINDOW
                } else if hot {
                    COLOR_3DLIGHT
                } else {
                    COLOR_BTNFACE
                }),
            );
            SetTextColor(dc, GetSysColor(COLOR_WINDOWTEXT));
            if selected {
                let mut line = *rect;
                line.bottom = line.top + (s.unit / 3).max(2);
                FillRect(dc, &line, GetSysColorBrush(COLOR_HIGHLIGHT));
            }
            let mut text_rect = *rect;
            text_rect.left += s.unit;
            if !icon.0.is_null() {
                DrawIconEx(
                    dc,
                    text_rect.left,
                    (rect.bottom - s.unit * 2) / 2,
                    icon.0,
                    s.unit * 2,
                    s.unit * 2,
                    0,
                    ptr::null_mut(),
                    DI_NORMAL,
                );
                text_rect.left += s.unit * 3;
            }
            text_rect.right -= s.unit;
            let title = wide(&item.title);
            SelectObject(dc, s.item_font(item.italic));
            let title = &title[..title.len() - 1];
            draw_title(dc, title, text_rect, title_overhang(dc, title));
            if selected && GetFocus() == s.hwnd {
                let mut r = *rect;
                InflateRect(&mut r, -3, -4);
                DrawFocusRect(dc, &r);
            }
        }
        if s.dragging
            && let (Some(pressed), Some(hot)) = (s.pressed, s.hot)
        {
            let from = s.items.iter().position(|i| i.id == pressed.id);
            let to = s.items.iter().position(|i| i.id == hot.id);
            if let (Some(from), Some(to)) = (from, to)
                && from != to
            {
                let mut marker = s.rects[to];
                let x = if from < to {
                    marker.right - 3
                } else {
                    marker.left
                };
                marker.left = x;
                marker.right = x + 3;
                FillRect(dc, &marker, GetSysColorBrush(COLOR_HIGHLIGHT));
            }
        }
        SelectObject(dc, old);
        EndPaint(s.hwnd, &ps);
    }
}
fn tooltip(shared: &Rc<RefCell<State>>, hit: Option<Hit>) {
    let (hwnd, tip, text) = {
        let mut s = shared.borrow_mut();
        let text = hit
            .and_then(|hit| {
                s.items
                    .iter()
                    .find(|i| i.id == hit.id)
                    .map(|i| i.title.clone())
            })
            .unwrap_or_default();
        s.tip = wide(&text);
        (s.hwnd, s.tooltip, s.tip.as_mut_ptr())
    };
    let info = TTTOOLINFOW {
        cbSize: TOOLINFO_SIZE,
        hwnd,
        uId: hwnd as usize,
        lpszText: text,
        ..unsafe { std::mem::zeroed() }
    };
    send(tip, TTM_UPDATETIPTEXTW, 0, &info as *const _ as isize);
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        if msg == WM_NCCREATE {
            let s = Rc::new(RefCell::new(State {
                hwnd,
                items: vec![],
                icons: vec![],
                selected: None,
                font: GetStockObject(DEFAULT_GUI_FONT),
                italic_font: ItalicFont::new(GetStockObject(DEFAULT_GUI_FONT)),
                unit: 8,
                rects: vec![],
                offset: 0,
                hot: None,
                pressed: None,
                origin: POINT { x: 0, y: 0 },
                dragging: false,
                tooltip: ptr::null_mut(),
                tip: vec![0],
                name: String::new(),
                select_label: String::new(),
                objects: Default::default(),
                child_ids: Default::default(),
                next_child: 0,
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(Box::new(s)) as _);
        }
        let Some(shared) = state(hwnd) else {
            return DefWindowProcW(hwnd, msg, w, l);
        };
        let p = POINT {
            x: l as u16 as i16 as i32,
            y: (l >> 16) as u16 as i16 as i32,
        };
        match msg {
            WM_CREATE => {
                let tip = CreateWindowExW(
                    WS_EX_TOPMOST,
                    w!("tooltips_class32"),
                    ptr::null(),
                    WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
                    0,
                    0,
                    0,
                    0,
                    hwnd,
                    ptr::null_mut(),
                    instance(),
                    ptr::null(),
                );
                shared.borrow_mut().tooltip = tip;
                if tip.is_null() {
                    return -1;
                }
                let mut info: TTTOOLINFOW = std::mem::zeroed();
                info.cbSize = TOOLINFO_SIZE;
                info.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
                info.hwnd = hwnd;
                info.uId = hwnd as usize;
                info.lpszText = shared.borrow_mut().tip.as_mut_ptr();
                if send(tip, TTM_ADDTOOLW, 0, &info as *const _ as isize) == 0 {
                    return -1;
                }
                return 0;
            }
            WM_GETOBJECT if l as i32 == OBJID_CLIENT => return accessible::result(&shared, w),
            WM_GETFONT => return shared.borrow().font as isize,
            WM_SETFONT => {
                let font = if w == 0 {
                    GetStockObject(DEFAULT_GUI_FONT)
                } else {
                    w as HFONT
                };
                {
                    let mut s = shared.borrow_mut();
                    s.font = font;
                    s.italic_font = ItalicFont::new(font);
                }
                layout(&shared, true);
                return 0;
            }
            WM_SIZE => {
                layout(&shared, true);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                paint(&shared);
                return 0;
            }
            WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
                InvalidateRect(hwnd, ptr::null(), 0);
                return 0;
            }
            WM_GETDLGCODE => return DLGC_WANTARROWS as isize,
            WM_SETFOCUS | WM_KILLFOCUS => {
                InvalidateRect(hwnd, ptr::null(), 0);
                let selected = shared.borrow().selected;
                if msg == WM_SETFOCUS {
                    event(hwnd, EVENT_OBJECT_FOCUS, selected);
                }
                return 0;
            }
            WM_MOUSEMOVE => {
                let hit = shared.borrow().hit(p);
                let changed = {
                    let mut s = shared.borrow_mut();
                    let changed = s.hot != hit;
                    s.hot = hit;
                    if s.pressed.is_some() && (p.x - s.origin.x).abs() > GetSystemMetrics(SM_CXDRAG)
                    {
                        s.dragging = true;
                    }
                    changed
                };
                let scrolling = {
                    let mut s = shared.borrow_mut();
                    let step = if p.x < s.unit * 2 {
                        -s.unit
                    } else if p.x > client(hwnd).right - s.unit * 2 {
                        s.unit
                    } else {
                        0
                    };
                    if s.dragging {
                        s.offset += step;
                        step != 0
                    } else {
                        false
                    }
                };
                if scrolling {
                    layout(&shared, false);
                }
                if changed || shared.borrow().dragging {
                    tooltip(&shared, hit);
                    InvalidateRect(hwnd, ptr::null(), 0);
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut track);
                return 0;
            }
            WM_MOUSELEAVE => {
                shared.borrow_mut().hot = None;
                tooltip(&shared, None);
                InvalidateRect(hwnd, ptr::null(), 0);
                return 0;
            }
            WM_LBUTTONDOWN => {
                let hit = shared.borrow().hit(p);
                {
                    let mut s = shared.borrow_mut();
                    s.pressed = hit;
                    s.origin = p;
                    s.dragging = false;
                }
                focus(hwnd);
                SetCapture(hwnd);
                if let Some(hit) = hit {
                    request(hwnd, SELECT, hit.id, 0, p);
                }
                return 0;
            }
            WM_LBUTTONUP => {
                let (pressed, hit, dragging, index) = {
                    let mut s = shared.borrow_mut();
                    let hit = s.hit(p);
                    let index = hit
                        .and_then(|h| s.items.iter().position(|i| i.id == h.id))
                        .unwrap_or(0);
                    (s.pressed.take(), hit, s.dragging, index)
                };
                ReleaseCapture();
                if let Some(pressed) = pressed
                    && dragging
                    && hit.is_some()
                {
                    request(hwnd, REORDER, pressed.id, index, p);
                }
                return 0;
            }
            WM_CAPTURECHANGED | WM_CANCELMODE => {
                {
                    let mut s = shared.borrow_mut();
                    s.pressed = None;
                    s.dragging = false;
                }
                InvalidateRect(hwnd, ptr::null(), 0);
                return 0;
            }
            WM_MBUTTONUP | WM_LBUTTONDBLCLK => {
                let hit = shared.borrow().hit(p);
                if let Some(hit) = hit {
                    request(
                        hwnd,
                        if msg == WM_MBUTTONUP { CLOSE } else { ACTIVATE },
                        hit.id,
                        0,
                        p,
                    );
                }
                return 0;
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = (w >> 16) as u16 as i16 as i32;
                {
                    let mut s = shared.borrow_mut();
                    s.offset += if msg == WM_MOUSEWHEEL { -delta } else { delta };
                }
                layout(&shared, false);
                return 0;
            }
            WM_CONTEXTMENU => {
                let (id, point) = if l == -1 {
                    let s = shared.borrow();
                    (
                        s.selected,
                        s.selected
                            .and_then(|id| s.rect(id))
                            .map(|r| POINT {
                                x: r.left.max(0),
                                y: r.bottom,
                            })
                            .unwrap_or(POINT { x: 0, y: 0 }),
                    )
                } else {
                    let mut point = p;
                    ScreenToClient(hwnd, &mut point);
                    (shared.borrow().hit(point).map(|h| h.id), point)
                };
                if let Some(id) = id {
                    let mut point = point;
                    ClientToScreen(hwnd, &mut point);
                    request(hwnd, CONTEXT, id, 0, point);
                }
                return 0;
            }
            WM_KEYDOWN => {
                if w == VK_APPS as usize || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0
                {
                    return send(hwnd, WM_CONTEXTMENU, hwnd as usize, -1);
                }
                let (id, selected, index) = {
                    let s = shared.borrow();
                    let index = s
                        .items
                        .iter()
                        .position(|i| Some(i.id) == s.selected)
                        .unwrap_or(0);
                    let next = match w as u16 {
                        VK_LEFT => index.saturating_sub(1),
                        VK_RIGHT => (index + 1).min(s.items.len().saturating_sub(1)),
                        VK_HOME => 0,
                        VK_END => s.items.len().saturating_sub(1),
                        _ => index,
                    };
                    (s.items.get(next).map(|i| i.id), s.selected, next)
                };
                if matches!(w as u16, VK_LEFT | VK_RIGHT | VK_HOME | VK_END) {
                    if GetKeyState(VK_CONTROL as i32) < 0 && GetKeyState(VK_SHIFT as i32) < 0 {
                        if let Some(id) = selected {
                            request(hwnd, REORDER, id, index, p);
                        }
                    } else if let Some(id) = id {
                        request(hwnd, SELECT, id, 0, p);
                    }
                    return 0;
                }
            }
            WM_NCDESTROY => {
                let tip = shared.borrow().tooltip;
                if !tip.is_null() {
                    DestroyWindow(tip);
                }
                shared.borrow_mut().hwnd = ptr::null_mut();
                let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Rc<RefCell<State>>;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(raw));
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

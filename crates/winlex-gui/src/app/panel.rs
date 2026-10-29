//! Registered sidebar views and their shared owner-data list presentation.
use super::{bind, catalog, dpi, keys, read, sidebar, sources};
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::{any::Any, cell::Cell, ptr, rc::Rc};

pub(super) struct Registration {
    pub id: u32,
    pub command: u16,
    pub label: u16,
    pub create: fn(HWND) -> anyhow::Result<Rc<dyn Panel>>,
}
pub(super) const SEARCH_PANEL: u32 = 0;
pub(super) const SOURCES_PANEL: u32 = 1;
pub(super) static PANELS: &[Registration] = &[
    Registration {
        id: SEARCH_PANEL,
        command: PANEL_SEARCH,
        label: IDS_PANEL_SEARCH,
        create: Search::create,
    },
    Registration {
        id: SOURCES_PANEL,
        command: MANAGE_SOURCES,
        label: IDS_PANEL_SOURCES,
        create: Sources::create,
    },
];
pub(super) trait Panel: Any {
    // Pure handle inspection; the host may call this while reading its own state.
    fn controls(&self) -> &[HWND];
    fn focus(&self) {
        focus(self.controls()[0]);
    }
    fn prepare(&self, m: &layout::Metrics) {
        for &h in self.controls() {
            bind(h, m.font);
        }
    }
    fn layout(&self, area: RECT, m: &layout::Metrics);
    fn command(&self, _id: u16, _code: u32) -> bool {
        false
    }
    fn region(&self, _hwnd: HWND) -> Option<keys::Region> {
        None
    }
    unsafe fn notify(&self, _l: isize) -> Option<isize> {
        None
    }
}
struct Search {
    controls: [HWND; 2],
    composing: Cell<bool>,
    deadline: Cell<Option<std::time::Instant>>,
}
struct Sources {
    controls: [HWND; 4],
}
fn search<T>(f: impl FnOnce(&Search) -> T) -> T {
    let panel: Rc<dyn Any> = read(|a| a.sidebar.panel(SEARCH_PANEL));
    f(panel.downcast_ref().expect("registered search panel"))
}
const SEARCH_TIMER: usize = 1;
pub(super) fn cancel_debounce() -> (HWND, bool) {
    let state = search(|s| {
        s.deadline.set(None);
        (s.controls[0], s.composing.get())
    });
    unsafe {
        KillTimer(state.0, SEARCH_TIMER);
    }
    state
}
pub(super) fn debounce(input: HWND) -> anyhow::Result<()> {
    if unsafe { SetTimer(input, SEARCH_TIMER, 200, None) } == 0 {
        return Err(error("Search debounce"));
    }
    search(|s| {
        s.deadline.set(Some(
            std::time::Instant::now() + std::time::Duration::from_millis(200),
        ))
    });
    Ok(())
}
impl Search {
    fn create(frame: HWND) -> anyhow::Result<Rc<dyn Panel>> {
        let input = child_ex(
            frame,
            w!("EDIT"),
            &[0],
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            SEARCH,
        )?;
        if unsafe { set_subclass(input, Some(edit_proc), 1, 0) } == 0 {
            anyhow::bail!("Subclass search input failed");
        }
        Ok(Rc::new(Self {
            controls: [input, create_list(frame, RESULTS)?],
            composing: Cell::new(false),
            deadline: Cell::new(None),
        }))
    }
}
impl Sources {
    fn create(frame: HWND) -> anyhow::Result<Rc<dyn Panel>> {
        let list = create_list(frame, SOURCE_LIST)?;
        let tools = [
            (LOAD_METADATA, IDS_LOAD_METADATA),
            (RELOAD_SOURCE, IDS_RELOAD),
            (REMOVE_SOURCE, IDS_UNLOAD),
        ]
        .into_iter()
        .map(|(id, label)| {
            child(
                frame,
                w!("BUTTON"),
                resources::wide_text(label),
                WS_TABSTOP,
                id,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
        Ok(Rc::new(Self {
            controls: [list, tools[0], tools[1], tools[2]],
        }))
    }
}
impl Panel for Search {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }
    fn prepare(&self, m: &layout::Metrics) {
        prepare_list(&self.controls, self.controls[1], m);
    }
    fn layout(&self, area: RECT, m: &layout::Metrics) {
        let [input, results] = self.controls;
        let mut area = layout::inset(area, m.px(4));
        let outer = dpi().child_outer(input, layout::rect(0, 0, 1, m.height + m.px(4)));
        place(input, layout::take_top(&mut area, outer.bottom - outer.top));
        layout::take_top(&mut area, m.px(4));
        place(results, area);
        fit(results);
    }
    fn command(&self, id: u16, code: u32) -> bool {
        match id {
            SEARCH if code == EN_CHANGE => catalog::input_changed(),
            FOCUS_SEARCH => {
                sidebar::show(SEARCH_PANEL, false);
                send(self.controls[0], EM_SETSEL, 0, -1);
            }
            _ => return false,
        }
        true
    }
    fn region(&self, hwnd: HWND) -> Option<keys::Region> {
        if hwnd == self.controls[0] {
            Some(keys::Region::SearchEntry {
                composing: self.composing.get(),
            })
        } else {
            (hwnd == self.controls[1]).then_some(keys::Region::Results)
        }
    }
    unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe {
            List {
                hwnd: self.controls[1],
                cell: catalog::cell,
                open: Some(catalog::open),
            }
            .notify(l)
        }
    }
}
impl Panel for Sources {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }
    fn prepare(&self, m: &layout::Metrics) {
        prepare_list(&self.controls, self.controls[0], m);
    }
    fn layout(&self, area: RECT, m: &layout::Metrics) {
        let mut area = layout::inset(area, m.px(4));
        m.buttons(&mut area, self.controls[1..].iter().copied());
        place(self.controls[0], area);
        fit(self.controls[0]);
    }
    fn command(&self, id: u16, _code: u32) -> bool {
        match id {
            LOAD_METADATA => sources::choose_add(),
            RELOAD_SOURCE | REMOVE_SOURCE => sources::action(id),
            _ => return false,
        }
        true
    }
    unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe {
            List {
                hwnd: self.controls[0],
                cell: sources::cell,
                open: None,
            }
            .notify(l)
        }
    }
}
fn prepare_list(controls: &[HWND], list: HWND, m: &layout::Metrics) {
    for &h in controls {
        bind(h, m.font);
    }
    // A transparent small image defines a fixed two-line native row height.
    unsafe {
        let image = ImageList_Create(1, m.height * 2 + m.px(8), ILC_COLOR32, 1, 1);
        let old = send(list, LVM_SETIMAGELIST, LVSIL_SMALL as usize, image as isize);
        if old != 0 {
            ImageList_Destroy(old as _);
        }
    }
}
struct List {
    hwnd: HWND,
    cell: fn(usize, bool) -> String,
    open: Option<fn(isize, catalog::Open)>,
}
impl List {
    pub unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if hdr.hwndFrom != self.hwnd {
                return None;
            }
            match hdr.code {
                LVN_GETDISPINFOW => {
                    let i = &mut *(l as *mut NMLVDISPINFOW);
                    if i.item.mask & LVIF_TEXT != 0 {
                        copy_text(
                            &(self.cell)(i.item.iItem as usize, true),
                            i.item.pszText,
                            i.item.cchTextMax,
                        );
                    }
                    Some(0)
                }
                LVN_GETINFOTIPW => {
                    let i = &mut *(l as *mut NMLVGETINFOTIPW);
                    copy_text(
                        &(self.cell)(i.iItem as usize, true),
                        i.pszText,
                        i.cchTextMax,
                    );
                    Some(0)
                }
                NM_CLICK | NM_DBLCLK => {
                    let i = &*(l as *const NMITEMACTIVATE);
                    if i.iItem >= 0
                        && let Some(open) = self.open
                    {
                        let mode = if hdr.code == NM_DBLCLK {
                            catalog::Open::Pin
                        } else {
                            catalog::Open::Preview
                        };
                        open(i.iItem as isize, mode);
                    }
                    Some(0)
                }
                NM_CUSTOMDRAW => {
                    let d = &mut *(l as *mut NMLVCUSTOMDRAW);
                    match d.nmcd.dwDrawStage {
                        CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as isize),
                        CDDS_ITEMPREPAINT => {
                            // XP custom-draw notifications do not supply usable item bounds.
                            d.nmcd.rc.left = LVIR_BOUNDS as i32;
                            if send(
                                hdr.hwndFrom,
                                LVM_GETITEMRECT,
                                d.nmcd.dwItemSpec,
                                &mut d.nmcd.rc as *mut RECT as isize,
                            ) == 0
                            {
                                return Some(CDRF_DODEFAULT as isize);
                            }
                            draw_list_background(hdr.hwndFrom, d);
                            SetBkMode(d.nmcd.hdc, TRANSPARENT as i32);
                            let content = (self.cell)(d.nmcd.dwItemSpec, false);
                            let mid = (d.nmcd.rc.top + d.nmcd.rc.bottom) / 2;
                            for (line, (top, bottom)) in content
                                .splitn(2, '\n')
                                .zip([(d.nmcd.rc.top, mid), (mid, d.nmcd.rc.bottom)])
                            {
                                let mut r = d.nmcd.rc;
                                r.left += layout::scale(4, dpi().window(hdr.hwndFrom));
                                r.top = top;
                                r.bottom = bottom;
                                DrawTextW(
                                    d.nmcd.hdc,
                                    wide(line).as_mut_ptr(),
                                    -1,
                                    &mut r,
                                    DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX | DT_VCENTER,
                                );
                            }
                            if d.nmcd.uItemState & CDIS_FOCUS != 0
                                && GetFocus() == hdr.hwndFrom
                                && send(hdr.hwndFrom, WM_QUERYUISTATE, 0, 0)
                                    & UISF_HIDEFOCUS as isize
                                    == 0
                            {
                                DrawFocusRect(d.nmcd.hdc, &d.nmcd.rc);
                            }
                            Some(CDRF_SKIPDEFAULT as isize)
                        }
                        _ => Some(CDRF_DODEFAULT as isize),
                    }
                }
                _ => None,
            }
        }
    }
}
unsafe fn draw_list_background(hwnd: HWND, draw: &NMLVCUSTOMDRAW) {
    unsafe {
        let focused = GetFocus() == hwnd;
        let selected = send(
            hwnd,
            LVM_GETITEMSTATE,
            draw.nmcd.dwItemSpec,
            LVIS_SELECTED as isize,
        ) != 0
            && (focused || GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & LVS_SHOWSELALWAYS != 0);
        let hot = draw.nmcd.uItemState & CDIS_HOT != 0;
        let state = match (selected, focused, hot) {
            (true, false, _) => LISS_SELECTEDNOTFOCUS,
            (true, true, true) => LISS_HOTSELECTED,
            (true, true, false) => LISS_SELECTED,
            (false, _, true) => LISS_HOT,
            _ => LISS_NORMAL,
        };
        let background = if selected {
            if focused {
                COLOR_HIGHLIGHT
            } else {
                COLOR_BTNFACE
            }
        } else {
            COLOR_WINDOW
        };
        let mut color = GetSysColor(if selected && focused {
            COLOR_HIGHLIGHTTEXT
        } else {
            COLOR_WINDOWTEXT
        });
        let theme = GetWindowTheme(hwnd);
        // The control owns this theme handle and refreshes it on theme changes.
        FillRect(draw.nmcd.hdc, &draw.nmcd.rc, GetSysColorBrush(COLOR_WINDOW));
        if theme == 0
            || DrawThemeBackground(
                theme,
                draw.nmcd.hdc,
                LVP_LISTITEM,
                state,
                &draw.nmcd.rc,
                ptr::null(),
            ) < 0
        {
            FillRect(draw.nmcd.hdc, &draw.nmcd.rc, GetSysColorBrush(background));
        } else {
            GetThemeColor(theme, LVP_LISTITEM, state, TMT_TEXTCOLOR as i32, &mut color);
        }
        SetTextColor(draw.nmcd.hdc, color);
    }
}
fn create_list(frame: HWND, id: u16) -> anyhow::Result<HWND> {
    let h = child(
        frame,
        w!("SysListView32"),
        &[0],
        WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_NOCOLUMNHEADER,
        id,
    )?;
    if unsafe { set_subclass(h, Some(list_proc), 1, 0) } == 0 {
        anyhow::bail!("Subclass list control {id} failed");
    }
    list_column(h, 0, "", 100);
    send(
        h,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        0,
        (LVS_EX_FULLROWSELECT | LVS_EX_INFOTIP | LVS_EX_DOUBLEBUFFER) as isize,
    );
    Ok(h)
}
pub(super) fn fit(h: HWND) {
    send(h, LVM_SETCOLUMNWIDTH, 0, client(h).right.max(1) as isize);
    unsafe {
        ShowScrollBar(h, SB_HORZ, 0);
    }
}
unsafe fn copy_text(value: &str, buffer: *mut u16, capacity: i32) {
    if !buffer.is_null() && capacity > 0 {
        let s = wide(value);
        let n = (capacity as usize - 1).min(s.len() - 1);
        unsafe {
            ptr::copy_nonoverlapping(s.as_ptr(), buffer, n);
            *buffer.add(n) = 0;
        }
    }
}
unsafe extern "system" fn edit_proc(
    h: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        if msg == WM_TIMER && w == SEARCH_TIMER {
            if search(|s| {
                s.deadline
                    .get()
                    .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            }) {
                catalog::search();
            }
            return 0;
        }
        if msg == WM_IME_STARTCOMPOSITION {
            search(|s| s.composing.set(true));
            catalog::input_changed();
        }
        let result = default_subclass(h, msg, w, l);
        if msg == WM_IME_ENDCOMPOSITION {
            search(|s| s.composing.set(false));
            catalog::input_changed();
        }
        if msg == WM_NCDESTROY {
            remove_subclass(h, Some(edit_proc), 1);
        }
        result
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

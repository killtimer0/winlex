//! Sidebar content. Each variant owns its controls and input state.
use super::{bind, catalog, dpi, read, sidebar, sources, write};
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::ptr;

pub(super) struct Registration {
    pub id: u32,
    pub command: u16,
    pub label: u16,
    pub create: fn(HWND) -> Option<Panel>,
}
pub(super) const SEARCH_PANEL: u32 = 0;
pub(super) const SOURCES_PANEL: u32 = 1;
pub(super) static PANELS: &[Registration] = &[
    Registration {
        id: SEARCH_PANEL,
        command: PANEL_SEARCH,
        label: IDS_PANEL_SEARCH,
        create: Panel::search,
    },
    Registration {
        id: SOURCES_PANEL,
        command: MANAGE_SOURCES,
        label: IDS_PANEL_SOURCES,
        create: Panel::sources,
    },
];
#[derive(Clone)]
pub(super) enum Panel {
    Search {
        input: HWND,
        results: HWND,
        composing: bool,
        deadline: Option<std::time::Instant>,
    },
    Sources {
        list: HWND,
        tools: [HWND; 4],
    },
}
const SEARCH_TIMER: usize = 1;
pub(super) fn cancel_debounce() -> (HWND, bool) {
    let state = write(|a| match a.sidebar.panel_mut(SEARCH_PANEL) {
        Panel::Search {
            input,
            deadline,
            composing,
            ..
        } => {
            *deadline = None;
            (*input, *composing)
        }
        _ => unreachable!(),
    });
    unsafe {
        KillTimer(state.0, SEARCH_TIMER);
    }
    state
}
pub(super) fn debounce(input: HWND) -> Result<(), String> {
    if unsafe { SetTimer(input, SEARCH_TIMER, 200, None) } == 0 {
        return Err(error("Search debounce"));
    }
    write(|a| {
        if let Panel::Search { deadline, .. } = a.sidebar.panel_mut(SEARCH_PANEL) {
            *deadline = Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
        }
    });
    Ok(())
}
impl Panel {
    fn search(frame: HWND) -> Option<Self> {
        let input = child_ex(
            frame,
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            SEARCH,
        );
        if input.is_null() || unsafe { set_subclass(input, Some(edit_proc), 1, 0) } == 0 {
            return None;
        }
        Some(Self::Search {
            input,
            results: create_list(frame, RESULTS)?,
            composing: false,
            deadline: None,
        })
    }
    fn sources(frame: HWND) -> Option<Self> {
        let list = create_list(frame, SOURCE_LIST)?;
        let tools = [
            (LOAD_METADATA, IDS_ADD),
            (REFRESH_SOURCES, IDS_REFRESH),
            (ADD_DIRECTORY, IDS_DIRECTORY),
            (FORGET_LOCATION, IDS_FORGET_LOCATION),
        ]
        .map(|(id, label)| child(frame, "BUTTON", &resources::text(label), WS_TABSTOP, id));
        tools
            .iter()
            .all(|h| !h.is_null())
            .then_some(Self::Sources { list, tools })
    }
    pub fn list(&self) -> HWND {
        match self {
            Self::Search { results, .. } => *results,
            Self::Sources { list, .. } => *list,
        }
    }
    pub fn focus(&self) -> HWND {
        match self {
            Self::Search { input, .. } => *input,
            Self::Sources { list, .. } => *list,
        }
    }
    pub fn controls(&self) -> impl Iterator<Item = HWND> + '_ {
        let (first, rest) = match self {
            Self::Search { input, results, .. } => (*input, std::slice::from_ref(results)),
            Self::Sources { list, tools } => (*list, tools.as_slice()),
        };
        std::iter::once(first).chain(rest.iter().copied())
    }
    pub fn prepare(&self, m: &layout::Metrics) {
        for h in self.controls() {
            bind(h, m.font);
        }
        // A transparent small image defines a fixed two-line native row height.
        unsafe {
            let image = ImageList_Create(1, m.height * 2 + m.px(8), ILC_COLOR32, 1, 1);
            let old = send(
                self.list(),
                LVM_SETIMAGELIST,
                LVSIL_SMALL as usize,
                image as isize,
            );
            if old != 0 {
                ImageList_Destroy(old as _);
            }
        }
    }
    pub fn layout(&self, area: RECT, m: &layout::Metrics) {
        let mut area = layout::inset(area, m.px(4));
        match self {
            Self::Search { input, .. } => {
                let outer = dpi().child_outer(*input, layout::rect(0, 0, 1, m.height + m.px(4)));
                place(
                    *input,
                    layout::take_top(&mut area, outer.bottom - outer.top),
                );
                layout::take_top(&mut area, m.px(4));
            }
            Self::Sources { tools, .. } => m.buttons(&mut area, *tools),
        }
        place(self.list(), area);
        fit(self.list());
    }
    pub fn command(&self, id: u16, code: u32) -> bool {
        match self {
            Self::Search { input, .. } => match id {
                SEARCH if code == EN_CHANGE => catalog::input_changed(),
                FOCUS_SEARCH => {
                    sidebar::show(SEARCH_PANEL, false);
                    focus(*input);
                    send(*input, EM_SETSEL, 0, -1);
                }
                _ => return false,
            },
            Self::Sources { .. } => match id {
                LOAD_METADATA => sources::choose_add(),
                ADD_DIRECTORY => sources::choose_directory(),
                REFRESH_SOURCES => sources::refresh(),
                FORGET_LOCATION => sources::forget_selected(),
                _ => return false,
            },
        }
        true
    }
    pub fn key(&self, hwnd: HWND, key: usize) -> bool {
        if key != VK_RETURN as usize {
            return false;
        }
        if let Self::Search {
            input, composing, ..
        } = self
            && hwnd == *input
        {
            if *composing {
                return false;
            }
            catalog::search();
        } else if hwnd == self.list() {
            self.open(send(
                hwnd,
                LVM_GETNEXTITEM,
                usize::MAX,
                LVNI_SELECTED as isize,
            ));
        } else {
            return false;
        }
        true
    }
    fn open(&self, row: isize) {
        match self {
            Self::Search { .. } => catalog::open_row(row),
            Self::Sources { .. } => sources::open_row(row),
        }
    }
    fn cell(&self, row: usize, detail: bool) -> String {
        match self {
            Self::Search { .. } => catalog::cell(row, detail),
            Self::Sources { .. } => sources::cell(row, detail),
        }
    }
    pub unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if hdr.hwndFrom != self.list() {
                return None;
            }
            match hdr.code {
                LVN_GETDISPINFOW => {
                    let i = &mut *(l as *mut NMLVDISPINFOW);
                    if i.item.mask & LVIF_TEXT != 0 {
                        copy_text(
                            &self.cell(i.item.iItem as usize, true),
                            i.item.pszText,
                            i.item.cchTextMax,
                        );
                    }
                    Some(0)
                }
                LVN_GETINFOTIPW => {
                    let i = &mut *(l as *mut NMLVGETINFOTIPW);
                    copy_text(&self.cell(i.iItem as usize, true), i.pszText, i.cchTextMax);
                    Some(0)
                }
                NM_CLICK | NM_DBLCLK
                    if matches!(self, Self::Sources { .. }) || hdr.code == NM_DBLCLK =>
                {
                    let i = &*(l as *const NMITEMACTIVATE);
                    if i.iItem >= 0 {
                        self.open(i.iItem as isize);
                    }
                    Some(0)
                }
                NM_CUSTOMDRAW => {
                    let d = &mut *(l as *mut NMLVCUSTOMDRAW);
                    match d.nmcd.dwDrawStage {
                        CDDS_PREPAINT => Some(CDRF_NOTIFYITEMDRAW as isize),
                        CDDS_ITEMPREPAINT => {
                            let selected = send(
                                hdr.hwndFrom,
                                LVM_GETITEMSTATE,
                                d.nmcd.dwItemSpec,
                                LVIS_SELECTED as isize,
                            ) != 0;
                            FillRect(
                                d.nmcd.hdc,
                                &d.nmcd.rc,
                                GetSysColorBrush(if selected {
                                    COLOR_HIGHLIGHT
                                } else {
                                    COLOR_WINDOW
                                }),
                            );
                            SetTextColor(
                                d.nmcd.hdc,
                                GetSysColor(if selected {
                                    COLOR_HIGHLIGHTTEXT
                                } else {
                                    COLOR_WINDOWTEXT
                                }),
                            );
                            SetBkMode(d.nmcd.hdc, TRANSPARENT as i32);
                            let content = self.cell(d.nmcd.dwItemSpec, false);
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
                            if d.nmcd.uItemState & CDIS_FOCUS != 0 {
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
fn create_list(frame: HWND, id: u16) -> Option<HWND> {
    let h = child(
        frame,
        "SysListView32",
        "",
        WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_NOCOLUMNHEADER,
        id,
    );
    if h.is_null() || unsafe { set_subclass(h, Some(list_proc), 1, 0) } == 0 {
        return None;
    }
    list_column(h, 0, "", 100);
    send(
        h,
        LVM_SETEXTENDEDLISTVIEWSTYLE,
        0,
        (LVS_EX_FULLROWSELECT | LVS_EX_INFOTIP | LVS_EX_DOUBLEBUFFER) as isize,
    );
    Some(h)
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
            if read(
                |a| matches!(a.sidebar.panel(SEARCH_PANEL), Panel::Search { deadline: Some(deadline), .. } if std::time::Instant::now() >= *deadline),
            ) {
                catalog::search();
            }
            return 0;
        }
        if msg == WM_IME_STARTCOMPOSITION {
            write(|a| {
                if let Panel::Search { composing, .. } = &mut a.sidebar.panel_mut(SEARCH_PANEL) {
                    *composing = true;
                }
            });
            catalog::input_changed();
        }
        let result = default_subclass(h, msg, w, l);
        if msg == WM_IME_ENDCOMPOSITION {
            write(|a| {
                if let Panel::Search { composing, .. } = &mut a.sidebar.panel_mut(SEARCH_PANEL) {
                    *composing = false;
                }
            });
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
        if msg == WM_KEYDOWN && w == VK_F1 as usize {
            let row = send(h, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
            if row >= 0
                && let Some(panel) = read(|a| a.sidebar.owner(h))
            {
                MessageBoxW(
                    h,
                    wide(&panel.cell(row as usize, true)).as_ptr(),
                    wide(&resources::text(IDS_SOURCE)).as_ptr(),
                    MB_OK | MB_ICONINFORMATION,
                );
            }
            return 0;
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

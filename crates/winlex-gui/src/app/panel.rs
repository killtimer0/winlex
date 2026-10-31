//! Registered sidebar views and their shared owner-data list presentation.
use super::{bind, catalog, dpi, keys, read, sidebar, sources};
use crate::list::List;
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::{cell::Cell, rc::Rc};
use winlex_core::SearchMode;

const SEARCH_MODES: &[(u16, SearchMode, u16)] = &[
    (
        IDS_MODE_COMBINED,
        SearchMode::Combined,
        IDS_SEARCH_TIP_QUICK,
    ),
    (
        IDS_MODE_CONDITION,
        SearchMode::Condition,
        IDS_SEARCH_TIP_CONDITION,
    ),
];

pub(super) fn search_mode() -> SearchMode {
    search(|s| SEARCH_MODES[s.mode.get()].1)
}

pub(super) struct Registration {
    pub id: u32,
    pub command: u16,
    pub label: u16,
    pub icon: u16,
    pub create: fn(HWND) -> anyhow::Result<Rc<Panel>>,
}
pub(super) const SEARCH_PANEL: u32 = 0;
pub(super) const SOURCES_PANEL: u32 = 1;
pub(super) static PANELS: &[Registration] = &[
    Registration {
        id: SEARCH_PANEL,
        command: PANEL_SEARCH,
        label: IDS_PANEL_SEARCH,
        icon: IDR_SEARCH,
        create: Panel::search,
    },
    Registration {
        id: SOURCES_PANEL,
        command: MANAGE_SOURCES,
        label: IDS_PANEL_SOURCES,
        icon: IDR_FILES,
        create: Panel::sources,
    },
];
pub(super) struct Panel {
    controls: Vec<HWND>,
    list: List,
    content: Content,
}
enum Content {
    Search(Search),
    Sources,
}
struct Search {
    input: HWND,
    tooltip: HWND,
    selector: HWND,
    mode: Cell<usize>,
    composing: Cell<bool>,
    deadline: Cell<Option<std::time::Instant>>,
}
fn search<T>(f: impl FnOnce(&Search) -> T) -> T {
    let panel = read(|a| a.sidebar.panel(SEARCH_PANEL));
    let Content::Search(search) = &panel.content else {
        unreachable!("registered search panel")
    };
    f(search)
}
const SEARCH_TIMER: usize = 1;
fn search_tool(input: HWND, label: u16) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: input,
        uId: input as usize,
        lpszText: resources::wide_text(label).as_ptr().cast_mut(),
        ..unsafe { std::mem::zeroed() }
    }
}
pub(super) fn cancel_debounce() -> (HWND, bool) {
    let state = search(|s| {
        s.deadline.set(None);
        (s.input, s.composing.get())
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
impl Panel {
    fn search(frame: HWND) -> anyhow::Result<Rc<Self>> {
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
        let mode = child(
            frame,
            w!("COMBOBOX"),
            &[0],
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            SEARCH_MODE,
        )?;
        for &(label, _, _) in SEARCH_MODES {
            send(
                mode,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(mode, CB_SETCURSEL, 0, 0);
        let tooltip = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                std::ptr::null(),
                WS_POPUP | TTS_ALWAYSTIP | TTS_NOPREFIX,
                0,
                0,
                0,
                0,
                frame,
                std::ptr::null_mut(),
                instance(),
                std::ptr::null(),
            )
        };
        if tooltip.is_null() {
            return Err(error("Create search tooltip"));
        }
        let tool = search_tool(input, SEARCH_MODES[0].2);
        if send(tooltip, TTM_ADDTOOLW, 0, &tool as *const _ as isize) == 0 {
            unsafe {
                DestroyWindow(tooltip);
            }
            anyhow::bail!("Register search tooltip failed");
        }
        send(tooltip, TTM_SETMAXTIPWIDTH, 0, 480);
        let list = List::new(
            frame,
            RESULTS,
            catalog::row,
            catalog::row_text,
            Some(catalog::activate),
            read(|a| a.system_version),
        )?;
        Ok(Rc::new(Self {
            controls: vec![input, list.hwnd, mode],
            content: Content::Search(Search {
                input,
                tooltip,
                selector: mode,
                mode: Cell::new(0),
                composing: Cell::new(false),
                deadline: Cell::new(None),
            }),
            list,
        }))
    }
    fn sources(frame: HWND) -> anyhow::Result<Rc<Self>> {
        let list = List::new(
            frame,
            SOURCE_LIST,
            sources::row,
            sources::row_text,
            None,
            read(|a| a.system_version),
        )?;
        let mut controls = vec![list.hwnd];
        for (id, label) in [
            (LOAD_METADATA, IDS_LOAD_METADATA),
            (RELOAD_SOURCE, IDS_RELOAD),
            (REMOVE_SOURCE, IDS_UNLOAD),
        ] {
            controls.push(child(
                frame,
                w!("BUTTON"),
                resources::wide_text(label),
                WS_TABSTOP,
                id,
            )?);
        }
        sources::update_actions();
        Ok(Rc::new(Self {
            controls,
            list,
            content: Content::Sources,
        }))
    }
    pub fn controls(&self) -> &[HWND] {
        &self.controls
    }
    pub fn refresh_tooltip(&self) {
        self.list.refresh_tooltip();
    }
    pub fn focus(&self) {
        focus(self.controls[0]);
    }
    pub fn prepare(&self, m: &layout::Metrics) {
        if let Content::Search(search) = &self.content {
            bind(search.tooltip, m.font);
            send(search.tooltip, TTM_SETMAXTIPWIDTH, 0, m.px(480) as isize);
        }
        for &h in &self.controls {
            bind(h, m.font);
        }
        if let Err(error) = self.list.prepare(m) {
            super::write(|a| a.failure = Some(error));
        }
    }
    pub fn recolor(&self) {
        if let Err(error) = self.list.recolor() {
            super::write(|a| a.failure = Some(error));
        }
    }
    pub unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe {
            let hdr = &*(l as *const NMHDR);
            if hdr.hwndFrom == self.list.hwnd
                && hdr.code == LVN_ITEMCHANGED
                && matches!(self.content, Content::Sources)
            {
                sources::update_actions();
                return Some(0);
            }
            self.list.notify(l)
        }
    }
    pub fn layout(&self, area: RECT, m: &layout::Metrics, positions: &mut Vec<(HWND, RECT)>) {
        let mut area = layout::inset(area, m.px(4));
        match &self.content {
            Content::Search(s) => {
                let outer = dpi().child_outer(s.input, layout::rect(0, 0, 1, m.height + m.px(4)));
                let mut row = layout::take_top(&mut area, outer.bottom - outer.top);
                let width = m
                    .label_width(
                        SEARCH_MODES
                            .iter()
                            .map(|&(label, _, _)| resources::wide_text(label)),
                        60,
                    )
                    .min((row.right - row.left) / 2);
                let right = row.right;
                row.right = (right - width - m.px(4)).max(row.left);
                positions.push((s.input, row));
                positions.push((
                    s.selector,
                    layout::rect(right - width, row.top, width, m.row() * 6),
                ));
                layout::take_top(&mut area, m.px(4));
            }
            Content::Sources => m.buttons(&mut area, self.controls[1..].iter().copied(), positions),
        }
        positions.push((self.list.hwnd, area));
    }
    pub fn fit_list(&self) {
        crate::list::fit(self.list.hwnd);
    }
    pub fn region(&self, hwnd: HWND) -> Option<keys::Region> {
        match &self.content {
            Content::Search(s) if hwnd == s.input => Some(keys::Region::SearchEntry {
                composing: s.composing.get(),
            }),
            Content::Search(_) => (hwnd == self.list.hwnd).then_some(keys::Region::Results),
            Content::Sources => None,
        }
    }
    pub fn command(&self, id: u16, code: u32) -> bool {
        match &self.content {
            Content::Search(s) => s.command(id, code),
            Content::Sources => sources::command(id),
        }
    }
    pub fn context_menu(&self, hwnd: HWND, location: isize) -> bool {
        match &self.content {
            Content::Sources if hwnd == self.list.hwnd => {
                if let Err(error) = sources::context_menu(hwnd, location) {
                    catalog::set_status(format!("{error:#}"));
                }
                true
            }
            Content::Search(_) | Content::Sources => false,
        }
    }
}
impl Search {
    fn command(&self, id: u16, code: u32) -> bool {
        match id {
            SEARCH if code == EN_CHANGE => catalog::input_changed(),
            SEARCH_MODE if code == CBN_SELENDOK => {
                let index = send(self.selector, CB_GETCURSEL, 0, 0) as usize;
                if index < SEARCH_MODES.len() && self.mode.replace(index) != index {
                    let tool = search_tool(self.input, SEARCH_MODES[index].2);
                    send(self.tooltip, TTM_POP, 0, 0);
                    send(
                        self.tooltip,
                        TTM_UPDATETIPTEXTW,
                        0,
                        &tool as *const _ as isize,
                    );
                    catalog::search();
                }
            }
            SEARCH_MODE if code == CBN_SELENDCANCEL => {
                send(self.selector, CB_SETCURSEL, self.mode.get(), 0);
            }
            FOCUS_SEARCH => {
                sidebar::show(SEARCH_PANEL, false);
                send(self.input, EM_SETSEL, 0, -1);
            }
            _ => return false,
        }
        true
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

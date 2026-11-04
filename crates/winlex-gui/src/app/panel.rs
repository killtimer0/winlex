//! The two sidebar views and their shared owner-data list presentation.
use super::{bind, catalog, dpi, read, sidebar, sources};
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
    SEARCH_MODES[search().mode.get()].1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum PanelId {
    Search = 0,
    Sources = 1,
    Tools = 2,
}
pub(super) const SEARCH_PANEL: PanelId = PanelId::Search;
pub(super) const SOURCES_PANEL: PanelId = PanelId::Sources;
impl PanelId {
    pub const ALL: [Self; 3] = [Self::Search, Self::Sources, Self::Tools];
    pub fn from_saved(value: u32) -> Self {
        Self::ALL
            .into_iter()
            .find(|&id| id as u32 == value)
            .unwrap_or(Self::Search)
    }
    pub fn presentation(self) -> (u16, u16, u16) {
        match self {
            Self::Search => (PANEL_SEARCH, IDS_PANEL_SEARCH, IDR_SEARCH),
            Self::Sources => (MANAGE_SOURCES, IDS_PANEL_SOURCES, IDR_FILES),
            Self::Tools => (PANEL_TOOLS, IDS_PANEL_TOOLS, IDR_SYMBOL_MISC),
        }
    }
}
pub(super) struct Panel {
    pub controls: Vec<HWND>,
    pub list: List,
}
pub(super) struct Search {
    pub input: HWND,
    tooltip: HWND,
    selector: HWND,
    mode: Cell<usize>,
    pub composing: Cell<bool>,
    deadline: Cell<Option<std::time::Instant>>,
}
pub(super) fn search() -> Rc<Search> {
    read(|a| a.sidebar.search.as_ref().unwrap().clone())
}
const SEARCH_TIMER: usize = 1;
fn search_tool(input: HWND, label: u16) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: TOOLINFO_SIZE,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: input,
        uId: input as usize,
        lpszText: resources::wide_text(label).as_ptr().cast_mut(),
        ..unsafe { std::mem::zeroed() }
    }
}
pub(super) fn cancel_debounce() -> (HWND, bool) {
    let search = search();
    search.deadline.set(None);
    unsafe {
        KillTimer(search.input, SEARCH_TIMER);
    }
    (search.input, search.composing.get())
}
pub(super) fn debounce(input: HWND) -> anyhow::Result<()> {
    if unsafe { SetTimer(input, SEARCH_TIMER, 200, None) } == 0 {
        return Err(error("Search debounce"));
    }
    search().deadline.set(Some(
        std::time::Instant::now() + std::time::Duration::from_millis(200),
    ));
    Ok(())
}
pub(super) fn create(frame: HWND) -> anyhow::Result<([Rc<Panel>; 3], Rc<Search>)> {
    let input = unsafe {
        child_ex(
            frame,
            w!("EDIT"),
            &[0],
            WS_TABSTOP | ES_AUTOHSCROLL as u32,
            WS_EX_CLIENTEDGE,
            SEARCH,
        )
    }?;
    if unsafe { set_subclass(input, Some(edit_proc), 1, 0) } == 0 {
        anyhow::bail!("Subclass search input failed");
    }
    let mode = unsafe {
        child(
            frame,
            w!("COMBOBOX"),
            &[0],
            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
            SEARCH_MODE,
        )
    }?;
    for &(label, _, _) in SEARCH_MODES {
        send(
            mode,
            CB_ADDSTRING,
            0,
            resources::wide_text(label).as_ptr() as isize,
        );
    }
    send(mode, CB_SETCURSEL, 0, 0);
    let tool = search_tool(input, SEARCH_MODES[0].2);
    let tooltip = tooltip(frame, &tool).map_err(|error| error.context("Search tooltip"))?;
    send(tooltip, TTM_SETMAXTIPWIDTH, 0, 480);
    let list = List::new(
        frame,
        RESULTS,
        catalog::row,
        catalog::row_text,
        Some(catalog::activate),
        read(|a| a.system_version),
    )?;
    let search_panel = Rc::new(Panel {
        controls: vec![input, list.hwnd, mode],
        list,
    });
    let search = Rc::new(Search {
        input,
        tooltip,
        selector: mode,
        mode: Cell::new(0),
        composing: Cell::new(false),
        deadline: Cell::new(None),
    });
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
        controls.push(unsafe {
            child(
                frame,
                w!("BUTTON"),
                resources::wide_text(label),
                WS_TABSTOP,
                id,
            )
        }?);
    }
    sources::update_actions();
    let sources_panel = Rc::new(Panel { controls, list });
    let list = List::new(
        frame,
        TOOL_LIST,
        super::tools::row,
        super::tools::row_text,
        Some(super::tools::activate),
        read(|a| a.system_version),
    )?;
    send(list.hwnd, LVM_SETITEMCOUNT, super::tools::REGISTRY.len(), 0);
    let controls = vec![list.hwnd, unsafe {
        child(
            frame,
            w!("BUTTON"),
            resources::wide_text(IDS_TOOL_NEW),
            WS_TABSTOP,
            TOOL_NEW,
        )
    }?];
    Ok((
        [
            search_panel,
            sources_panel,
            Rc::new(Panel { controls, list }),
        ],
        search,
    ))
}
impl Panel {
    pub fn focus(&self) {
        focus(self.controls[0]);
    }
    pub fn prepare(&self, m: &layout::Metrics) {
        for &h in &self.controls {
            bind(h, m.font);
        }
        super::checked(self.list.prepare(m));
    }
    pub fn recolor(&self) {
        super::checked(self.list.recolor());
    }
}
impl Search {
    pub fn prepare(&self, m: &layout::Metrics) {
        bind(self.tooltip, m.font);
        send(self.tooltip, TTM_SETMAXTIPWIDTH, 0, m.px(480) as isize);
    }
    pub fn layout(&self, area: &mut RECT, m: &layout::Metrics, positions: &mut Vec<(HWND, RECT)>) {
        let mut row = layout::take_top(area, m.edit_height(self.input, dpi()));
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
        positions.push((self.input, row));
        positions.push((
            self.selector,
            layout::rect(right - width, row.top, width, m.row() * 6),
        ));
        layout::take_top(area, m.px(4));
    }
    fn command(&self, id: u16, code: u32) -> bool {
        match id {
            SEARCH if code == EN_SETFOCUS => {
                super::status::status_context(super::status::StatusContext::Catalog);
            }
            SEARCH_MODE if code == CBN_SETFOCUS => {
                super::status::status_context(super::status::StatusContext::Catalog);
            }
            SEARCH if code == EN_CHANGE => catalog::input_changed(),
            SEARCH_MODE if code == CBN_SELENDOK => {
                super::status::begin_status(super::status::StatusContext::Catalog);
                let index = send(self.selector, CB_GETCURSEL, 0, 0) as usize;
                if index < SEARCH_MODES.len() && self.mode.replace(index) != index {
                    let tool = search_tool(self.input, SEARCH_MODES[index].2);
                    send(self.tooltip, TTM_POP, 0, 0);
                    send_ref(self.tooltip, TTM_UPDATETIPTEXTW, 0, &tool);
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
pub(super) fn command(w: usize, control: HWND) -> bool {
    let id = w as u16;
    if id == TOOL_NEW {
        super::tools::selected(true);
        return true;
    }
    if let Some(panel) = PanelId::ALL.into_iter().find(|p| p.presentation().0 == id) {
        sidebar::show(panel, true);
        return true;
    }
    // Ignore notifications from controls that have not finished initialization.
    if !control.is_null() && read(|a| a.sidebar.owner(control)).is_none() {
        return false;
    }
    search().command(id, (w >> 16) as u32) || sources::command(id)
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
            if search()
                .deadline
                .get()
                .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            {
                catalog::search();
            }
            return 0;
        }
        if msg == WM_IME_STARTCOMPOSITION {
            search().composing.set(true);
            catalog::input_changed();
        }
        let result = default_subclass(h, msg, w, l);
        if msg == WM_IME_ENDCOMPOSITION {
            search().composing.set(false);
            catalog::input_changed();
        }
        if msg == WM_NCDESTROY {
            remove_subclass(h, Some(edit_proc), 1);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_ids_preserve_saved_values_and_unique_presentations() {
        assert_eq!(PanelId::Search as u32, 0);
        assert_eq!(PanelId::Sources as u32, 1);
        for (index, id) in PanelId::ALL.into_iter().enumerate() {
            assert_eq!(index, id as usize);
            assert_eq!(PanelId::from_saved(id as u32), id);
            let (command, label, icon) = id.presentation();
            for other in PanelId::ALL.into_iter().filter(|other| *other != id) {
                let (other_command, other_label, other_icon) = other.presentation();
                assert_ne!(command, other_command);
                assert_ne!(label, other_label);
                assert_ne!(icon, other_icon);
            }
        }
        assert_eq!(PanelId::from_saved(u32::MAX), PanelId::Search);
    }
}

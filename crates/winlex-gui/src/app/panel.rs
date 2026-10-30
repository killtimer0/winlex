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

const SEARCH_MODES: &[(u16, SearchMode)] = &[
    (IDS_MODE_COMBINED, SearchMode::Combined),
    (IDS_MODE_NAME, SearchMode::Name),
    (IDS_MODE_VALUE, SearchMode::Value),
    (IDS_MODE_GUID, SearchMode::Guid),
];

pub(super) fn search_mode() -> SearchMode {
    search(|s| SEARCH_MODES[s.mode.get()].1)
}

pub(super) struct Registration {
    pub id: u32,
    pub command: u16,
    pub label: u16,
    pub create: fn(HWND) -> anyhow::Result<Rc<Panel>>,
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
pub(super) struct Panel {
    controls: Vec<HWND>,
    list: List,
    search: Option<Search>,
}
struct Search {
    input: HWND,
    selector: HWND,
    mode: Cell<usize>,
    composing: Cell<bool>,
    deadline: Cell<Option<std::time::Instant>>,
}
fn search<T>(f: impl FnOnce(&Search) -> T) -> T {
    let panel = read(|a| a.sidebar.panel(SEARCH_PANEL));
    f(panel.search.as_ref().expect("registered search panel"))
}
const SEARCH_TIMER: usize = 1;
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
        for &(label, _) in SEARCH_MODES {
            send(
                mode,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(mode, CB_SETCURSEL, 0, 0);
        let list = List::new(
            frame,
            RESULTS,
            catalog::row,
            catalog::row_text,
            Some(catalog::activate),
        )?;
        Ok(Rc::new(Self {
            controls: vec![input, list.hwnd, mode],
            search: Some(Search {
                input,
                selector: mode,
                mode: Cell::new(0),
                composing: Cell::new(false),
                deadline: Cell::new(None),
            }),
            list,
        }))
    }
    fn sources(frame: HWND) -> anyhow::Result<Rc<Self>> {
        let list = List::new(frame, SOURCE_LIST, sources::row, sources::row_text, None)?;
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
        Ok(Rc::new(Self {
            controls,
            list,
            search: None,
        }))
    }
    pub fn controls(&self) -> &[HWND] {
        &self.controls
    }
    pub fn focus(&self) {
        focus(self.controls[0]);
    }
    pub fn prepare(&self, m: &layout::Metrics) {
        for &h in &self.controls {
            bind(h, m.font);
        }
        if let Err(error) = self.list.prepare(m) {
            super::write(|a| a.failure = Some(error));
        }
    }
    pub unsafe fn notify(&self, l: isize) -> Option<isize> {
        unsafe { self.list.notify(l) }
    }
    pub fn layout(&self, area: RECT, m: &layout::Metrics) {
        let mut area = layout::inset(area, m.px(4));
        if let Some(s) = &self.search {
            let outer = dpi().child_outer(s.input, layout::rect(0, 0, 1, m.height + m.px(4)));
            let mut row = layout::take_top(&mut area, outer.bottom - outer.top);
            let width = m
                .label_width(
                    SEARCH_MODES
                        .iter()
                        .map(|&(label, _)| resources::wide_text(label)),
                    60,
                )
                .min((row.right - row.left) / 2);
            let right = row.right;
            row.right = (right - width - m.px(4)).max(row.left);
            place(s.input, row);
            place(
                s.selector,
                layout::rect(right - width, row.top, width, m.row() * 6),
            );
            layout::take_top(&mut area, m.px(4));
        } else {
            m.buttons(&mut area, self.controls[1..].iter().copied());
        }
        self.list.layout(area);
    }
    pub fn region(&self, hwnd: HWND) -> Option<keys::Region> {
        let s = self.search.as_ref()?;
        if hwnd == s.input {
            Some(keys::Region::SearchEntry {
                composing: s.composing.get(),
            })
        } else {
            (hwnd == self.list.hwnd).then_some(keys::Region::Results)
        }
    }
}
pub(super) fn command(id: u16, code: u32) -> bool {
    match id {
        SEARCH if code == EN_CHANGE => catalog::input_changed(),
        SEARCH_MODE if code == CBN_SELENDOK => search(|s| {
            let index = send(s.selector, CB_GETCURSEL, 0, 0) as usize;
            if index < SEARCH_MODES.len() && s.mode.replace(index) != index {
                catalog::input_changed();
                catalog::search();
            }
        }),
        SEARCH_MODE if code == CBN_SELENDCANCEL => search(|s| {
            send(s.selector, CB_SETCURSEL, s.mode.get(), 0);
        }),
        FOCUS_SEARCH => {
            sidebar::show(SEARCH_PANEL, false);
            search(|s| {
                send(s.input, EM_SETSEL, 0, -1);
            });
        }
        LOAD_METADATA => sources::choose_add(),
        RELOAD_SOURCE | REMOVE_SOURCE => sources::action(id),
        _ => return false,
    }
    true
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

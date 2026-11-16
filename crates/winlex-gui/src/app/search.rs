//! Search panel controls, input composition and debounce.
use super::{
    bind, catalog,
    panel::{Panel, PanelId},
    sidebar,
};
use crate::app::context::Context;
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

pub(super) fn search_mode(ctx: &Context) -> SearchMode {
    SEARCH_MODES[sidebar::search(ctx).mode.get()].1
}

pub(super) struct Search {
    pub input: HWND,
    tooltip: HWND,
    selector: HWND,
    mode: Cell<usize>,
    pub composing: Cell<bool>,
    deadline: Cell<Option<std::time::Instant>>,
}
const SEARCH_TIMER: usize = 1;
pub(super) fn cancel_debounce(ctx: &Context) -> (HWND, bool) {
    let search = sidebar::search(ctx);
    search.deadline.set(None);
    unsafe {
        KillTimer(search.input, SEARCH_TIMER);
    }
    (search.input, search.composing.get())
}
pub(super) fn debounce(ctx: &Context, input: HWND) -> anyhow::Result<()> {
    if unsafe { SetTimer(input, SEARCH_TIMER, 200, None) } == 0 {
        return Err(error("Search debounce"));
    }
    sidebar::search(ctx).deadline.set(Some(
        std::time::Instant::now() + std::time::Duration::from_millis(200),
    ));
    Ok(())
}
pub(super) fn create(ctx: &Context, frame: HWND) -> anyhow::Result<(Rc<Panel>, Rc<Search>)> {
    // Lists outlive this call; the boxed context outlives every list.
    let state = ctx as *const Context;
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
    let tool = tooltip_info(
        input,
        resources::wide_text(SEARCH_MODES[0].2).as_ptr().cast_mut(),
    );
    let tooltip = tooltip(frame, &tool).map_err(|error| error.context("Search tooltip"))?;
    send(tooltip, TTM_SETMAXTIPWIDTH, 0, 480);
    let list = List::new(
        frame,
        RESULTS,
        move |index, visit| catalog::row(unsafe { &*state }, index, visit),
        move |index| catalog::row_text(unsafe { &*state }, index),
        Some(Box::new(move |row, double| {
            catalog::activate(unsafe { &*state }, row, double)
        })),
        ctx.system_version(),
    )?;
    let search_panel = Rc::new(Panel {
        id: PanelId::Search,
        focus: input,
        fields: vec![input, mode],
        buttons: vec![],
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
    Ok((search_panel, search))
}
impl Search {
    pub fn prepare(&self, m: &layout::Metrics) -> anyhow::Result<()> {
        bind(self.tooltip, m.font);
        send(self.tooltip, TTM_SETMAXTIPWIDTH, 0, m.px(480) as isize);
        // Configure dropdown height with font/DPI changes, not routine layout.
        collapse_combo(self.selector, m.row(), 6)
    }
    pub fn layout(
        &self,
        ctx: &Context,
        area: &mut RECT,
        m: &layout::Metrics,
        positions: &mut Vec<(HWND, RECT)>,
    ) {
        let mut row = layout::take_top(area, m.edit_height(self.input, ctx.dpi()));
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
        let selector = bounds(self.selector, unsafe { GetParent(self.selector) });
        positions.push((
            self.selector,
            layout::rect(
                right - width,
                row.top,
                width,
                selector.bottom - selector.top,
            ),
        ));
        layout::take_top(area, m.px(4));
    }
    pub fn command(&self, ctx: &Context, id: u16, code: u32) -> bool {
        match id {
            // Both search controls report their own focus notification code.
            SEARCH | SEARCH_MODE if matches!(code, EN_SETFOCUS | CBN_SETFOCUS) => {
                super::status::status_context(
                    ctx,
                    super::status::StatusContext::Panel(PanelId::Search),
                );
            }
            SEARCH if code == EN_CHANGE => catalog::input_changed(ctx),
            SEARCH_MODE if code == CBN_SELENDOK => {
                super::status::begin_status(
                    ctx,
                    super::status::StatusContext::Panel(PanelId::Search),
                );
                let index = send(self.selector, CB_GETCURSEL, 0, 0) as usize;
                if index < SEARCH_MODES.len() && self.mode.replace(index) != index {
                    let tool = tooltip_info(
                        self.input,
                        resources::wide_text(SEARCH_MODES[index].2)
                            .as_ptr()
                            .cast_mut(),
                    );
                    send(self.tooltip, TTM_POP, 0, 0);
                    send_ref(self.tooltip, TTM_UPDATETIPTEXTW, 0, &tool);
                    catalog::search(ctx);
                }
            }
            SEARCH_MODE if code == CBN_SELENDCANCEL => {
                send(self.selector, CB_SETCURSEL, self.mode.get(), 0);
            }
            FOCUS_SEARCH => {
                sidebar::show(ctx, PanelId::Search, false);
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
        let Some(ctx) = Context::from_window(h) else {
            return default_subclass(h, msg, w, l);
        };
        if msg == WM_TIMER && w == SEARCH_TIMER {
            if sidebar::search(ctx)
                .deadline
                .get()
                .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            {
                catalog::search(ctx);
            }
            return 0;
        }
        if msg == WM_IME_STARTCOMPOSITION {
            sidebar::search(ctx).composing.set(true);
            catalog::input_changed(ctx);
        }
        let result = default_subclass(h, msg, w, l);
        if msg == WM_IME_ENDCOMPOSITION {
            sidebar::search(ctx).composing.set(false);
            catalog::input_changed(ctx);
        }
        if msg == WM_NCDESTROY {
            remove_subclass(h, Some(edit_proc), 1);
        }
        result
    }
}

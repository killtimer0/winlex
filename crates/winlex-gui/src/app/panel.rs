//! Common sidebar presentation and panel-owned command routing.
use super::{bind, catalog, sidebar, sources, tools};
use crate::{app::context::Context, layout, list::List, native::*, resources::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum PanelId {
    Search = 0,
    Sources = 1,
    Tools = 2,
}
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
    pub id: PanelId,
    pub focus: HWND,
    pub fields: Vec<HWND>,
    pub buttons: Vec<HWND>,
    pub list: List,
}
impl Panel {
    pub fn controls(&self) -> impl Iterator<Item = HWND> + '_ {
        std::iter::once(self.list.hwnd)
            .chain(self.fields.iter().copied())
            .chain(self.buttons.iter().copied())
    }
    pub fn focus(&self) {
        focus(self.focus);
    }
    pub fn prepare(&self, ctx: &Context, m: &layout::Metrics) {
        for h in self.controls() {
            bind(h, m.font);
        }
        ctx.checked(self.list.prepare(m));
    }
    pub fn recolor(&self, ctx: &Context) {
        ctx.checked(self.list.recolor());
    }
}
pub(super) fn update_actions(ctx: &Context, id: PanelId) {
    match id {
        PanelId::Search => {}
        PanelId::Sources => sources::update_actions(ctx),
        PanelId::Tools => tools::panel::update_actions(ctx),
    }
}
pub(super) fn status(ctx: &Context, id: PanelId) -> String {
    match id {
        PanelId::Search => catalog::search_status(ctx),
        PanelId::Sources => catalog::source_status(ctx),
        PanelId::Tools => String::new(),
    }
}
pub(super) fn open_selected(ctx: &Context, new: bool) {
    match sidebar::active(ctx) {
        PanelId::Search => catalog::open_selected(
            ctx,
            if new {
                catalog::Open::New
            } else {
                catalog::Open::Pin
            },
        ),
        PanelId::Tools => tools::panel::selected(ctx, new),
        PanelId::Sources => {}
    }
}
pub(super) fn context_menu(ctx: &Context, panel: &Panel, location: isize) -> anyhow::Result<()> {
    match panel.id {
        PanelId::Search => catalog::context_menu(ctx, panel.list.hwnd, location),
        PanelId::Sources => sources::context_menu(ctx, panel.list.hwnd, location),
        PanelId::Tools => tools::panel::context_menu(ctx, panel.list.hwnd, location),
    }
}
pub(super) fn command(ctx: &Context, w: usize, control: HWND) -> bool {
    let id = w as u16;
    // Menu and accelerator commands have no control notification code.
    let code = if control.is_null() {
        0
    } else {
        (w >> 16) as u32
    };
    if let Some(panel) = PanelId::ALL.into_iter().find(|p| p.presentation().0 == id) {
        sidebar::show(ctx, panel, true);
        return true;
    }
    // Ignore notifications from controls that have not finished initialization.
    if !control.is_null() {
        let Some(owner) = sidebar::owner(ctx, control) else {
            return false;
        };
        return match owner.id {
            PanelId::Search => sidebar::search(ctx).command(ctx, id, code),
            PanelId::Sources => sources::command(ctx, id, code),
            PanelId::Tools => tools::panel::command(ctx, id, code),
        };
    }
    sidebar::search(ctx).command(ctx, id, code)
        || sources::command(ctx, id, code)
        || tools::panel::command(ctx, id, code)
}

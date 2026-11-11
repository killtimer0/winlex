//! Tool identity, list presentation and instance hosting.
use super::{history, workspace};
use crate::app::context::Context;
use crate::{
    icons::Icon,
    layout::Placement,
    list::{Display, TextSegment},
    native::*,
    resources::{self, *},
};
use std::rc::Rc;

mod error_code;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Kind {
    ErrorCode,
}

pub(super) struct Descriptor {
    pub kind: Kind,
    pub title: u16,
    pub description: u16,
    pub icon: Icon,
}

pub(super) const TOOLS: &[Descriptor] = &[Descriptor {
    kind: Kind::ErrorCode,
    title: IDS_ERROR_CODE_TOOL,
    description: IDS_ERROR_CODE_DESCRIPTION,
    icon: Icon::Constant,
}];

impl Kind {
    pub fn descriptor(self) -> &'static Descriptor {
        TOOLS
            .iter()
            .find(|descriptor| descriptor.kind == self)
            .expect("tool descriptor")
    }
}

/// Initial work handed to a tool when it is opened from outside.
#[derive(Clone)]
pub(super) enum Request {
    ErrorCode(u32),
}

impl Request {
    fn kind(&self) -> Kind {
        match self {
            Self::ErrorCode(_) => Kind::ErrorCode,
        }
    }
}

/// A search-result command owned by one tool.
pub(super) struct Action {
    pub label: u16,
    pub request: Request,
}

/// Search-result actions in tool descriptor order.
pub(super) fn actions(value: Option<&crate::app::result_value::ResultValue>) -> Vec<Action> {
    TOOLS
        .iter()
        .filter_map(|descriptor| match descriptor.kind {
            Kind::ErrorCode => error_code::action(value),
        })
        .collect()
}

pub(super) enum ToolView {
    ErrorCode(Rc<error_code::View>),
}

macro_rules! forward {
    ($this:ident, $view:ident, $value:expr) => {
        match $this {
            Self::ErrorCode($view) => $value,
        }
    };
}

impl ToolView {
    pub fn kind(&self) -> Kind {
        match self {
            Self::ErrorCode(_) => Kind::ErrorCode,
        }
    }
    pub fn id(&self) -> u64 {
        forward!(self, view, view.id)
    }
    pub fn hwnd(&self) -> HWND {
        forward!(self, view, view.hwnd)
    }
    fn recent(&self) -> u64 {
        forward!(self, view, view.recent.get())
    }
    pub fn is_live(&self) -> bool {
        forward!(self, view, view.live.get())
    }
    pub fn mark_closed(&self) {
        forward!(self, view, view.live.set(false))
    }
    pub fn placement(&self) -> Option<Placement> {
        forward!(self, view, view.placement.get())
    }
    pub fn set_placement(&self, placement: Placement) {
        forward!(self, view, view.placement.set(Some(placement)))
    }
    pub fn focus(&self) {
        forward!(self, view, view.focus())
    }
    pub fn layout(&self) {
        forward!(self, view, view.layout())
    }
    pub fn prepare(&self) {
        forward!(self, view, view.prepare())
    }
    pub fn recolor(&self) {
        forward!(self, view, view.recolor())
    }
    pub fn status(&self) -> String {
        forward!(self, view, view.status())
    }
    pub fn key(&self, msg: &MSG) -> bool {
        forward!(self, view, view.key(msg))
    }
    pub fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        forward!(self, view, view.message(msg, w, l))
    }
    pub fn destroy(&self) {
        forward!(self, view, view.destroy())
    }
    pub fn activated(&self) {
        forward!(self, view, view.activated())
    }
    pub fn complete(&self, id: u64, completion: crate::tasks::tools::Completion) {
        forward!(self, view, view.complete(id, completion))
    }
    pub fn worker_stopped(&self) {
        forward!(self, view, view.worker_stopped())
    }
    pub fn description(&self) -> String {
        resources::text(self.kind().descriptor().description)
    }
    pub fn apply(&self, request: &Request) {
        match (self, request) {
            (Self::ErrorCode(view), Request::ErrorCode(code)) => view.search_code(*code),
        }
    }
}

pub(super) fn instantiate(ctx: &Context, kind: Kind, hwnd: HWND) -> anyhow::Result<ToolView> {
    let view = match kind {
        Kind::ErrorCode => {
            let mut view = error_code::View::new(ctx);
            view.create(hwnd)?;
            ToolView::ErrorCode(Rc::new(view))
        }
    };
    Ok(view)
}

pub(super) fn open(ctx: &Context, kind: Kind, new: bool) -> Option<Rc<ToolView>> {
    history::visit(ctx, || {
        let existing = (!new)
            .then(|| {
                workspace::contents(ctx)
                    .into_iter()
                    .filter_map(|content| content.tool())
                    .filter(|view| view.kind() == kind)
                    .max_by_key(|view| view.recent())
            })
            .flatten();
        let view = if let Some(view) = existing {
            workspace::show_document(ctx, view.hwnd());
            Some(view)
        } else {
            super::status::report(ctx, workspace::create_tool(ctx, kind))
        }?;
        view.focus();
        Some(view)
    })
}

pub(super) fn open_with(ctx: &Context, request: Request) -> Option<Rc<ToolView>> {
    let view = open(ctx, request.kind(), false)?;
    view.apply(&request);
    Some(view)
}

pub(super) fn open_row(ctx: &Context, row: usize, new: bool) {
    if let Some(descriptor) = TOOLS.get(row) {
        open(ctx, descriptor.kind, new);
    }
}

pub(super) fn list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), TOOL_LIST as i32) }
}
pub(super) fn refresh(ctx: &Context) {
    let list = list(ctx);
    crate::list::State::capture(list).apply(list, 1);
    super::panel::update_actions(ctx, super::panel::PanelId::Tools);
}
pub(super) fn update_actions(ctx: &Context) {
    unsafe {
        EnableWindow(
            GetDlgItem(ctx.frame(), TOOL_NEW as i32),
            i32::from(crate::list::selected(list(ctx)).is_some_and(|row| row < TOOLS.len())),
        );
    }
}
pub(super) fn selected(ctx: &Context, new: bool) {
    if let Some(row) = crate::list::selected(list(ctx)) {
        open_row(ctx, row, new);
    }
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    let Some(descriptor) = TOOLS.get(index) else {
        return;
    };
    let description = resources::text(descriptor.description);
    let mut display = Display {
        title: resources::text(descriptor.title).encode_utf16().collect(),
        icon: Some(descriptor.icon),
        tooltip: description.encode_utf16().collect(),
        ..Default::default()
    };
    display.detail.push(TextSegment::plain(&description));
    visit(&display);
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    let Some(descriptor) = TOOLS.get(index) else {
        return Vec::new();
    };
    format!(
        "{}\r\n{}",
        resources::text(descriptor.title),
        resources::text(descriptor.description)
    )
    .encode_utf16()
    .collect()
}
pub(super) fn context_menu(ctx: &Context, hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let Some((row, point)) = crate::list::context_target(hwnd, l) else {
        return Ok(());
    };
    let Some(descriptor) = TOOLS.get(row) else {
        return Ok(());
    };
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_TOOL_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_TOOL_NEW), true)?;
    match menu.track(ctx.frame(), point) {
        1 => {
            open(ctx, descriptor.kind, false);
        }
        2 => {
            open(ctx, descriptor.kind, true);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_descriptors_are_unique_and_addressable_by_kind() {
        let mut kinds = std::collections::HashSet::new();
        for descriptor in TOOLS {
            assert!(kinds.insert(descriptor.kind));
            let resolved = descriptor.kind.descriptor();
            assert_eq!(resolved.kind, descriptor.kind);
            assert_eq!(resolved.title, descriptor.title);
            assert_eq!(resolved.description, descriptor.description);
            assert_ne!(descriptor.title, descriptor.description);
        }
    }

    #[test]
    fn rows_beyond_the_descriptor_table_are_empty() {
        let mut visits = 0;
        row(TOOLS.len(), &mut |_| visits += 1);
        assert_eq!(visits, 0);
        assert!(row_text(TOOLS.len()).is_empty());
    }

    #[test]
    fn every_request_names_its_tool_descriptor() {
        assert_eq!(
            Request::ErrorCode(5).kind().descriptor().kind,
            Kind::ErrorCode
        );
    }

    #[test]
    fn search_result_actions_follow_the_descriptor_order() {
        use crate::app::result_value::ResultValue;
        assert!(actions(None).is_empty());
        let integer = ResultValue {
            text: "5".into(),
            hexadecimal: Some("0x00000005".into()),
            integer: Some(5),
        };
        let acts = actions(Some(&integer));
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].label, IDS_ERROR_CODE_SEARCH);
        match &acts[0].request {
            Request::ErrorCode(code) => assert_eq!(*code, 5),
        }
        let string_value = ResultValue {
            text: "Windows".into(),
            hexadecimal: None,
            integer: None,
        };
        assert!(actions(Some(&string_value)).is_empty());
    }
}

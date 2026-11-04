//! Internal registration and content contract. No dynamic plugins or persisted sessions.
use super::{history, result_value::ResultValue, workspace};
use crate::app::context::Context;
use crate::{
    icons::Icon,
    list::{Display, TextSegment},
    native::*,
    resources::{self, *},
};

mod error_code;
pub(super) use error_code::View as ToolView;

pub(super) fn complete(
    ctx: &Context,
    document: u64,
    id: u64,
    completion: crate::tasks::tools::Completion,
) {
    let Some(tool) = workspace::content_by_id(ctx, document).and_then(|c| c.tool()) else {
        return;
    };
    match completion {
        crate::tasks::tools::Completion::Query { revision, result } => {
            tool.query_complete(id, revision, result);
        }
        crate::tasks::tools::Completion::Languages { path, result } => {
            tool.languages_complete(id, path, result);
        }
    }
}

pub(super) fn worker_stopped(ctx: &Context) {
    for tool in workspace::contents(ctx)
        .into_iter()
        .filter_map(|c| c.tool())
    {
        tool.worker_stopped();
    }
}
pub(super) struct Descriptor {
    pub title: u16,
    pub description: u16,
    pub icon: Icon,
    item_actions: fn(&'static Descriptor, Option<&ResultValue>) -> Vec<ItemAction>,
}
pub(super) struct ItemAction {
    pub label: u16,
    run: Box<dyn FnOnce(&Context)>,
}
impl ItemAction {
    pub fn execute(self, ctx: &Context) {
        (self.run)(ctx);
    }
}
pub(super) fn item_actions(value: Option<&ResultValue>) -> Vec<ItemAction> {
    REGISTRY
        .iter()
        .flat_map(|descriptor| (descriptor.item_actions)(descriptor, value))
        .collect()
}
pub(super) static REGISTRY: &[Descriptor] = &[Descriptor {
    title: IDS_ERROR_CODE_TOOL,
    description: IDS_ERROR_CODE_DESCRIPTION,
    icon: Icon::Constant,
    item_actions: error_code::item_actions,
}];
pub(super) fn open(ctx: &Context, index: isize, new: bool) {
    let Some(descriptor) = REGISTRY.get(index as usize) else {
        return;
    };
    open_descriptor(ctx, descriptor, new);
}
fn open_descriptor(
    ctx: &Context,
    descriptor: &'static Descriptor,
    new: bool,
) -> Option<std::rc::Rc<ToolView>> {
    history::visit(ctx, || {
        let existing = (!new)
            .then(|| {
                workspace::contents(ctx)
                    .into_iter()
                    .filter_map(|content| content.tool())
                    .filter(|view| std::ptr::eq(view.descriptor, descriptor))
                    .max_by_key(|view| view.recent.get())
            })
            .flatten();
        let hwnd = if let Some(view) = existing {
            workspace::show_document(ctx, view.hwnd);
            Some(view.hwnd)
        } else {
            super::status::report(ctx, workspace::create_tool(ctx, descriptor))
        };
        let content = hwnd.and_then(|hwnd| workspace::content(ctx, hwnd))?;
        content.activated();
        content.focus();
        content.tool()
    })
}
pub(super) fn list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), TOOL_LIST as i32) }
}
pub(super) fn refresh(ctx: &Context) {
    let list = list(ctx);
    crate::list::State::capture(list).apply(list, REGISTRY.len());
}
pub(super) fn selected(ctx: &Context, new: bool) {
    open(
        ctx,
        crate::list::selected(list(ctx)).map_or(-1, |index| index as isize),
        new,
    );
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    if let Some(d) = REGISTRY.get(index) {
        let mut display = Display {
            title: resources::text(d.title).encode_utf16().collect(),
            icon: Some(d.icon),
            ..Default::default()
        };
        display
            .detail
            .push(TextSegment::plain(&resources::text(d.description)));
        display.tooltip = resources::text(d.description).encode_utf16().collect();
        visit(&display);
    }
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    REGISTRY
        .get(index)
        .map(|d| {
            format!(
                "{}\r\n{}",
                resources::text(d.title),
                resources::text(d.description)
            )
            .encode_utf16()
            .collect()
        })
        .unwrap_or_default()
}
pub(super) fn context_menu(ctx: &Context, hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let Some((index, point)) = crate::list::context_target(hwnd, l) else {
        return Ok(());
    };
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_TOOL_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_TOOL_NEW), true)?;
    match menu.track(ctx.frame(), point) {
        1 => open(ctx, index as isize, false),
        2 => open(ctx, index as isize, true),
        _ => {}
    }
    Ok(())
}

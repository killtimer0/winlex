//! Error-code tool list presentation and instance opening.
use super::{history, workspace};
use crate::app::context::Context;
use crate::{
    icons::Icon,
    list::{Display, TextSegment},
    native::*,
    resources::{self, *},
};

mod error_code;
pub(super) use error_code::{View as ToolView, item_code};

pub(super) fn open(ctx: &Context, new: bool) -> Option<std::rc::Rc<ToolView>> {
    history::visit(ctx, || {
        let existing = (!new)
            .then(|| {
                workspace::contents(ctx)
                    .into_iter()
                    .filter_map(|content| content.tool())
                    .max_by_key(|view| view.recent.get())
            })
            .flatten();
        let view = if let Some(view) = existing {
            workspace::show_document(ctx, view.hwnd);
            Some(view)
        } else {
            super::status::report(ctx, workspace::create_tool(ctx))
        }?;
        view.focus();
        Some(view)
    })
}
pub(super) fn list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), TOOL_LIST as i32) }
}
pub(super) fn refresh(ctx: &Context) {
    let list = list(ctx);
    crate::list::State::capture(list).apply(list, 1);
}
pub(super) fn selected(ctx: &Context, new: bool) {
    if crate::list::selected(list(ctx)) == Some(0) {
        open(ctx, new);
    }
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    if index != 0 {
        return;
    }
    let description = resources::text(IDS_ERROR_CODE_DESCRIPTION);
    let mut display = Display {
        title: resources::text(IDS_ERROR_CODE_TOOL)
            .encode_utf16()
            .collect(),
        icon: Some(Icon::Constant),
        tooltip: description.encode_utf16().collect(),
        ..Default::default()
    };
    display.detail.push(TextSegment::plain(&description));
    visit(&display);
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    if index != 0 {
        return Vec::new();
    }
    format!(
        "{}\r\n{}",
        resources::text(IDS_ERROR_CODE_TOOL),
        resources::text(IDS_ERROR_CODE_DESCRIPTION)
    )
    .encode_utf16()
    .collect()
}
pub(super) fn context_menu(ctx: &Context, hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let Some((0, point)) = crate::list::context_target(hwnd, l) else {
        return Ok(());
    };
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_TOOL_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_TOOL_NEW), true)?;
    match menu.track(ctx.frame(), point) {
        1 => {
            open(ctx, false);
        }
        2 => {
            open(ctx, true);
        }
        _ => {}
    }
    Ok(())
}

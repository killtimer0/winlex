//! Tool launcher list and its actions, separate from document hosting.
use super::{TOOLS, open};
use crate::app::{
    context::Context,
    panel::{Panel, PanelId},
};
use crate::{
    list::{Display, List, TextSegment},
    native::*,
    resources::{self, *},
};
use std::rc::Rc;

pub(in crate::app) fn create(ctx: &Context, frame: HWND) -> anyhow::Result<Rc<Panel>> {
    let state = ctx as *const Context;
    let list = List::new(
        frame,
        TOOL_LIST,
        row,
        row_text,
        Some(Box::new(move |row, double| {
            if double {
                open_row(
                    unsafe { &*state },
                    row,
                    unsafe { GetKeyState(VK_CONTROL as i32) } < 0,
                );
            }
        })),
        ctx.system_version(),
    )?;
    let button = unsafe {
        child(
            frame,
            w!("BUTTON"),
            resources::wide_text(IDS_TOOL_NEW),
            WS_TABSTOP,
            TOOL_NEW,
        )
    }?;
    update_actions(ctx);
    Ok(Rc::new(Panel {
        id: PanelId::Tools,
        focus: list.hwnd,
        fields: vec![],
        buttons: vec![button],
        list,
    }))
}
fn open_row(ctx: &Context, row: usize, new: bool) {
    if let Some(descriptor) = TOOLS.get(row) {
        open(ctx, descriptor.kind, new);
    }
}
fn list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), TOOL_LIST as i32) }
}
pub(in crate::app) fn refresh(ctx: &Context) {
    let list = list(ctx);
    crate::list::State::capture(list).apply(list, TOOLS.len());
    update_actions(ctx);
}
pub(in crate::app) fn update_actions(ctx: &Context) {
    unsafe {
        EnableWindow(
            GetDlgItem(ctx.frame(), TOOL_NEW as i32),
            i32::from(crate::list::selected(list(ctx)).is_some_and(|row| row < TOOLS.len())),
        );
    }
}
pub(in crate::app) fn selected(ctx: &Context, new: bool) {
    if let Some(row) = crate::list::selected(list(ctx)) {
        open_row(ctx, row, new);
    }
}
pub(in crate::app) fn command(ctx: &Context, id: u16, code: u32) -> bool {
    if id != TOOL_NEW || code != BN_CLICKED {
        return false;
    }
    selected(ctx, true);
    true
}
fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
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
fn row_text(index: usize) -> Vec<u16> {
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
pub(in crate::app) fn context_menu(ctx: &Context, hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let Some((row, point)) = crate::list::context_target(hwnd, l) else {
        return Ok(());
    };
    let Some(descriptor) = TOOLS.get(row) else {
        return Ok(());
    };
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_OPEN), true)?;
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

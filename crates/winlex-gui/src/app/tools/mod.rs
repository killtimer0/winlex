//! Internal registration and content contract. No dynamic plugins or persisted sessions.
use super::{history, read, workspace};
use crate::{
    icons::Icon,
    list::{Display, TextSegment},
    native::*,
    resources::{self, *},
};
use std::cell::Cell;

mod error_code;
pub(super) use error_code::View as ToolView;
pub(super) struct Descriptor {
    pub id: &'static str,
    pub title: u16,
    pub description: u16,
    pub icon: Icon,
}
pub(super) static REGISTRY: &[Descriptor] = &[Descriptor {
    id: "error_code",
    title: IDS_ERROR_CODE_TOOL,
    description: IDS_ERROR_CODE_DESCRIPTION,
    icon: Icon::Constant,
}];
thread_local! {static CLOCK:Cell<u64>=const{Cell::new(0)};}
pub(super) fn open(index: isize, new: bool) {
    let Some(descriptor) = REGISTRY.get(index as usize) else {
        return;
    };
    history::visit(|| {
        let existing = (!new)
            .then(|| {
                workspace::contents()
                    .into_iter()
                    .filter_map(|content| content.tool())
                    .filter(|view| view.descriptor.id == descriptor.id)
                    .max_by_key(|view| view.recent.get())
            })
            .flatten();
        let hwnd = if let Some(view) = existing {
            workspace::show_document(view.hwnd);
            Some(view.hwnd)
        } else {
            super::frame::report(workspace::create_tool(descriptor))
        };
        if let Some(content) = hwnd.and_then(workspace::content) {
            content.activated();
            content.focus();
        }
    });
}
pub(super) fn activate(index: isize, _double: bool) {
    open(index, false)
}
pub(super) fn list() -> HWND {
    unsafe { GetDlgItem(read(|a| a.frame), TOOL_LIST as i32) }
}
pub(super) fn selected(new: bool) {
    open(
        crate::list::selected(list()).map_or(-1, |index| index as isize),
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
pub(super) fn context_menu(hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_TOOL_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_TOOL_NEW), true)?;
    let point = if l == -1 {
        let b = bounds(hwnd, std::ptr::null_mut());
        POINT {
            x: b.left + 8,
            y: b.top + 8,
        }
    } else {
        point_from_lparam(l)
    };
    match menu.track(read(|a| a.frame), point) {
        1 => selected(false),
        2 => selected(true),
        _ => {}
    }
    Ok(())
}

//! Frame menu commands and their enabled/checked state.
use super::{catalog, help, history, panel, settings, sidebar, status, tools, workspace};
use crate::app::context::Context;
use crate::{layout::Arrangement, native::*, resources::*};
use windows_sys::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD,
};

fn is_topmost(hwnd: HWND) -> bool {
    unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 }
}

pub(super) fn command(ctx: &Context, id: u16) -> bool {
    if let Some(direction) = history::Direction::from_command(id) {
        status::begin_operation(ctx);
        history::navigate(ctx, direction);
        return true;
    }
    if matches!(
        id,
        FIND_DOCUMENT
            | FIND_NEXT
            | FIND_PREVIOUS
            | SETTINGS
            | ABOUT
            | SEARCH_HELP
            | FOCUS_NEXT_REGION
            | CLOSE
            | OPEN_RESULT
            | OPEN_NEW
            | EXIT
            | COLUMNS
            | ROWS
            | FREE
            | MAXIMIZE
            | ALWAYS_ON_TOP
    ) {
        status::begin_operation(ctx);
    }
    match id {
        FIND_DOCUMENT | FIND_NEXT | FIND_PREVIOUS => {
            if let Some(document) = workspace::content(ctx, workspace::active(ctx)) {
                document.command(id, 0);
            }
        }
        SETTINGS => settings::open(ctx),
        ALWAYS_ON_TOP => {
            let frame = ctx.read(|a| a.frame);
            unsafe {
                let after = if is_topmost(frame) {
                    HWND_NOTOPMOST
                } else {
                    HWND_TOPMOST
                };
                if SetWindowPos(
                    frame,
                    after,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                ) == 0
                {
                    status::report::<()>(ctx, Err(error("Set window topmost state")));
                }
            }
            update_commands(ctx);
        }
        ABOUT => {
            status::report(ctx, help::about(ctx));
        }
        SEARCH_HELP => {
            status::report(ctx, help::search(ctx));
        }
        FOCUS_NEXT_REGION => {
            let h = unsafe { GetFocus() };
            let d = workspace::active(ctx);
            if !d.is_null() && (h == d || unsafe { IsChild(d, h) } != 0) {
                sidebar::focus(ctx);
            } else if h == workspace::tabs(ctx) {
                if d.is_null() {
                    sidebar::focus(ctx);
                } else if let Some(document) = workspace::content(ctx, d) {
                    document.focus();
                }
            } else {
                focus(workspace::tabs(ctx));
            }
        }
        CLOSE => workspace::close_document(ctx, workspace::active(ctx)),
        OPEN_RESULT if sidebar::active(ctx) == panel::PanelId::Tools => tools::selected(ctx, false),
        OPEN_NEW if sidebar::active(ctx) == panel::PanelId::Tools => tools::selected(ctx, true),
        OPEN_RESULT => catalog::open_selected(ctx, catalog::Open::Pin),
        OPEN_NEW => catalog::open_selected(ctx, catalog::Open::New),
        EXIT => {
            send(ctx.read(|a| a.frame), WM_CLOSE, 0, 0);
        }
        COLUMNS => workspace::set_arrangement(ctx, Arrangement::Columns),
        ROWS => workspace::set_arrangement(ctx, Arrangement::Rows),
        FREE => workspace::set_arrangement(ctx, Arrangement::Free),
        MAXIMIZE => {
            let h = workspace::active(ctx);
            if !h.is_null() {
                workspace::set_maximized(ctx, true);
                send(workspace::mdi(ctx), WM_MDIMAXIMIZE, h as usize, 0);
            }
        }
        _ => return false,
    }
    true
}

pub(super) fn update_commands(ctx: &Context) {
    let enabled =
        workspace::content(ctx, workspace::active(ctx)).is_some_and(|c| c.supports_find());
    let frame = ctx.read(|a| a.frame);
    if frame.is_null() {
        return;
    }
    unsafe {
        CheckMenuItem(
            GetMenu(frame),
            ALWAYS_ON_TOP as u32,
            MF_BYCOMMAND
                | if is_topmost(frame) {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                },
        );
    }
    for id in [FIND_DOCUMENT, FIND_NEXT, FIND_PREVIOUS] {
        unsafe {
            EnableMenuItem(
                GetMenu(frame),
                id as u32,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
    }
}

pub(super) fn appcommand(l: isize) -> Option<history::Direction> {
    match (l as u32 >> 16) & 0x0fff {
        APPCOMMAND_BROWSER_BACKWARD => Some(history::Direction::Back),
        APPCOMMAND_BROWSER_FORWARD => Some(history::Direction::Forward),
        _ => None,
    }
}

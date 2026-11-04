//! Frame menu commands and their enabled/checked state.
use super::{catalog, help, history, panel, read, settings, sidebar, status, tools, workspace};
use crate::{layout::Arrangement, native::*, resources::*};
use windows_sys::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD,
};

fn is_topmost(hwnd: HWND) -> bool {
    unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 }
}

pub(super) fn command(id: u16) -> bool {
    if let Some(direction) = history::Direction::from_command(id) {
        status::begin_operation();
        history::navigate(direction);
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
        status::begin_operation();
    }
    match id {
        FIND_DOCUMENT | FIND_NEXT | FIND_PREVIOUS => {
            if let Some(document) = workspace::content(workspace::active()) {
                document.command(id, 0);
            }
        }
        SETTINGS => settings::open(),
        ALWAYS_ON_TOP => {
            let frame = read(|a| a.frame);
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
                    status::report::<()>(Err(error("Set window topmost state")));
                }
            }
            update_commands();
        }
        ABOUT => {
            status::report(help::about());
        }
        SEARCH_HELP => {
            status::report(help::search());
        }
        FOCUS_NEXT_REGION => {
            let h = unsafe { GetFocus() };
            let d = workspace::active();
            if !d.is_null() && (h == d || unsafe { IsChild(d, h) } != 0) {
                sidebar::focus();
            } else if h == read(|a| a.workspace.tabs) {
                if d.is_null() {
                    sidebar::focus();
                } else if let Some(document) = workspace::content(d) {
                    document.focus();
                }
            } else {
                focus(read(|a| a.workspace.tabs));
            }
        }
        CLOSE => workspace::close_document(workspace::active()),
        OPEN_RESULT if read(|a| a.sidebar.active == panel::PanelId::Tools) => {
            tools::selected(false)
        }
        OPEN_NEW if read(|a| a.sidebar.active == panel::PanelId::Tools) => tools::selected(true),
        OPEN_RESULT => catalog::open_selected(catalog::Open::Pin),
        OPEN_NEW => catalog::open_selected(catalog::Open::New),
        EXIT => {
            send(read(|a| a.frame), WM_CLOSE, 0, 0);
        }
        COLUMNS => workspace::set_arrangement(Arrangement::Columns),
        ROWS => workspace::set_arrangement(Arrangement::Rows),
        FREE => workspace::set_arrangement(Arrangement::Free),
        MAXIMIZE => {
            let h = workspace::active();
            if !h.is_null() {
                workspace::set_maximized(true);
                send(read(|a| a.workspace.mdi), WM_MDIMAXIMIZE, h as usize, 0);
            }
        }
        _ => return false,
    }
    true
}

pub(super) fn update_commands() {
    let enabled = workspace::content(workspace::active()).is_some_and(|c| c.supports_find());
    let frame = read(|a| a.frame);
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

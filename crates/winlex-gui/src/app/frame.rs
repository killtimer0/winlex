//! Native frame orchestration; sidebar owns its controls and notifications.
use super::workspace::{self, active};
use super::{catalog, dpi, history, panel, read, sidebar, sources, write};
use crate::{
    layout::{self, Arrangement},
    native::*,
    resources::*,
};
use std::ptr;
use windows_sys::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum StatusContext {
    #[default]
    Catalog,
    Document(HWND),
}
#[derive(Clone, Default)]
pub(super) struct Status {
    context: StatusContext,
    notice: Option<String>,
}
impl Status {
    fn begin(&mut self, context: StatusContext) {
        self.context = context;
        self.notice = None;
    }
    fn message(
        &self,
        content: impl FnOnce(HWND) -> Option<String>,
        catalog: impl FnOnce() -> String,
    ) -> String {
        self.notice.clone().unwrap_or_else(|| {
            match self.context {
                StatusContext::Document(hwnd) => content(hwnd),
                StatusContext::Catalog => None,
            }
            .unwrap_or_else(catalog)
        })
    }
}
pub(super) fn begin_status(context: StatusContext) {
    write(|a| a.status_state.begin(context));
    refresh_status();
}
pub(super) fn status_context(context: StatusContext) {
    if read(|a| a.status_state.context != context) {
        begin_status(context);
    }
}
fn begin_operation() {
    write(|a| a.status_state.notice = None);
    refresh_status();
}
pub(super) fn status_input(hwnd: HWND) {
    let doc = active();
    if !doc.is_null() && (hwnd == doc || unsafe { IsChild(doc, hwnd) } != 0) {
        begin_status(StatusContext::Document(doc));
    } else if read(|a| a.sidebar.owner(hwnd).is_some()) {
        begin_status(StatusContext::Catalog);
    }
}
pub(super) fn set_status(message: String) {
    write(|a| a.status_state.notice = Some(message));
    refresh_status();
}
pub(super) fn report<T>(result: anyhow::Result<T>) -> Option<T> {
    result
        .map_err(|error| set_status(format!("{error:#}")))
        .ok()
}
pub(super) fn refresh_status() {
    let (state, hwnd) = read(|a| (a.status_state.clone(), a.status));
    let message = state.message(
        |hwnd| workspace::content(hwnd).and_then(|content| content.status()),
        || read(|a| a.session.status()),
    );
    set_status_text(hwnd, &message);
}

fn prepare() {
    let (frame, tabs, status) = read(|a| (a.frame, a.workspace.tabs, a.status));
    let docs = workspace::contents();
    let value = dpi().window(frame);
    if let Some(icons) = super::checked(crate::icons::WindowIcons::load(value)) {
        icons.apply(frame);
        write(|a| a.icons = Some(icons));
    }
    let (font, _) = super::font_for(value);
    super::bind(tabs, font);
    super::bind(status, font);
    sidebar::prepare();
    layout_frame();
    for content in docs {
        content.prepare();
    }
    workspace::refresh_document_tabs();
}
fn recolor() {
    unsafe {
        InvalidateRect(read(|a| a.workspace.tabs), ptr::null(), 0);
    }
    sidebar::recolor();
    for content in workspace::contents() {
        content.recolor();
    }
    workspace::refresh_document_tabs();
}
pub(super) fn layout_frame() {
    let (frame, mdi, tabs, status) =
        read(|a| (a.frame, a.workspace.mdi, a.workspace.tabs, a.status));
    if mdi.is_null() {
        return;
    }
    send(status, WM_SIZE, 0, 0);
    let m = super::metrics(frame);
    let mut area = client(frame);
    area.bottom = bounds(status, frame).top;
    let mut positions = Vec::new();
    let sidebar = sidebar::layout(area.bottom, &mut positions);
    layout::take_left(&mut area, sidebar);
    positions.push((tabs, layout::take_top(&mut area, m.height + m.px(8))));
    positions.push((mdi, area));
    if super::checked(place_all(&positions)).is_some() {
        let panel = read(|a| {
            let s = &a.sidebar;
            (!s.collapsed).then(|| s.panel(s.active))
        });
        if let Some(panel) = panel {
            crate::list::fit(panel.list.hwnd);
        }
    }
}
pub(super) fn command(id: u16) -> bool {
    if let Some(direction) = history::Direction::from_command(id) {
        begin_operation();
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
    ) {
        begin_operation();
    }
    match id {
        FIND_DOCUMENT | FIND_NEXT | FIND_PREVIOUS => {
            if let Some(document) = workspace::content(active()) {
                document.command(id, 0);
            }
        }
        SETTINGS => super::settings::open(),
        ABOUT => {
            report(super::help::about());
        }
        SEARCH_HELP => {
            report(super::help::search());
        }
        FOCUS_NEXT_REGION => {
            let h = unsafe { GetFocus() };
            let d = active();
            if !d.is_null() && (h == d || unsafe { IsChild(d, h) != 0 }) {
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
        CLOSE => workspace::close_document(active()),
        OPEN_RESULT if read(|a| a.sidebar.active == panel::PanelId::Tools) => {
            super::tools::selected(false)
        }
        OPEN_NEW if read(|a| a.sidebar.active == panel::PanelId::Tools) => {
            super::tools::selected(true)
        }
        OPEN_RESULT => catalog::open_selected(catalog::Open::Pin),
        OPEN_NEW => catalog::open_selected(catalog::Open::New),
        EXIT => {
            send(read(|a| a.frame), WM_CLOSE, 0, 0);
        }
        COLUMNS => workspace::set_arrangement(Arrangement::Columns),
        ROWS => workspace::set_arrangement(Arrangement::Rows),
        FREE => workspace::set_arrangement(Arrangement::Free),
        MAXIMIZE => {
            let h = active();
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
    let enabled = workspace::content(active()).is_some_and(|c| c.supports_find());
    let frame = read(|a| a.frame);
    if frame.is_null() {
        return;
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
pub(super) unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        match msg {
            WM_NCCREATE => {
                write(|a| a.frame = hwnd);
                dpi().enable_nonclient(hwnd);
            }
            WM_CREATE => {
                let result = (|| -> anyhow::Result<()> {
                    let tabs = crate::tabs::create(
                        hwnd,
                        DOCUMENT_TABS,
                        crate::resources::text(IDS_DOCUMENT_TABS),
                        crate::resources::text(IDS_TAB_SELECT),
                    )?;
                    let status = child(
                        hwnd,
                        w!("msctls_statusbar32"),
                        &[0],
                        SBARS_SIZEGRIP | SBARS_TOOLTIPS,
                        STATUS,
                    )?;
                    let c = CLIENTCREATESTRUCT {
                        hWindowMenu: GetSubMenu(GetMenu(hwnd), 2),
                        idFirstChild: 40000,
                    };
                    let mdi = CreateWindowExW(
                        0,
                        w!("MDICLIENT"),
                        ptr::null(),
                        WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_HSCROLL | WS_VSCROLL,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        ptr::null_mut(),
                        instance(),
                        (&c as *const CLIENTCREATESTRUCT).cast(),
                    );
                    if mdi.is_null() {
                        return Err(error("Create MDI client"));
                    }
                    write(|a| {
                        a.workspace.tabs = tabs;
                        a.status = status;
                        a.workspace.mdi = mdi;
                    });
                    sidebar::create(hwnd)?;
                    windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd, 1);
                    prepare();
                    crate::theme::apply(hwnd);
                    Ok(())
                })();
                if super::checked(result).is_none() {
                    return -1;
                }
                return 0;
            }
            WM_SIZE => {
                if w != SIZE_MINIMIZED as usize {
                    layout_frame();
                }
                return 0;
            }
            WM_DPICHANGED if dpi().per_monitor() => {
                let _ = set_bounds(hwnd, *(l as *const RECT));
                prepare();
                workspace::scale_documents();
                return 0;
            }
            WM_SETTINGCHANGE => {
                if w == 0 || w as u32 == SPI_SETNONCLIENTMETRICS {
                    let old = write(|a| std::mem::take(&mut a.fonts));
                    prepare();
                    drop(old);
                }
                recolor();
            }
            WM_SYSCOLORCHANGE | WM_THEMECHANGED => recolor(),
            WM_COMMAND => {
                if panel::command(w, l as HWND) || command(w as u16) {
                    return 0;
                }
            }
            WM_INITMENUPOPUP => {
                history::refresh();
                update_commands();
            }
            WM_APPCOMMAND => {
                let direction = match (l as u32 >> 16) & 0x0fff {
                    APPCOMMAND_BROWSER_BACKWARD => Some(history::Direction::Back),
                    APPCOMMAND_BROWSER_FORWARD => Some(history::Direction::Forward),
                    _ => None,
                };
                if let Some(direction) = direction {
                    history::navigate(direction);
                    return 1;
                }
            }
            WM_NOTIFY => {
                let hdr = &*(l as *const NMHDR);
                if hdr.hwndFrom == read(|a| a.workspace.tabs) {
                    workspace::tab_notification(&*(l as *const crate::tabs::Notification));
                    return 0;
                }
                if let Some(panel) = read(|a| a.sidebar.owner(hdr.hwndFrom)) {
                    if hdr.code == NM_SETFOCUS {
                        status_context(StatusContext::Catalog);
                    }
                    if hdr.idFrom == SOURCE_LIST as usize && hdr.code == LVN_ITEMCHANGED {
                        sources::update_actions();
                        return 0;
                    }
                    if let Some(result) = panel.list.notify(l) {
                        return result;
                    }
                }
            }
            WM_CONTEXTMENU if w as HWND == GetDlgItem(hwnd, SOURCE_LIST as i32) => {
                begin_status(StatusContext::Catalog);
                report(sources::context_menu(w as HWND, l));
                return 0;
            }
            WM_CONTEXTMENU if w as HWND == GetDlgItem(hwnd, TOOL_LIST as i32) => {
                begin_status(StatusContext::Catalog);
                report(super::tools::context_menu(w as HWND, l));
                return 0;
            }
            // A top-level WM_GETMINMAXINFO minimum can block a DPI downscale.
            // Use WM_SIZING for interactive limits; retain child minmax for layouts.
            WM_SIZING => {
                let r = &mut *(l as *mut RECT);
                let value = dpi().window(hwnd);
                layout::limit_sizing(
                    r,
                    w as u32,
                    SIZE {
                        cx: layout::scale(480, value),
                        cy: layout::scale(300, value),
                    },
                );
                return 1;
            }
            WM_CLOSE => {
                let _ = super::settings::save_window(hwnd);
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return 0;
            }
            WM_DROPFILES => {
                sources::add(crate::input::dropped_files(w as _));
                return 0;
            }
            _ => {}
        }
        DefFrameProcW(hwnd, read(|a| a.workspace.mdi), msg, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_notice_survives_background_updates_until_next_operation() {
        let mut status = Status::default();
        status.begin(StatusContext::Document(ptr::null_mut()));
        status.notice = Some("operation failed".into());
        for background in ["querying", "done"] {
            assert_eq!(
                status.message(|_| Some(background.into()), || background.into()),
                "operation failed"
            );
        }
        status.begin(StatusContext::Document(ptr::null_mut()));
        assert_eq!(
            status.message(|_| Some("tool".into()), || "catalog".into()),
            "tool"
        );
        status.notice = Some("another failure".into());
        status.begin(StatusContext::Catalog);
        assert_eq!(
            status.message(
                |_| panic!("inactive tool queried"),
                || "search error".into()
            ),
            "search error"
        );
    }

    #[test]
    fn missing_or_unspecialized_document_falls_back_to_catalog() {
        let mut status = Status::default();
        status.begin(StatusContext::Document(ptr::null_mut()));
        assert_eq!(status.message(|_| None, || "catalog".into()), "catalog");
        assert_eq!(
            status.message(|_| Some(String::new()), || "catalog".into()),
            ""
        );
    }
}

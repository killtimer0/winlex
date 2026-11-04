//! Native frame window lifecycle, layout and painting.
use super::{commands, dpi, history, panel, read, sidebar, sources, status, workspace, write};
use crate::{layout, native::*, resources::*};
use std::ptr;

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
                if panel::command(w, l as HWND) || commands::command(w as u16) {
                    return 0;
                }
            }
            WM_INITMENUPOPUP => {
                history::refresh();
                commands::update_commands();
            }
            WM_APPCOMMAND => {
                if let Some(direction) = commands::appcommand(l) {
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
                        status::status_context(status::StatusContext::Catalog);
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
                status::begin_status(status::StatusContext::Catalog);
                status::report(sources::context_menu(w as HWND, l));
                return 0;
            }
            WM_CONTEXTMENU if w as HWND == GetDlgItem(hwnd, TOOL_LIST as i32) => {
                status::begin_status(status::StatusContext::Catalog);
                status::report(super::tools::context_menu(w as HWND, l));
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
                        cx: layout::scale(layout::MIN_WIDTH, value),
                        cy: layout::scale(layout::MIN_HEIGHT, value),
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

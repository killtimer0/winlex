//! Native frame window lifecycle, layout and painting.
use super::{commands, history, panel, sidebar, sources, status, workspace};
use crate::app::context::Context;
use crate::{layout, native::*, resources::*};
use std::ptr;

fn prepare(ctx: &Context) {
    let frame = ctx.frame();
    let tabs = workspace::tabs(ctx);
    let status = ctx.status_bar();
    let docs = workspace::contents(ctx);
    let value = ctx.dpi().window(frame);
    if let Some(icons) = ctx.checked(crate::icons::WindowIcons::load(value)) {
        icons.apply(frame);
        ctx.set_icons(icons);
    }
    let (font, _) = ctx.font_for(value);
    super::bind(tabs, font);
    super::bind(status, font);
    sidebar::prepare(ctx);
    layout_frame(ctx);
    for content in docs {
        content.prepare();
    }
    workspace::refresh_document_tabs(ctx);
}
fn recolor(ctx: &Context) {
    unsafe {
        InvalidateRect(workspace::tabs(ctx), ptr::null(), 0);
    }
    sidebar::recolor(ctx);
    for content in workspace::contents(ctx) {
        content.recolor();
    }
    workspace::refresh_document_tabs(ctx);
}
pub(super) fn layout_frame(ctx: &Context) {
    let frame = ctx.frame();
    let mdi = workspace::mdi(ctx);
    let tabs = workspace::tabs(ctx);
    let status = ctx.status_bar();
    if mdi.is_null() {
        return;
    }
    send(status, WM_SIZE, 0, 0);
    let m = ctx.metrics(frame);
    let mut area = client(frame);
    area.bottom = bounds(status, frame).top;
    let mut positions = Vec::new();
    let sidebar = sidebar::layout(ctx, area.bottom, &mut positions);
    layout::take_left(&mut area, sidebar);
    positions.push((tabs, layout::take_top(&mut area, m.height + m.px(8))));
    positions.push((mdi, area));
    if ctx.checked(place_all(&positions)).is_some() {
        let panel = sidebar::current(ctx);
        if let Some(panel) = panel {
            crate::list::fit(panel.list.hwnd);
        }
    }
}
pub(super) unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(l as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as _);
        }
        let Some(ctx) = Context::from_window(hwnd) else {
            return DefWindowProcW(hwnd, msg, w, l);
        };
        match msg {
            WM_NCCREATE => {
                ctx.set_frame(hwnd);
                ctx.dpi().enable_nonclient(hwnd);
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
                    ctx.set_status_bar(status);
                    workspace::attach(ctx, tabs, mdi);
                    sidebar::create(ctx, hwnd)?;
                    windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd, 1);
                    prepare(ctx);
                    crate::theme::apply(hwnd);
                    Ok(())
                })();
                if ctx.checked(result).is_none() {
                    return -1;
                }
                return 0;
            }
            WM_SIZE => {
                if w != SIZE_MINIMIZED as usize {
                    layout_frame(ctx);
                }
                return 0;
            }
            WM_DPICHANGED if ctx.dpi().per_monitor() => {
                let _ = set_bounds(hwnd, *(l as *const RECT));
                prepare(ctx);
                workspace::scale_documents(ctx);
                return 0;
            }
            WM_SETTINGCHANGE => {
                if w == 0 || w as u32 == SPI_SETNONCLIENTMETRICS {
                    let old = ctx.take_fonts();
                    prepare(ctx);
                    drop(old);
                }
                recolor(ctx);
            }
            WM_SYSCOLORCHANGE | WM_THEMECHANGED => recolor(ctx),
            WM_COMMAND => {
                if panel::command(ctx, w, l as HWND) || commands::command(ctx, w as u16) {
                    return 0;
                }
            }
            WM_INITMENUPOPUP => {
                history::refresh(ctx);
                commands::update_commands(ctx);
            }
            WM_APPCOMMAND => {
                if let Some(direction) = commands::appcommand(l) {
                    history::navigate(ctx, direction);
                    return 1;
                }
            }
            WM_NOTIFY => {
                let hdr = &*(l as *const NMHDR);
                if hdr.hwndFrom == workspace::tabs(ctx) {
                    workspace::tab_notification(ctx, &*(l as *const crate::tabs::Notification));
                    return 0;
                }
                if let Some(panel) = sidebar::owner(ctx, hdr.hwndFrom) {
                    if hdr.code == NM_SETFOCUS {
                        status::status_context(ctx, status::StatusContext::Catalog);
                    }
                    if hdr.idFrom == SOURCE_LIST as usize && hdr.code == LVN_ITEMCHANGED {
                        sources::update_actions(ctx);
                        return 0;
                    }
                    if let Some(result) = panel.list.notify(l) {
                        return result;
                    }
                }
            }
            WM_CONTEXTMENU if w as HWND == GetDlgItem(hwnd, SOURCE_LIST as i32) => {
                status::begin_status(ctx, status::StatusContext::Catalog);
                status::report(ctx, sources::context_menu(ctx, w as HWND, l));
                return 0;
            }
            WM_CONTEXTMENU if w as HWND == GetDlgItem(hwnd, TOOL_LIST as i32) => {
                status::begin_status(ctx, status::StatusContext::Catalog);
                status::report(ctx, super::tools::context_menu(ctx, w as HWND, l));
                return 0;
            }
            // A top-level WM_GETMINMAXINFO minimum can block a DPI downscale.
            // Use WM_SIZING for interactive limits; retain child minmax for layouts.
            WM_SIZING => {
                let r = &mut *(l as *mut RECT);
                let value = ctx.dpi().window(hwnd);
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
                let _ = super::settings::save_window(ctx, hwnd);
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return 0;
            }
            WM_DROPFILES => {
                sources::add(ctx, crate::input::dropped_files(w as _));
                return 0;
            }
            _ => {}
        }
        DefFrameProcW(hwnd, workspace::mdi(ctx), msg, w, l)
    }
}

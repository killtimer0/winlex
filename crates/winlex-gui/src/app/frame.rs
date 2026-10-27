//! Native frame orchestration; sidebar owns its controls and notifications.
use super::workspace::{self, ARRANGE_DOCUMENTS, active, arrange_documents};
use super::{dpi, read, sidebar, sources, write};
use crate::{
    layout::{self, Arrangement},
    native::*,
    resources::*,
};
use std::ptr;

fn update_environment(suggested: Option<RECT>) {
    let old = write(|a| std::mem::take(&mut a.fonts));
    let (frame, tabs, status, docs) = read(|a| {
        (
            a.frame,
            a.workspace.tabs,
            a.status,
            a.workspace
                .documents
                .iter()
                .map(|d| (d.hwnd, d.content.clone()))
                .collect::<Vec<_>>(),
        )
    });
    let (font, _) = super::font_for(dpi().window(frame));
    super::bind(tabs, font);
    super::bind(status, font);
    sidebar::prepare();
    if let Some(r) = suggested {
        unsafe {
            SetWindowPos(
                frame,
                ptr::null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    layout_frame();
    for (hwnd, content) in docs {
        content.prepare(hwnd);
    }
    drop(old);
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
    let sidebar = sidebar::layout(area.bottom);
    layout::take_left(&mut area, sidebar);
    place(tabs, layout::take_top(&mut area, m.height + m.px(16)));
    place(mdi, area);
    arrange_documents();
}
pub(super) fn command(id: u16) -> bool {
    match id {
        LANGUAGE_AUTO | LANGUAGE_ENGLISH | LANGUAGE_CHINESE => super::settings::language(id),
        CLOSE => workspace::close_document(active()),
        EXIT => {
            send(read(|a| a.frame), WM_CLOSE, 0, 0);
        }
        COLUMNS => workspace::set_arrangement(Arrangement::Columns),
        ROWS => workspace::set_arrangement(Arrangement::Rows),
        FREE => workspace::set_arrangement(Arrangement::Free),
        MAXIMIZE => {
            let h = active();
            if !h.is_null() {
                send(read(|a| a.workspace.mdi), WM_MDIMAXIMIZE, h as usize, 0);
            }
        }
        _ => return false,
    }
    true
}
pub(super) unsafe extern "system" fn frame_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    unsafe {
        match msg {
            WM_NCCREATE => {
                write(|a| a.frame = hwnd);
                dpi().enable_nonclient(hwnd);
            }
            WM_CREATE => {
                let value = dpi().initial(hwnd);
                write(|a| a.dpi.root = value);
                let tabs = child(
                    hwnd,
                    "SysTabControl32",
                    "",
                    WS_TABSTOP | TCS_TOOLTIPS,
                    DOCUMENT_TABS,
                );
                let status = child(
                    hwnd,
                    "msctls_statusbar32",
                    "",
                    SBARS_SIZEGRIP | SBARS_TOOLTIPS,
                    STATUS,
                );
                let c = CLIENTCREATESTRUCT {
                    hWindowMenu: GetSubMenu(GetMenu(hwnd), 1),
                    idFirstChild: 40000,
                };
                let mdi = CreateWindowExW(
                    0,
                    wide("MDICLIENT").as_ptr(),
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
                if [tabs, status, mdi].iter().any(|h| h.is_null())
                    || set_subclass(tabs, Some(workspace::document_tabs_proc), 1, 0) == 0
                {
                    return -1;
                }
                write(|a| {
                    a.workspace.tabs = tabs;
                    a.status = status;
                    a.workspace.mdi = mdi;
                });
                if !sidebar::create(hwnd) {
                    return -1;
                }
                windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd, 1);
                update_environment(None);
                return 0;
            }
            WM_SIZE => {
                if w != SIZE_MINIMIZED as usize {
                    layout_frame();
                }
                return 0;
            }
            WM_DPICHANGED if dpi().per_monitor() => {
                write(|a| a.dpi.root = (w & 0xffff) as u32);
                update_environment(Some(*(l as *const RECT)));
                return 0;
            }
            WM_SETTINGCHANGE => {
                update_environment(None);
            }
            WM_COMMAND => {
                if sidebar::command(w) || command(w as u16) {
                    return 0;
                }
            }
            WM_NOTIFY => {
                let hdr = &*(l as *const NMHDR);
                if hdr.idFrom == DOCUMENT_TABS as usize && hdr.code == TCN_SELCHANGE {
                    workspace::activate_document_tab();
                    return 0;
                }
                if let Some(result) = sidebar::notify(l) {
                    return result;
                }
            }
            WM_SIZING => {
                let r = &mut *(l as *mut RECT);
                let value = dpi().window(hwnd);
                let mw = layout::scale(480, value);
                let mh = layout::scale(300, value);
                if r.right - r.left < mw {
                    if matches!(w as u32, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
                        r.left = r.right - mw;
                    } else {
                        r.right = r.left + mw;
                    }
                }
                if r.bottom - r.top < mh {
                    if matches!(w as u32, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
                        r.top = r.bottom - mh;
                    } else {
                        r.bottom = r.top + mh;
                    }
                }
                return 1;
            }
            WM_CLOSE => {
                if let Err(e) = super::settings::save_window(hwnd) {
                    crate::show_error(&e);
                }
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return 0;
            }
            ARRANGE_DOCUMENTS => {
                arrange_documents();
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

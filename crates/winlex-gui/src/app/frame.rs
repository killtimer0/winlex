//! Native frame orchestration; sidebar owns its controls and notifications.
use super::workspace::{self, ARRANGE_DOCUMENTS, active, arrange_documents};
use super::{catalog, dpi, read, sidebar, sources, write};
use crate::{
    layout::{self, Arrangement},
    native::*,
    resources::*,
};
use std::ptr;

pub(super) fn composing() -> bool {
    read(|a| a.composing)
}
fn prepare_frame() {
    let (frame, tabs, status, docs) = read(|a| {
        (
            a.frame,
            a.document_tabs,
            a.status,
            a.documents.iter().map(|d| d.hwnd).collect::<Vec<_>>(),
        )
    });
    let (font, _) = super::font_for(dpi().window(frame));
    super::bind(tabs, font);
    super::bind(status, font);
    sidebar::prepare();
    for h in docs {
        if let Some(d) = super::document(h) {
            super::document::prepare_document(&d);
        }
    }
}
pub(super) fn layout_frame() {
    let (frame, mdi, tabs, status) = read(|a| (a.frame, a.mdi, a.document_tabs, a.status));
    if mdi.is_null() {
        return;
    }
    send(status, WM_SIZE, 0, 0);
    let value = dpi().window(frame);
    let area = client(frame);
    let bottom = bounds(status, frame).top;
    let x = sidebar::layout(bottom);
    let (_, height) = super::font_for(value);
    let tab = height + layout::scale(16, value);
    place(tabs, layout::rect(x, 0, area.right - x, tab));
    place(mdi, layout::rect(x, tab, area.right - x, bottom - tab));
    arrange_documents();
}
pub(super) fn command(id: u16) {
    match id {
        LANGUAGE_AUTO | LANGUAGE_ENGLISH | LANGUAGE_CHINESE => super::settings::language(id),
        FOCUS_SEARCH => {
            sidebar::show(catalog::Panel::Results, false);
            let h = read(|a| a.search);
            focus(h);
            send(h, EM_SETSEL, 0, -1);
        }
        PANEL_SEARCH => sidebar::show(catalog::Panel::Results, true),
        MANAGE_SOURCES => sidebar::show(catalog::Panel::Sources, true),
        LOAD_METADATA => sources::choose_add(),
        ADD_DIRECTORY => sources::choose_directory(),
        REFRESH_SOURCES => sources::refresh(),
        FORGET_LOCATION => sources::forget_selected(),
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
                send(read(|a| a.mdi), WM_MDIMAXIMIZE, h as usize, 0);
            }
        }
        _ => {}
    }
}
pub(super) unsafe extern "system" fn splitter_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
) -> isize {
    unsafe {
        match msg {
            WM_SETCURSOR => {
                SetCursor(LoadCursorW(ptr::null_mut(), IDC_SIZEWE));
                return 1;
            }
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                return 0;
            }
            WM_MOUSEMOVE if GetCapture() == hwnd => {
                let mut p = POINT {
                    x: l as u16 as i16 as i32,
                    y: (l >> 16) as u16 as i16 as i32,
                };
                let frame = read(|a| a.frame);
                MapWindowPoints(hwnd, frame, &mut p, 1);
                let width = (MulDiv(p.x, 96, dpi().window(frame) as i32) - sidebar::TOOLBAR_WIDTH)
                    .clamp(120, 2000);
                write(|a| a.sidebar = width);
                layout_frame();
                return 0;
            }
            WM_LBUTTONUP => {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, w, l)
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
                    a.document_tabs = tabs;
                    a.status = status;
                    a.mdi = mdi;
                });
                if !sidebar::create(hwnd) {
                    return -1;
                }
                windows_sys::Win32::UI::Shell::DragAcceptFiles(hwnd, 1);
                prepare_frame();
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
                prepare_frame();
                let r = *(l as *const RECT);
                SetWindowPos(
                    hwnd,
                    ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                layout_frame();
                for h in read(|a| a.documents.iter().map(|d| d.hwnd).collect::<Vec<_>>()) {
                    super::document::layout_document(h);
                }
                return 0;
            }
            WM_SETTINGCHANGE => {
                let old = write(|a| std::mem::take(&mut a.fonts));
                prepare_frame();
                layout_frame();
                drop(old);
            }
            WM_COMMAND => {
                let id = w as u16;
                if id == SEARCH && (w >> 16) as u32 == EN_CHANGE {
                    catalog::input_changed();
                    return 0;
                }
                if matches!(
                    id,
                    LANGUAGE_AUTO
                        | LANGUAGE_ENGLISH
                        | LANGUAGE_CHINESE
                        | FOCUS_SEARCH
                        | PANEL_SEARCH
                        | MANAGE_SOURCES
                        | LOAD_METADATA
                        | ADD_DIRECTORY
                        | REFRESH_SOURCES
                        | FORGET_LOCATION
                        | CLOSE
                        | EXIT
                        | COLUMNS
                        | ROWS
                        | FREE
                        | MAXIMIZE
                ) {
                    command(id);
                    return 0;
                }
            }
            WM_TIMER if w == catalog::SEARCH_TIMER => {
                catalog::timer();
                return 0;
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
        DefFrameProcW(hwnd, read(|a| a.mdi), msg, w, l)
    }
}

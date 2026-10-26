//! Main-window controls, commands and root DPI/layout orchestration.
use super::Architecture;
use super::document::{layout_document, prepare_document};
use super::workspace::{
    ARRANGE_DOCUMENTS, activate_document_tab, active, arrange_documents, close_document,
    document_tabs_proc, set_arrangement,
};
use super::{
    CLOSE, COLUMNS, DOCUMENT_TABS, EXIT, FREE, MAXIMIZE, NEW_X64, NEW_X86, OPEN, RESULTS, ROWS,
    SEARCH, bind, dpi, font_for, new_document, read, write,
};
use crate::resources::{self, IDS_NEW_X64, IDS_NEW_X86, IDS_OPEN, IDS_UNTITLED, IDS_WORKSPACE};
use crate::{
    layout::{self, Arrangement},
    native::*,
};
use std::ptr;

fn layout_frame() {
    let (frame, search, open, results, splitter, mdi, sidebar) = read(|a| {
        (
            a.frame, a.search, a.open, a.results, a.splitter, a.mdi, a.sidebar,
        )
    });
    if mdi.is_null() {
        return;
    }
    let value = dpi().window(frame);
    let (font, height) = font_for(value);
    let document_tabs = read(|a| a.document_tabs);
    for h in [search, open, results, document_tabs] {
        bind(h, font);
    }
    let r = layout::frame(client(frame), value, height, sidebar);
    place(search, r.edit);
    place(open, r.button);
    place(results, r.results);
    place(splitter, r.splitter);
    place(document_tabs, r.documents);
    place(mdi, r.mdi);
    send(
        results,
        LVM_SETCOLUMNWIDTH,
        0,
        (r.results.right - r.results.left - layout::scale(8, value)).max(0) as isize,
    );
    arrange_documents();
}
pub(super) fn open_search() {
    let h = read(|a| a.search);
    let title = text(h);
    let untitled = resources::text(IDS_UNTITLED);
    let title = if title.trim().is_empty() {
        &untitled
    } else {
        title.trim()
    };
    if let Err(e) = new_document(title, Architecture::X64) {
        crate::show_error(&e);
    }
}
pub(super) fn command(id: u16) {
    let mdi = read(|a| a.mdi);
    match id {
        NEW_X86 | NEW_X64 => {
            let architecture = if id == NEW_X64 {
                Architecture::X64
            } else {
                Architecture::X86
            };
            if let Err(e) = new_document(&resources::text(IDS_UNTITLED), architecture) {
                crate::show_error(&e);
            }
        }
        OPEN => open_search(),
        CLOSE => close_document(active()),
        EXIT => {
            let frame = read(|a| a.frame);
            unsafe {
                DestroyWindow(frame);
            }
        }
        COLUMNS => set_arrangement(Arrangement::Columns),
        ROWS => set_arrangement(Arrangement::Rows),
        FREE => set_arrangement(Arrangement::Free),
        MAXIMIZE => {
            let h = active();
            if !h.is_null() {
                send(mdi, WM_MDIMAXIMIZE, h as usize, 0);
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
                let mut point = POINT {
                    x: (l as u16 as i16) as i32,
                    y: ((l >> 16) as u16 as i16) as i32,
                };
                let frame = read(|a| a.frame);
                MapWindowPoints(hwnd, frame, &mut point, 1);
                let value = dpi().window(frame);
                let width = MulDiv(point.x, 96, value as i32).max(120);
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
                let search = child(hwnd, "EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32, SEARCH);
                let open = child(
                    hwnd,
                    "BUTTON",
                    &resources::text(IDS_OPEN),
                    WS_TABSTOP | BS_PUSHBUTTON as u32,
                    OPEN,
                );
                let results = child(
                    hwnd,
                    "SysListView32",
                    "",
                    WS_TABSTOP | LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL,
                    RESULTS,
                );
                let splitter = child(hwnd, "Winlex.Splitter", "", 0, 0);
                let document_tabs = child(
                    hwnd,
                    "SysTabControl32",
                    "",
                    WS_TABSTOP | TCS_TOOLTIPS,
                    DOCUMENT_TABS,
                );
                let menu = GetSubMenu(GetMenu(hwnd), 1);
                let c = CLIENTCREATESTRUCT {
                    hWindowMenu: menu,
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
                if [search, open, results, splitter, mdi, document_tabs]
                    .iter()
                    .any(|h| h.is_null())
                {
                    return -1;
                }
                if set_subclass(document_tabs, Some(document_tabs_proc), 1, 0) == 0 {
                    return -1;
                }
                write(|a| {
                    a.search = search;
                    a.open = open;
                    a.results = results;
                    a.splitter = splitter;
                    a.mdi = mdi;
                    a.document_tabs = document_tabs;
                });
                list_column(results, 0, &resources::text(IDS_WORKSPACE), 240);
                send(results, LVM_SETITEMCOUNT, 2, 0);
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
                let (search, open, results, tabs, docs) = read(|a| {
                    (
                        a.search,
                        a.open,
                        a.results,
                        a.document_tabs,
                        a.documents.clone(),
                    )
                });
                let (font, _) = font_for(dpi().window(hwnd));
                for h in [search, open, results, tabs] {
                    bind(h, font);
                }
                for doc in &docs {
                    prepare_document(doc);
                }
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
                for doc in read(|a| a.documents.clone()) {
                    layout_document(doc.hwnd);
                }
                return 0;
            }
            WM_COMMAND => {
                let id = w as u16;
                if matches!(
                    id,
                    NEW_X86 | NEW_X64 | CLOSE | EXIT | COLUMNS | ROWS | FREE | MAXIMIZE | OPEN
                ) {
                    command(id);
                    return 0;
                }
            }
            WM_NOTIFY => {
                let hdr = &*(l as *const NMHDR);
                if hdr.idFrom == DOCUMENT_TABS as usize && hdr.code == TCN_SELCHANGE {
                    activate_document_tab();
                    return 0;
                }
                if hdr.idFrom == RESULTS as usize {
                    if hdr.code == LVN_GETDISPINFOW {
                        let info = &mut *(l as *mut NMLVDISPINFOW);
                        if info.item.mask & LVIF_TEXT != 0
                            && !info.item.pszText.is_null()
                            && info.item.cchTextMax > 0
                        {
                            let s = wide(&resources::text(if info.item.iItem == 0 {
                                IDS_NEW_X86
                            } else {
                                IDS_NEW_X64
                            }));
                            let n = (info.item.cchTextMax as usize - 1).min(s.len() - 1);
                            ptr::copy_nonoverlapping(s.as_ptr(), info.item.pszText, n);
                            *info.item.pszText.add(n) = 0;
                        }
                        return 0;
                    }
                    if hdr.code == NM_DBLCLK {
                        let selected = send(
                            hdr.hwndFrom,
                            LVM_GETNEXTITEM,
                            usize::MAX,
                            LVNI_SELECTED as isize,
                        );
                        if selected >= 0 {
                            command(if selected == 0 { NEW_X86 } else { NEW_X64 });
                        }
                        return 0;
                    }
                }
            }
            WM_SIZING => {
                let value = dpi().window(hwnd);
                let r = &mut *(l as *mut RECT);
                let min_w = layout::scale(480, value);
                let min_h = layout::scale(300, value);
                if r.right - r.left < min_w {
                    if matches!(w as u32, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
                        r.left = r.right - min_w;
                    } else {
                        r.right = r.left + min_w;
                    }
                }
                if r.bottom - r.top < min_h {
                    if matches!(w as u32, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
                        r.top = r.bottom - min_h;
                    } else {
                        r.bottom = r.top + min_h;
                    }
                }
                return 1;
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                return 0;
            }
            ARRANGE_DOCUMENTS => {
                arrange_documents();
                return 0;
            }
            _ => {}
        }
        DefFrameProcW(hwnd, read(|a| a.mdi), msg, w, l)
    }
}

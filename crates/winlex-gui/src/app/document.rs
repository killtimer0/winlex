//! Document-owned controls, content, page selection and focus.
use super::workspace::{
    arrange_documents, capture_placements, refresh_document_tabs, sync_document_selection,
    tiled_bounds,
};
use super::{
    ARCH, DECLARATION, Document, TABLE, TABS, bind, document, dpi, edit_document, font_for, read,
    workspace,
};
use super::{ARCHITECTURES, Architecture};
use crate::resources::{
    self, IDS_ARCHITECTURE, IDS_CATALOG, IDS_DECLARATION, IDS_LAYOUT, IDS_NOT_LOADED,
    IDS_PLACEHOLDER, IDS_PROPERTY, IDS_REFERENCES, IDS_VALUE,
};
use crate::{
    layout::{self, Placement},
    native::*,
};

pub(super) fn prepare_document(doc: &Document) {
    let (font, _) = font_for(dpi().window(doc.hwnd));
    for hwnd in [doc.arch, doc.tabs, doc.declaration, doc.table] {
        bind(hwnd, font);
    }
}
pub(super) fn layout_document(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let value = dpi().window(hwnd);
    prepare_document(&doc);
    let (_, font_height) = font_for(value);
    let px = |n| layout::scale(n, value);
    let r = client(hwnd);
    let margin = px(8);
    let row = font_height + px(12);
    // Combo box window height includes its drop-down list.
    place(
        doc.arch,
        layout::rect(margin, margin, px(110), row + px(100)),
    );
    place(
        doc.tabs,
        layout::rect(
            margin,
            margin + row + px(6),
            r.right - margin * 2,
            r.bottom - margin * 2 - row - px(6),
        ),
    );
    let mut page = client(doc.tabs);
    send(doc.tabs, TCM_ADJUSTRECT, 0, &mut page as *mut _ as isize);
    unsafe {
        MapWindowPoints(doc.tabs, hwnd, (&mut page as *mut RECT).cast(), 2);
    }
    place(doc.declaration, page);
    place(doc.table, page);
    for (index, width) in doc.column_widths.iter().enumerate() {
        let width = px(*width);
        if send(doc.table, LVM_GETCOLUMNWIDTH, index, 0) != width as isize {
            send(doc.table, LVM_SETCOLUMNWIDTH, index, width as isize);
        }
    }
}
pub(super) fn restore_focus(hwnd: HWND) {
    if let Some(doc) = document(hwnd) {
        let h = doc.last_focus;
        let valid = !h.is_null()
            && unsafe {
                IsWindow(h) != 0
                    && IsChild(hwnd, h) != 0
                    && IsWindowVisible(h) != 0
                    && IsWindowEnabled(h) != 0
            };
        focus(if valid { h } else { doc.arch });
    }
}
pub(super) fn remember_focus() {
    let h = unsafe { GetFocus() };
    let docs = read(|a| a.documents.iter().map(|d| d.hwnd).collect::<Vec<_>>());
    if let Some(owner) = docs.into_iter().find(|&d| unsafe { IsChild(d, h) } != 0) {
        edit_document(owner, |d| d.last_focus = h);
    }
}
pub(super) fn refresh_title(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    set_text(
        hwnd,
        &format!("{} — {}", doc.title, doc.architecture_name()),
    );
    refresh_document_tabs();
}
pub(super) fn refresh_content(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let arch = doc.architecture_name();
    set_text(
        doc.declaration,
        &resources::text(IDS_PLACEHOLDER)
            .replace("{arch}", &arch)
            .replace("{title}", &doc.title),
    );
    send(doc.table, LVM_DELETEALLITEMS, 0, 0);
    list_row(doc.table, 0, &[&resources::text(IDS_ARCHITECTURE), &arch]);
    list_row(
        doc.table,
        1,
        &[
            &resources::text(IDS_CATALOG),
            &resources::text(IDS_NOT_LOADED),
        ],
    );
}
pub(super) fn select_page(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let page = send(doc.tabs, TCM_GETCURSEL, 0, 0);
    show(doc.declaration, page == 0);
    show(doc.table, page != 0);
}

pub(super) unsafe extern "system" fn document_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
) -> isize {
    unsafe {
        match msg {
            WM_CLOSE => {
                workspace::close_document(hwnd);
                return 0;
            }
            WM_CREATE => {
                let create = &*(l as *const CREATESTRUCTW);
                let mdi_create = &*(create.lpCreateParams as *const MDICREATESTRUCTW);
                let Some(architecture) = Architecture::from_index(mdi_create.lParam as usize)
                else {
                    return -1;
                };
                let arch = child(
                    hwnd,
                    "COMBOBOX",
                    "",
                    WS_TABSTOP | CBS_DROPDOWNLIST as u32 | WS_VSCROLL,
                    ARCH,
                );
                let tabs = child(hwnd, "SysTabControl32", "", WS_TABSTOP, TABS);
                let declaration = child(
                    hwnd,
                    "EDIT",
                    "",
                    WS_TABSTOP
                        | WS_VSCROLL
                        | ES_MULTILINE as u32
                        | ES_READONLY as u32
                        | ES_AUTOVSCROLL as u32,
                    DECLARATION,
                );
                let table = child(
                    hwnd,
                    "SysListView32",
                    "",
                    WS_TABSTOP | LVS_REPORT | LVS_SINGLESEL,
                    TABLE,
                );
                if [arch, tabs, declaration, table].iter().any(|h| h.is_null()) {
                    return -1;
                }
                if set_subclass(table, Some(table_proc), 1, hwnd as usize) == 0 {
                    return -1;
                }
                for (_, id) in ARCHITECTURES {
                    send(
                        arch,
                        CB_ADDSTRING,
                        0,
                        wide(&resources::text(id)).as_ptr() as isize,
                    );
                }
                send(arch, CB_SETCURSEL, architecture.index(), 0);
                for (index, id) in [IDS_DECLARATION, IDS_LAYOUT, IDS_REFERENCES]
                    .iter()
                    .enumerate()
                {
                    let mut s = wide(&resources::text(*id));
                    let item = TCITEMW {
                        mask: TCIF_TEXT,
                        pszText: s.as_mut_ptr(),
                        ..std::mem::zeroed()
                    };
                    send(tabs, TCM_INSERTITEMW, index, &item as *const _ as isize);
                }
                list_column(table, 0, &resources::text(IDS_PROPERTY), 140);
                list_column(table, 1, &resources::text(IDS_VALUE), 240);
                let mdi = read(|a| a.mdi);
                let placement = Placement {
                    bounds: bounds(hwnd, mdi),
                    dpi: dpi().window(hwnd),
                };
                workspace::register_document(Document {
                    hwnd,
                    tabs,
                    arch,
                    declaration,
                    table,
                    last_focus: arch,
                    placement,
                    title: String::new(),
                    architecture,
                    column_widths: [140, 240],
                });
                return 0;
            }
            WM_SIZE => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                if w != SIZE_MINIMIZED as usize {
                    layout_document(hwnd);
                }
                return result;
            }
            WM_WINDOWPOSCHANGING => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                let r = if IsZoomed(hwnd) != 0 {
                    let mdi = read(|a| a.mdi);
                    Some(dpi().child_outer(hwnd, client(mdi)))
                } else if IsIconic(hwnd) == 0 {
                    tiled_bounds(hwnd)
                } else {
                    None
                };
                if let Some(r) = r {
                    let p = &mut *(l as *mut WINDOWPOS);
                    if p.flags & SWP_NOMOVE == 0 {
                        p.x = r.left;
                        p.y = r.top;
                    }
                    if p.flags & SWP_NOSIZE == 0 {
                        p.cx = r.right - r.left;
                        p.cy = r.bottom - r.top;
                    }
                }
                return result;
            }
            WM_SYSCOMMAND if matches!((w & 0xfff0) as u32, SC_RESTORE | SC_MINIMIZE) => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                arrange_documents();
                return result;
            }
            WM_ENTERSIZEMOVE => workspace::begin_free_placement(),
            WM_EXITSIZEMOVE => capture_placements(),
            WM_SETFOCUS => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                restore_focus(hwnd);
                return result;
            }
            WM_MDIACTIVATE => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                sync_document_selection();
                return result;
            }
            WM_COMMAND if w as u16 == ARCH && (w >> 16) as u32 == CBN_SELCHANGE => {
                let Some(selected) =
                    Architecture::from_index(send(l as HWND, CB_GETCURSEL, 0, 0) as usize)
                else {
                    return 0;
                };
                edit_document(hwnd, |d| d.architecture = selected);
                refresh_title(hwnd);
                refresh_content(hwnd);
                return 0;
            }
            WM_NOTIFY => {
                let hdr = &*(l as *const NMHDR);
                if hdr.idFrom == TABS as usize && hdr.code == TCN_SELCHANGE {
                    select_page(hwnd);
                    return 0;
                }
            }
            WM_NCDESTROY => {
                workspace::unregister_document(hwnd);
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                workspace::document_destroyed();
                return result;
            }
            _ => {}
        }
        DefMDIChildProcW(hwnd, msg, w, l)
    }
}

unsafe extern "system" fn table_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    owner: usize,
) -> isize {
    unsafe {
        let result = default_subclass(hwnd, msg, w, l);
        if msg == WM_NOTIFY {
            let hdr = &*(l as *const NMHDR);
            if hdr.code == HDN_ENDTRACKW || hdr.code == HDN_ENDTRACKA {
                let value = dpi().window(owner as HWND);
                let widths = [
                    send(hwnd, LVM_GETCOLUMNWIDTH, 0, 0) as i32,
                    send(hwnd, LVM_GETCOLUMNWIDTH, 1, 0) as i32,
                ]
                .map(|v| MulDiv(v, 96, value as i32));
                edit_document(owner as HWND, |d| d.column_widths = widths);
            }
        }
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(table_proc), 1);
        }
        result
    }
}

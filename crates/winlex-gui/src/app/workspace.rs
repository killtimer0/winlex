//! MDI lifecycle, activation projection and persistent arrangement.
use super::{Architecture, Document, document, dpi, edit_document, read, write};
use crate::{
    layout::{self, Arrangement, Placement},
    native::*,
};

pub(super) const ARRANGE_DOCUMENTS: u32 = WM_APP + 1;

pub(super) fn active() -> HWND {
    let mdi = read(|a| a.mdi);
    send(mdi, WM_MDIGETACTIVE, 0, 0) as HWND
}
pub(super) fn register_document(doc: Document) {
    write(|a| a.documents.push(doc));
}
pub(super) fn unregister_document(hwnd: HWND) {
    write(|a| a.documents.retain(|d| d.hwnd != hwnd));
}
pub(super) fn document_destroyed() {
    refresh_document_tabs();
    let frame = read(|a| a.frame);
    // Arrange after the native destruction sequence has completed.
    unsafe {
        PostMessageW(frame, ARRANGE_DOCUMENTS, 0, 0);
    }
}
pub(super) fn close_document(hwnd: HWND) {
    let mdi = read(|a| a.mdi);
    if document(hwnd).is_some() {
        send(mdi, WM_MDIDESTROY, hwnd as usize, 0);
    }
}
pub(super) fn sync_document_selection() {
    let current = active();
    let (tabs, selected) = read(|a| {
        (
            a.document_tabs,
            a.documents.iter().position(|d| d.hwnd == current),
        )
    });
    send(tabs, TCM_SETCURSEL, selected.unwrap_or(usize::MAX), 0);
}
pub(super) unsafe extern "system" fn document_tabs_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        if msg == WM_MBUTTONUP {
            let mut hit = TCHITTESTINFO {
                pt: POINT {
                    x: l as u16 as i16 as i32,
                    y: (l >> 16) as u16 as i16 as i32,
                },
                flags: 0,
            };
            let index = send(hwnd, TCM_HITTEST, 0, &mut hit as *mut _ as isize);
            let target = read(|a| a.documents.get(index as usize).map(|d| d.hwnd));
            if let Some(target) = target {
                close_document(target);
            }
            return 0;
        }
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(document_tabs_proc), 1);
        }
        default_subclass(hwnd, msg, w, l)
    }
}
pub(super) fn refresh_document_tabs() {
    let (tabs, docs) = read(|a| (a.document_tabs, a.documents.clone()));
    let count = send(tabs, TCM_GETITEMCOUNT, 0, 0).max(0) as usize;
    for index in (docs.len()..count).rev() {
        send(tabs, TCM_DELETEITEM, index, 0);
    }
    for (index, doc) in docs.iter().enumerate() {
        let arch = doc.architecture_name();
        let ordinal = docs[..index]
            .iter()
            .filter(|d| d.title == doc.title && d.architecture == doc.architecture)
            .count()
            + 1;
        let suffix = if ordinal > 1 {
            format!(" ({ordinal})")
        } else {
            String::new()
        };
        let mut label = wide(&format!("{} — {arch}{suffix}", doc.title));
        let item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: label.as_mut_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        send(
            tabs,
            if index < count {
                TCM_SETITEMW
            } else {
                TCM_INSERTITEMW
            },
            index,
            &item as *const _ as isize,
        );
    }
    sync_document_selection();
}
pub(super) fn activate_document_tab() {
    let (tabs, mdi) = read(|a| (a.document_tabs, a.mdi));
    let index = send(tabs, TCM_GETCURSEL, 0, 0);
    let target = read(|a| a.documents.get(index as usize).map(|d| d.hwnd));
    if let Some(hwnd) = target {
        if unsafe { IsIconic(hwnd) } != 0 {
            send(mdi, WM_MDIRESTORE, hwnd as usize, 0);
        }
        send(mdi, WM_MDIACTIVATE, hwnd as usize, 0);
    }
    sync_document_selection();
}
pub(super) fn arrange_documents() {
    let (mdi, arrangement, docs) = read(|a| (a.mdi, a.arrangement, a.documents.clone()));
    if mdi.is_null() {
        return;
    }
    let current = active();
    if !current.is_null() && unsafe { IsZoomed(current) } != 0 {
        return;
    }
    let visible: Vec<_> = docs
        .iter()
        .filter(|d| unsafe { IsIconic(d.hwnd) } == 0)
        .collect();
    let area = client(mdi);
    for (index, doc) in visible.iter().enumerate() {
        let r = match arrangement {
            Arrangement::Free => doc.placement.at(dpi().window(doc.hwnd)),
            _ => layout::tile(area, arrangement, index, visible.len()),
        };
        place(doc.hwnd, r);
    }
}
pub(super) fn tiled_bounds(hwnd: HWND) -> Option<RECT> {
    let (mdi, arrangement, docs) = read(|a| (a.mdi, a.arrangement, a.documents.clone()));
    if arrangement == Arrangement::Free {
        return None;
    }
    let docs: Vec<_> = docs
        .iter()
        .filter(|d| unsafe { IsIconic(d.hwnd) } == 0)
        .collect();
    let index = docs.iter().position(|d| d.hwnd == hwnd)?;
    Some(layout::tile(client(mdi), arrangement, index, docs.len()))
}
pub(super) fn capture_placements() {
    let (mdi, docs) = read(|a| (a.mdi, a.documents.clone()));
    for doc in docs {
        if unsafe { IsZoomed(doc.hwnd) != 0 || IsIconic(doc.hwnd) != 0 } {
            continue;
        }
        let p = Placement {
            bounds: bounds(doc.hwnd, mdi),
            dpi: dpi().window(doc.hwnd),
        };
        edit_document(doc.hwnd, |d| d.placement = p);
    }
}
pub(super) fn begin_free_placement() {
    capture_placements();
    write(|a| a.arrangement = Arrangement::Free);
}
pub(super) fn set_arrangement(arrangement: Arrangement) {
    let mdi = read(|a| a.mdi);
    let current = active();
    if !current.is_null() && unsafe { IsZoomed(current) } != 0 {
        send(mdi, WM_MDIRESTORE, current as usize, 0);
    }
    if arrangement == Arrangement::Free {
        capture_placements();
    }
    write(|a| a.arrangement = arrangement);
    arrange_documents();
    if arrangement != Arrangement::Free {
        capture_placements();
    }
}
pub(super) fn new_document(title: &str, architecture: Architecture) -> Result<HWND, String> {
    let (mdi, count) = read(|a| (a.mdi, a.documents.len()));
    let value = dpi().window(mdi);
    let hwnd = unsafe {
        CreateMDIWindowW(
            wide("Winlex.Document").as_ptr(),
            wide(title).as_ptr(),
            WS_VISIBLE | WS_OVERLAPPEDWINDOW,
            layout::scale(16 + (count as i32 % 6) * 24, value),
            layout::scale(16 + (count as i32 % 6) * 24, value),
            layout::scale(600, value),
            layout::scale(400, value),
            mdi,
            instance(),
            architecture.index() as isize,
        )
    };
    if hwnd.is_null() {
        return Err(error("Create MDI document"));
    }
    edit_document(hwnd, |d| d.title = title.to_owned());
    document::refresh_title(hwnd);
    document::refresh_content(hwnd);
    document::select_page(hwnd);
    document::layout_document(hwnd);
    if read(|a| a.arrangement) != Arrangement::Free {
        arrange_documents();
    }
    Ok(hwnd)
}

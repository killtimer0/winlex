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
pub(super) fn activate_document(hwnd: HWND) {
    let mdi = read(|a| a.mdi);
    if unsafe { IsIconic(hwnd) } != 0 {
        send(mdi, WM_MDIRESTORE, hwnd as usize, 0);
    }
    send(mdi, WM_MDIACTIVATE, hwnd as usize, 0);
    document::restore_focus(hwnd);
}
pub(super) fn register_document(doc: Document) {
    write(|a| a.documents.push(doc));
}
pub(super) fn unregister_document(hwnd: HWND) {
    if let Some(doc) = document(hwnd) {
        doc.query.cancel();
    }
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
    let (tabs, docs) = read(|a| {
        (
            a.document_tabs,
            a.documents
                .iter()
                .map(|d| (d.title.clone(), d.architecture, d.query.is_source()))
                .collect::<Vec<_>>(),
        )
    });
    let count = send(tabs, TCM_GETITEMCOUNT, 0, 0).max(0) as usize;
    for index in (docs.len()..count).rev() {
        send(tabs, TCM_DELETEITEM, index, 0);
    }
    let mut ordinals = std::collections::HashMap::new();
    for (index, (title, architecture, source)) in docs.iter().enumerate() {
        let arch = architecture.label();
        let ordinal = ordinals.entry((title, architecture.index())).or_insert(0);
        *ordinal += 1;
        let suffix = if *ordinal > 1 {
            format!(" ({ordinal})")
        } else {
            String::new()
        };
        let mut label = wide(&if *source {
            title.clone()
        } else {
            format!("{title} — {arch}{suffix}")
        });
        if index < count {
            let mut old = vec![0u16; label.len() + 1];
            let mut item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: old.as_mut_ptr(),
                cchTextMax: old.len() as i32,
                ..unsafe { std::mem::zeroed() }
            };
            if send(tabs, TCM_GETITEMW, index, &mut item as *mut _ as isize) != 0
                && old[..old.iter().position(|&c| c == 0).unwrap_or(old.len())]
                    == label[..label.len() - 1]
            {
                continue;
            }
        }
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
    let tabs = read(|a| a.document_tabs);
    let index = send(tabs, TCM_GETCURSEL, 0, 0);
    let target = read(|a| a.documents.get(index as usize).map(|d| d.hwnd));
    if let Some(hwnd) = target {
        activate_document(hwnd);
    }
    sync_document_selection();
}
pub(super) fn arrange_documents() {
    let (mdi, arrangement, docs) = read(|a| {
        (
            a.mdi,
            a.arrangement,
            a.documents
                .iter()
                .map(|d| (d.hwnd, d.placement))
                .collect::<Vec<_>>(),
        )
    });
    if mdi.is_null() {
        return;
    }
    let current = active();
    if !current.is_null() && unsafe { IsZoomed(current) } != 0 {
        return;
    }
    let visible: Vec<_> = docs
        .iter()
        .filter(|d| unsafe { IsIconic(d.0) } == 0)
        .collect();
    let area = client(mdi);
    for (index, doc) in visible.iter().enumerate() {
        let r = match arrangement {
            Arrangement::Free => doc.1.at(dpi().window(doc.0)),
            _ => layout::tile(area, arrangement, index, visible.len()),
        };
        place(doc.0, r);
    }
}
pub(super) fn tiled_bounds(hwnd: HWND) -> Option<RECT> {
    let (mdi, arrangement) = read(|a| (a.mdi, a.arrangement));
    if arrangement == Arrangement::Free {
        return None;
    }
    let docs: Vec<_> = read(|a| a.documents.iter().map(|d| d.hwnd).collect::<Vec<_>>())
        .into_iter()
        .filter(|&h| unsafe { IsIconic(h) } == 0)
        .collect();
    let index = docs.iter().position(|&h| h == hwnd)?;
    Some(layout::tile(client(mdi), arrangement, index, docs.len()))
}
pub(super) fn capture_placements() {
    let (mdi, docs) = read(|a| {
        (
            a.mdi,
            a.documents.iter().map(|d| d.hwnd).collect::<Vec<_>>(),
        )
    });
    for hwnd in docs {
        if unsafe { IsZoomed(hwnd) != 0 || IsIconic(hwnd) != 0 } {
            continue;
        }
        let p = Placement {
            bounds: bounds(hwnd, mdi),
            dpi: dpi().window(hwnd),
        };
        edit_document(hwnd, |d| d.placement = p);
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
pub(super) struct DocumentInit<'a> {
    pub title: &'a str,
    pub architecture: Architecture,
    pub binding: super::catalog::Binding,
}
pub(super) fn create_document(
    title: &str,
    architecture: Architecture,
    binding: super::catalog::Binding,
) -> Result<HWND, String> {
    let (mdi, count) = read(|a| (a.mdi, a.documents.len()));
    let value = dpi().window(mdi);
    // CreateMDIWindowW consumes this pointer synchronously in WM_CREATE.
    let init = DocumentInit {
        title,
        architecture,
        binding,
    };
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
            &init as *const DocumentInit<'_> as isize,
        )
    };
    if hwnd.is_null() {
        return Err(error("Create MDI document"));
    }
    if let Err(error) = document::create_actions(hwnd) {
        close_document(hwnd);
        return Err(error);
    }
    document::refresh_title(hwnd);
    super::catalog::request_document(hwnd);
    if let Some(doc) = document(hwnd) {
        document::prepare_document(&doc);
    }
    document::layout_document(hwnd);
    if read(|a| a.arrangement) != Arrangement::Free {
        arrange_documents();
    }
    Ok(hwnd)
}

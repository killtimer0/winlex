//! MDI lifecycle, activation projection and persistent arrangement.
use super::{
    document::{self, Content},
    dpi, read, write,
};
use crate::{
    layout::{self, Arrangement, Placement},
    native::*,
};

pub(super) const ARRANGE_DOCUMENTS: u32 = WM_APP + 1;

pub(super) struct Workspace {
    pub mdi: HWND,
    pub tabs: HWND,
    pub documents: Vec<Document>,
    arrangement: Arrangement,
    serial: u64,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            mdi: std::ptr::null_mut(),
            tabs: std::ptr::null_mut(),
            documents: vec![],
            arrangement: Arrangement::Free,
            serial: 0,
        }
    }
}
pub(super) struct Document {
    pub id: u64,
    pub hwnd: HWND,
    pub title: String,
    pub content: Content,
    pub last_focus: HWND,
    pub placement: Placement,
}
pub(super) fn document<T>(hwnd: HWND, inspect: impl FnOnce(&Document) -> T) -> Option<T> {
    read(|a| {
        a.workspace
            .documents
            .iter()
            .find(|d| d.hwnd == hwnd)
            .map(inspect)
    })
}
pub(super) fn edit_document(hwnd: HWND, edit: impl FnOnce(&mut Document)) {
    write(|a| {
        if let Some(doc) = a.workspace.documents.iter_mut().find(|d| d.hwnd == hwnd) {
            edit(doc);
        }
    });
}

pub(super) fn active() -> HWND {
    let mdi = read(|a| a.workspace.mdi);
    send(mdi, WM_MDIGETACTIVE, 0, 0) as HWND
}
pub(super) fn activate_document(hwnd: HWND) {
    let mdi = read(|a| a.workspace.mdi);
    if unsafe { IsIconic(hwnd) } != 0 {
        send(mdi, WM_MDIRESTORE, hwnd as usize, 0);
    }
    send(mdi, WM_MDIACTIVATE, hwnd as usize, 0);
    document::restore_focus(hwnd);
}
pub(super) fn register_document(doc: Document) {
    write(|a| a.workspace.documents.push(doc));
}
pub(super) fn unregister_document(hwnd: HWND) {
    let removed = write(|a| {
        let docs = &mut a.workspace.documents;
        docs.iter()
            .position(|d| d.hwnd == hwnd)
            .map(|index| docs.remove(index))
    });
    if let Some(doc) = removed {
        doc.content.cancel();
    }
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
    let mdi = read(|a| a.workspace.mdi);
    if document(hwnd, |_| ()).is_some() {
        send(mdi, WM_MDIDESTROY, hwnd as usize, 0);
    }
}
pub(super) fn sync_document_selection() {
    let current = active();
    let (tabs, selected) = read(|a| {
        (
            a.workspace.tabs,
            a.workspace.documents.iter().position(|d| d.hwnd == current),
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
            let target = read(|a| a.workspace.documents.get(index as usize).map(|d| d.hwnd));
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
            a.workspace.tabs,
            a.workspace
                .documents
                .iter()
                .map(|d| d.title.clone())
                .collect::<Vec<_>>(),
        )
    });
    let count = send(tabs, TCM_GETITEMCOUNT, 0, 0).max(0) as usize;
    for index in (docs.len()..count).rev() {
        send(tabs, TCM_DELETEITEM, index, 0);
    }
    let mut ordinals = std::collections::HashMap::new();
    for (index, title) in docs.iter().enumerate() {
        let ordinal = ordinals.entry(title).or_insert(0);
        *ordinal += 1;
        let suffix = if *ordinal > 1 {
            format!(" ({ordinal})")
        } else {
            String::new()
        };
        let mut label = wide(&format!("{title}{suffix}"));
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
    let tabs = read(|a| a.workspace.tabs);
    let index = send(tabs, TCM_GETCURSEL, 0, 0);
    let target = read(|a| a.workspace.documents.get(index as usize).map(|d| d.hwnd));
    if let Some(hwnd) = target {
        activate_document(hwnd);
    }
    sync_document_selection();
}
pub(super) fn arrange_documents() {
    let (mdi, arrangement, docs) = read(|a| {
        (
            a.workspace.mdi,
            a.workspace.arrangement,
            a.workspace
                .documents
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
    let (mdi, arrangement) = read(|a| (a.workspace.mdi, a.workspace.arrangement));
    if arrangement == Arrangement::Free {
        return None;
    }
    let docs: Vec<_> = read(|a| {
        a.workspace
            .documents
            .iter()
            .map(|d| d.hwnd)
            .collect::<Vec<_>>()
    })
    .into_iter()
    .filter(|&h| unsafe { IsIconic(h) } == 0)
    .collect();
    let index = docs.iter().position(|&h| h == hwnd)?;
    Some(layout::tile(client(mdi), arrangement, index, docs.len()))
}
pub(super) fn capture_placements() {
    let (mdi, docs) = read(|a| {
        (
            a.workspace.mdi,
            a.workspace
                .documents
                .iter()
                .map(|d| d.hwnd)
                .collect::<Vec<_>>(),
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
    write(|a| a.workspace.arrangement = Arrangement::Free);
}
pub(super) fn set_arrangement(arrangement: Arrangement) {
    let mdi = read(|a| a.workspace.mdi);
    let current = active();
    if !current.is_null() && unsafe { IsZoomed(current) } != 0 {
        send(mdi, WM_MDIRESTORE, current as usize, 0);
    }
    if arrangement == Arrangement::Free {
        capture_placements();
    }
    write(|a| a.workspace.arrangement = arrangement);
    arrange_documents();
    if arrangement != Arrangement::Free {
        capture_placements();
    }
}
pub(super) fn next_id() -> u64 {
    write(|a| {
        a.workspace.serial += 1;
        a.workspace.serial
    })
}
pub(super) fn create_document(title: &str, content: Content) -> Result<HWND, String> {
    let (mdi, count) = read(|a| (a.workspace.mdi, a.workspace.documents.len()));
    let value = dpi().window(mdi);
    let mut init = document::Init {
        content: Some(content),
        error: None,
    };
    // CreateMDIWindowW consumes this pointer synchronously in WM_CREATE.
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
            &mut init as *mut document::Init as isize,
        )
    };
    if hwnd.is_null() {
        return Err(init.error.unwrap_or_else(|| error("Create MDI document")));
    }
    refresh_document_tabs();
    if let Some(content) = document(hwnd, |d| d.content.clone()) {
        content.prepare(hwnd);
    }
    if read(|a| a.workspace.arrangement) != Arrangement::Free {
        arrange_documents();
    }
    Ok(hwnd)
}

//! MDI lifecycle, activation projection and one-shot arrangement commands.
use super::{document as host, dpi, read, write};
use crate::{
    layout::{Arrangement, Placement},
    native::*,
    resources::{self, IDS_PREVIEW, IDS_TAB_CLOSE, IDS_TAB_PIN},
    tabs,
};

pub(super) struct Workspace {
    pub mdi: HWND,
    pub tabs: HWND,
    pub documents: Vec<DocumentHost>,
    pub icons: crate::icons::DocumentIcons,
    maximized: bool,
    serial: u64,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            mdi: std::ptr::null_mut(),
            tabs: std::ptr::null_mut(),
            documents: vec![],
            icons: crate::icons::DocumentIcons::default(),
            maximized: true,
            serial: 0,
        }
    }
}
pub(super) struct DocumentHost {
    pub id: u64,
    pub hwnd: HWND,
    pub content: std::rc::Rc<super::metadata::MetadataView>,
    pub placement: Option<Placement>,
    /// A transient document that the next preview request may replace in place.
    pub preview: bool,
}
pub(super) fn document<T>(hwnd: HWND, inspect: impl FnOnce(&DocumentHost) -> T) -> Option<T> {
    read(|a| {
        a.workspace
            .documents
            .iter()
            .find(|d| d.hwnd == hwnd)
            .map(inspect)
    })
}
pub(super) fn edit_document(hwnd: HWND, edit: impl FnOnce(&mut DocumentHost)) {
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
pub(super) fn show_document(hwnd: HWND) {
    let mdi = read(|a| a.workspace.mdi);
    if unsafe { IsIconic(hwnd) } != 0 {
        send(mdi, WM_MDIRESTORE, hwnd as usize, 0);
    }
    send(mdi, WM_MDIACTIVATE, hwnd as usize, 0);
}
/// Opening from the sidebar shows the document without taking keyboard focus from it.
pub(super) fn keep_focus(open: impl FnOnce()) {
    let (frame, mdi) = read(|a| (a.frame, a.workspace.mdi));
    let previous = unsafe { GetFocus() };
    let sidebar = !previous.is_null()
        && unsafe { IsChild(frame, previous) != 0 && IsChild(mdi, previous) == 0 };
    open();
    // Win32 gives a newly active MDI child the focus even when the request came
    // from beside the MDI client; hand it back.
    if sidebar && unsafe { IsWindow(previous) != 0 } {
        focus(previous);
    }
}
pub(super) fn register_document(doc: DocumentHost) {
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
pub(super) fn close_document(hwnd: HWND) {
    let mdi = read(|a| a.workspace.mdi);
    if document(hwnd, |_| ()).is_some() {
        let tabs = read(|a| a.workspace.tabs);
        let previous = unsafe { GetFocus() };
        let tab_focus = previous == tabs;
        let next = if active() == hwnd {
            read(|a| {
                let docs = &a.workspace.documents;
                let index = docs.iter().position(|d| d.hwnd == hwnd)?;
                docs.get(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|i| docs.get(i)))
                    .map(|d| d.hwnd)
            })
        } else {
            None
        };
        send(mdi, WM_MDIDESTROY, hwnd as usize, 0);
        if let Some(next) = next {
            keep_focus(|| show_document(next));
        }
        if tab_focus {
            if read(|a| a.workspace.documents.is_empty()) {
                super::sidebar::focus();
            } else {
                focus(tabs);
            }
        } else if read(|a| a.workspace.documents.is_empty())
            && (previous == hwnd || unsafe { IsWindow(previous) } == 0)
        {
            super::sidebar::focus();
        }
    }
}
pub(super) fn preview_document() -> Option<HWND> {
    read(|a| {
        a.workspace
            .documents
            .iter()
            .find(|d| d.preview)
            .map(|d| d.hwnd)
    })
}
pub(super) fn pin_document(hwnd: HWND) {
    let changed = write(|a| {
        let Some(doc) = a.workspace.documents.iter_mut().find(|d| d.hwnd == hwnd) else {
            return false;
        };
        let was_preview = doc.preview;
        doc.preview = false;
        was_preview
    });
    if changed {
        refresh_document_tabs();
    }
}
pub(super) fn sync_document_selection() {
    let current = active();
    let (tabs, selected) = read(|a| {
        (
            a.workspace.tabs,
            a.workspace
                .documents
                .iter()
                .find(|d| d.hwnd == current)
                .map(|d| d.id),
        )
    });
    tabs::select(tabs, selected);
}
pub(super) fn refresh_document_tabs() {
    let (tabs, docs) = read(|a| {
        (
            a.workspace.tabs,
            a.workspace
                .documents
                .iter()
                .map(|d| (d.id, d.hwnd, d.content.clone(), d.preview))
                .collect::<Vec<_>>(),
        )
    });
    let mut ordinals = std::collections::HashMap::new();
    let items = docs
        .iter()
        .map(|(id, hwnd, content, preview)| {
            let title = content.title();
            let ordinal = ordinals.entry(title.clone()).or_insert(0);
            *ordinal += 1;
            let suffix = if *ordinal > 1 {
                format!(" ({ordinal})")
            } else {
                String::new()
            };
            let title = format!("{title}{suffix}");
            let mut description = super::metadata::binding(*hwnd)
                .map(|b| {
                    format!(
                        "{} · {:08X}",
                        b.catalog.sources()[b.id.source_id].path.display(),
                        b.id.token
                    )
                })
                .unwrap_or_default();
            if *preview {
                description = format!("{} · {description}", resources::text(IDS_PREVIEW));
            }
            tabs::Item {
                id: *id,
                italic: *preview,
                title,
                description,
                icon: send(*hwnd, WM_GETICON, ICON_SMALL as usize, 0) as HICON,
            }
        })
        .collect();
    tabs::set_items(tabs, items);
    sync_document_selection();
}
pub(super) fn tab_notification(n: &tabs::Notification) {
    let target = read(|a| {
        a.workspace
            .documents
            .iter()
            .find(|d| d.id == n.id)
            .map(|d| d.hwnd)
    });
    let Some(hwnd) = target else { return };
    match n.header.code {
        tabs::SELECT => keep_focus(|| show_document(hwnd)),
        tabs::CLOSE => close_document(hwnd),
        tabs::ACTIVATE => pin_document(hwnd),
        tabs::REORDER => {
            write(|a| {
                let docs = &mut a.workspace.documents;
                if let Some(index) = docs.iter().position(|d| d.id == n.id) {
                    let doc = docs.remove(index);
                    docs.insert(n.index.min(docs.len()), doc);
                }
            });
            refresh_document_tabs();
        }
        tabs::CONTEXT => unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            AppendMenuW(
                menu,
                MF_STRING,
                1,
                resources::wide_text(IDS_TAB_CLOSE).as_ptr(),
            );
            if document(hwnd, |d| d.preview) == Some(true) {
                AppendMenuW(
                    menu,
                    MF_STRING,
                    2,
                    resources::wide_text(IDS_TAB_PIN).as_ptr(),
                );
            }
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                n.point.x,
                n.point.y,
                0,
                read(|a| a.frame),
                std::ptr::null(),
            );
            DestroyMenu(menu);
            match command {
                1 => close_document(hwnd),
                2 => pin_document(hwnd),
                _ => {}
            }
        },
        _ => {}
    }
}
pub(super) fn scale_documents() {
    let docs = read(|a| {
        a.workspace
            .documents
            .iter()
            .filter_map(|d| d.placement.map(|p| (d.hwnd, p)))
            .collect::<Vec<_>>()
    });
    for (hwnd, placement) in docs {
        if unsafe { IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 } {
            place(hwnd, placement.at(dpi().window(hwnd)));
        }
    }
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
        edit_document(hwnd, |d| d.placement = Some(p));
    }
}
pub(super) fn set_arrangement(arrangement: Arrangement) {
    set_maximized(false);
    let mdi = read(|a| a.workspace.mdi);
    let current = active();
    if !current.is_null() && unsafe { IsZoomed(current) } != 0 {
        send(mdi, WM_MDIRESTORE, current as usize, 0);
    }
    if arrangement != Arrangement::Free {
        send(
            mdi,
            WM_MDITILE,
            if arrangement == Arrangement::Columns {
                MDITILE_VERTICAL as usize
            } else {
                MDITILE_HORIZONTAL as usize
            },
            0,
        );
    }
    capture_placements();
}
pub(super) fn set_maximized(maximized: bool) {
    write(|a| a.workspace.maximized = maximized);
}
fn normal_placement(hwnd: HWND) -> anyhow::Result<Placement> {
    let mut placement: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 {
        return Err(error("Read document placement"));
    }
    Ok(Placement {
        bounds: placement.rcNormalPosition,
        dpi: dpi().window(hwnd),
    })
}
pub(super) fn next_id() -> u64 {
    write(|a| {
        a.workspace.serial += 1;
        a.workspace.serial
    })
}
pub(super) fn create_document(
    title: &str,
    content: super::metadata::MetadataView,
    preview: bool,
) -> anyhow::Result<HWND> {
    let (mdi, maximized) = read(|a| (a.workspace.mdi, a.workspace.maximized));
    let mut init = host::Init {
        content: Some(content),
        preview,
        error: None,
    };
    // CreateMDIWindowW consumes this pointer synchronously in WM_CREATE.
    let hwnd = unsafe {
        CreateMDIWindowW(
            w!("Winlex.Document"),
            wide(title).as_ptr(),
            WS_VISIBLE | WS_OVERLAPPEDWINDOW | if maximized { WS_MAXIMIZE } else { 0 },
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            mdi,
            instance(),
            &mut init as *mut host::Init as isize,
        )
    };
    if hwnd.is_null() {
        return Err(init.error.unwrap_or_else(|| error("Create MDI document")));
    }
    let placement = match normal_placement(hwnd) {
        Ok(placement) => placement,
        Err(error) => {
            close_document(hwnd);
            return Err(error);
        }
    };
    edit_document(hwnd, |d| d.placement = Some(placement));
    refresh_document_tabs();
    if let Some(content) = view(hwnd) {
        content.prepare(hwnd);
    }
    Ok(hwnd)
}
pub(super) fn view(hwnd: HWND) -> Option<std::rc::Rc<super::metadata::MetadataView>> {
    document(hwnd, |d| d.content.clone())
}
pub(super) fn views() -> Vec<(HWND, std::rc::Rc<super::metadata::MetadataView>)> {
    read(|a| {
        a.workspace
            .documents
            .iter()
            .map(|d| (d.hwnd, d.content.clone()))
            .collect()
    })
}
pub(super) fn complete(id: u64, ticket: u64, result: anyhow::Result<crate::rich_text::Content>) {
    let target = read(|a| {
        a.workspace
            .documents
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.content.clone())
    });
    if let Some(view) = target {
        view.complete(ticket, result);
    }
}

//! Bound document controls and focus. Every document has content and relevant actions only.
use super::catalog::{Binding, DocumentQuery};
use super::workspace::{
    arrange_documents, capture_placements, refresh_document_tabs, sync_document_selection,
    tiled_bounds,
};
use super::{Document, bind, document, dpi, edit_document, font_for, read, workspace};
use crate::{
    layout::{self, Placement},
    native::*,
    resources::{self, *},
};
use std::sync::Arc;

pub(super) fn prepare_document(doc: &Document) {
    let (font, _) = font_for(dpi().window(doc.hwnd));
    bind(doc.declaration, font);
    for &(_, h) in &doc.actions {
        bind(h, font);
    }
}
pub(super) fn create_actions(hwnd: HWND) -> Result<(), String> {
    let doc = document(hwnd).ok_or("Document no longer exists")?;
    let commands: &[(u16, u16)] = if doc.query.is_source() {
        &[
            (LOAD_METADATA, IDS_LOAD_METADATA),
            (REMOVE_SOURCE, IDS_UNLOAD),
            (RELOAD_SOURCE, IDS_RELOAD),
            (REPLACE_SOURCE, IDS_REPLACE),
            (REFRESH_SOURCE, IDS_REFRESH),
        ]
    } else {
        &[
            (OPEN_OTHER_ARCH, IDS_OPEN_OTHER_ARCH),
            (OPEN_CURRENT, IDS_OPEN_CURRENT),
        ]
    };
    let mut actions = Vec::new();
    for &(id, label) in commands {
        let caption = resources::text(label).replace("{arch}", &doc.architecture.other().label());
        let h = child(hwnd, "BUTTON", &caption, WS_TABSTOP, id);
        if h.is_null() || unsafe { set_subclass(h, Some(focus_proc), 1, hwnd as usize) } == 0 {
            return Err(error("Create document action"));
        }
        if id == OPEN_OTHER_ARCH
            && let Binding::Entity {
                architecture_mask, ..
            } = &doc.query.binding
        {
            unsafe {
                EnableWindow(
                    h,
                    i32::from(doc.architecture.other().supported_by(*architecture_mask)),
                );
            }
        }
        actions.push((id, h));
    }
    edit_document(hwnd, |d| d.actions = actions);
    Ok(())
}
pub(super) fn layout_document(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let value = dpi().window(hwnd);
    let (_, font_height) = font_for(value);
    let px = |n| layout::scale(n, value);
    let r = client(hwnd);
    let margin = px(8);
    let row = font_height + px(12);
    let available = (r.right - margin * 2).max(1);
    let width = px(if doc.query.is_source() { 100 } else { 180 }).min(available);
    let columns = (available / width).max(1);
    let mut count = 0;
    for &(_, h) in doc
        .actions
        .iter()
        .filter(|&&(id, _)| id != OPEN_CURRENT || doc.old_version)
    {
        place(
            h,
            layout::rect(
                margin + count % columns * width,
                margin + count / columns * row,
                width,
                row,
            ),
        );
        count += 1;
    }
    let top = margin + (count + columns - 1) / columns * row + px(6);
    place(
        doc.declaration,
        layout::rect(margin, top, available, r.bottom - top - margin),
    );
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
        focus(if valid { h } else { doc.declaration });
    }
}
pub(super) fn refresh_title(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    set_text(
        hwnd,
        &if doc.query.is_source() {
            doc.title.clone()
        } else {
            format!("{} — {}", doc.title, doc.architecture_name())
        },
    );
    refresh_document_tabs();
}
pub(super) fn refresh_content(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let header = if doc.query.is_source() {
        format!("{}\r\n\r\n", doc.title)
    } else {
        format!(
            "{} — {}\r\n{}\r\n{}\r\n\r\n",
            doc.title,
            doc.architecture_name(),
            doc.query.source,
            doc.query.entity().unwrap_or("")
        )
    };
    let old = super::catalog::old_version(&doc.query.binding);
    edit_document(hwnd, |d| d.old_version = old);
    let content = if old {
        format!(
            "{}\r\n{header}{}",
            resources::text(IDS_OLD_VERSION),
            doc.query.content
        )
    } else {
        header + &doc.query.content
    };
    set_text(doc.declaration, &content);
    for (id, h) in doc.actions {
        if id == OPEN_CURRENT {
            show(h, old);
        }
    }
    layout_document(hwnd);
}
fn open_other_arch(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let Binding::Entity {
        id,
        catalog,
        architecture_mask,
    } = &doc.query.binding
    else {
        return;
    };
    let arch = doc.architecture.other();
    if !arch.supported_by(*architecture_mask) {
        return;
    }
    let existing = read(|a| {
        a.documents.iter().find(|d| d.architecture == arch && matches!(&d.query.binding,
        Binding::Entity { id: other, catalog: snapshot, .. } if other == id && Arc::ptr_eq(snapshot, catalog))).map(|d| d.hwnd)
    });
    if let Some(h) = existing {
        workspace::activate_document(h);
    } else if let Err(error) = workspace::create_document(&doc.title, arch, doc.query.binding) {
        super::catalog::set_status(error);
    }
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
                let init = &*(mdi_create.lParam as *const workspace::DocumentInit<'_>);
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
                if declaration.is_null()
                    || set_subclass(declaration, Some(focus_proc), 1, hwnd as usize) == 0
                {
                    return -1;
                }
                send(declaration, EM_SETLIMITTEXT, 0x7ffffffe, 0);
                let placement = Placement {
                    bounds: bounds(hwnd, read(|a| a.mdi)),
                    dpi: dpi().window(hwnd),
                };
                workspace::register_document(Document {
                    actions: Vec::new(),
                    old_version: false,
                    id: super::write(|a| a.session.next_id()),
                    hwnd,
                    declaration,
                    last_focus: declaration,
                    placement,
                    title: init.title.to_owned(),
                    architecture: init.architecture,
                    query: DocumentQuery::new(init.binding.clone()),
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
                    Some(dpi().child_outer(hwnd, client(read(|a| a.mdi))))
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
            WM_COMMAND if w as u16 == OPEN_OTHER_ARCH => {
                open_other_arch(hwnd);
                return 0;
            }
            WM_COMMAND if w as u16 == OPEN_CURRENT => {
                super::catalog::open_current(hwnd);
                return 0;
            }
            WM_COMMAND
                if matches!(
                    w as u16,
                    LOAD_METADATA | REMOVE_SOURCE | REPLACE_SOURCE | RELOAD_SOURCE | REFRESH_SOURCE
                ) =>
            {
                super::sources::action(hwnd, w as u16);
                return 0;
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
unsafe extern "system" fn focus_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    owner: usize,
) -> isize {
    unsafe {
        if msg == WM_SETFOCUS {
            edit_document(owner as HWND, |d| d.last_focus = hwnd);
        }
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(focus_proc), 1);
        }
        default_subclass(hwnd, msg, w, l)
    }
}

//! MDI document host. Content owns its controls, commands and asynchronous work.
use super::{document, dpi, edit_document, metadata, read, workspace};
use crate::{layout::Placement, native::*};

#[derive(Clone)]
pub(super) enum Content {
    Metadata(metadata::Document),
}
pub(super) struct Init {
    pub content: Option<Content>,
    pub error: Option<String>,
}
impl Content {
    fn create(&mut self, hwnd: HWND) -> Result<(), String> {
        match self {
            Self::Metadata(d) => d.create(hwnd),
        }
    }
    pub fn cancel(&self) {
        match self {
            Self::Metadata(d) => d.cancel(),
        }
    }
    pub fn prepare(&self, hwnd: HWND) {
        match self {
            Self::Metadata(d) => d.prepare(hwnd),
        }
        self.layout(hwnd);
    }
    fn layout(&self, hwnd: HWND) {
        match self {
            Self::Metadata(d) => d.layout(hwnd),
        }
    }
    fn focus(&self) -> HWND {
        match self {
            Self::Metadata(d) => d.editor,
        }
    }
    fn command(&self, hwnd: HWND, id: u16) -> bool {
        match self {
            Self::Metadata(d) => d.command(hwnd, id),
        }
    }
}
pub(super) fn layout_document(hwnd: HWND) {
    if let Some(content) = document(hwnd, |d| d.content.clone()) {
        content.layout(hwnd);
    }
}
pub(super) fn restore_focus(hwnd: HWND) {
    if let Some((h, default)) = document(hwnd, |d| (d.last_focus, d.content.focus())) {
        let valid = !h.is_null()
            && unsafe {
                IsWindow(h) != 0
                    && IsChild(hwnd, h) != 0
                    && IsWindowVisible(h) != 0
                    && IsWindowEnabled(h) != 0
            };
        focus(if valid { h } else { default });
    }
}
pub(super) fn track_focus(owner: HWND, control: HWND) -> Result<(), String> {
    if control.is_null()
        || unsafe { set_subclass(control, Some(focus_proc), 1, owner as usize) } == 0
    {
        Err(error("Create document control"))
    } else {
        Ok(())
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
                let init = &mut *(mdi_create.lParam as *mut Init);
                let mut content = init.content.take().unwrap();
                if let Err(error) = content.create(hwnd) {
                    init.error = Some(error);
                    return -1;
                }
                workspace::register_document(workspace::Document {
                    id: workspace::next_id(),
                    hwnd,
                    title: text(hwnd),
                    last_focus: content.focus(),
                    content,
                    placement: Placement {
                        bounds: bounds(hwnd, read(|a| a.workspace.mdi)),
                        dpi: dpi().window(hwnd),
                    },
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
                    Some(dpi().child_outer(hwnd, client(read(|a| a.workspace.mdi))))
                } else if IsIconic(hwnd) == 0 {
                    workspace::tiled_bounds(hwnd)
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
                workspace::arrange_documents();
                return result;
            }
            WM_ENTERSIZEMOVE => workspace::begin_free_placement(),
            WM_EXITSIZEMOVE => workspace::capture_placements(),
            WM_SETFOCUS => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                restore_focus(hwnd);
                return result;
            }
            WM_MDIACTIVATE => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                workspace::sync_document_selection();
                return result;
            }
            WM_COMMAND => {
                if let Some(content) = document(hwnd, |d| d.content.clone())
                    && content.command(hwnd, w as u16)
                {
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

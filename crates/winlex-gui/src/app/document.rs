//! MDI document host. Content owns its controls, commands and asynchronous work.
use super::{dpi, read, workspace};
use crate::native::*;
use std::rc::Rc;

pub(super) struct Init {
    pub content: Option<super::metadata::MetadataView>,
    pub preview: bool,
    pub error: Option<anyhow::Error>,
}
pub(super) fn layout_document(hwnd: HWND) {
    if let Some(content) = workspace::view(hwnd) {
        content.layout(hwnd);
    }
}
pub(super) fn focus_document(hwnd: HWND) {
    if let Some(view) = workspace::view(hwnd) {
        view.focus();
    }
}
pub(super) fn track_editor(owner: HWND, editor: HWND) -> anyhow::Result<()> {
    if unsafe { set_subclass(editor, Some(editor_proc), 1, owner as usize) } == 0 {
        anyhow::bail!("Subclass document editor failed")
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
                workspace::register_document(workspace::DocumentHost {
                    id: workspace::next_id(),
                    hwnd,
                    title: text(hwnd),
                    content: Rc::new(content),
                    placement: None,
                    preview: init.preview,
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
            WM_GETMINMAXINFO => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                if IsZoomed(hwnd) == 0 && IsIconic(hwnd) == 0 {
                    let info = &mut *(l as *mut MINMAXINFO);
                    let value = dpi().window(hwnd);
                    info.ptMinTrackSize.x = crate::layout::scale(320, value);
                    info.ptMinTrackSize.y = crate::layout::scale(400, value);
                }
                return result;
            }
            WM_WINDOWPOSCHANGING => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                // Native MDI uses system DPI for maximized bounds, including MDICLIENT
                // resizing. Match the PMv2 child's nonclient metrics before positioning.
                if dpi().mode == crate::dpi::Mode::Pmv2 && IsZoomed(hwnd) != 0 {
                    let r = dpi().child_outer(hwnd, client(read(|a| a.workspace.mdi)));
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
            WM_SYSCOMMAND if matches!((w & 0xfff0) as u32, SC_MAXIMIZE | SC_RESTORE) => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                workspace::set_maximized(IsZoomed(hwnd) != 0);
                workspace::capture_placements();
                return result;
            }
            WM_EXITSIZEMOVE => workspace::capture_placements(),
            WM_SETFOCUS => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                focus_document(hwnd);
                return result;
            }
            WM_MDIACTIVATE => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                workspace::sync_document_selection();
                return result;
            }
            WM_COMMAND => {
                let id = (w & 0xffff) as u16;
                let code = (w >> 16) as u32;
                if let Some(content) = workspace::view(hwnd)
                    && content.command(hwnd, id, code)
                {
                    return 0;
                }
            }
            WM_CONTEXTMENU => {
                if let Some(view) = workspace::view(hwnd)
                    && view.context_menu(w as HWND, l)
                {
                    return 0;
                }
            }
            WM_CTLCOLORSTATIC => {
                if let Some(view) = workspace::view(hwnd)
                    && let Some(brush) = view.color(l as HWND, w as HDC)
                {
                    return brush;
                }
            }
            WM_DESTROY => {
                if let Some(view) = workspace::view(hwnd) {
                    view.destroy();
                }
            }
            WM_NCDESTROY => {
                let content = workspace::view(hwnd);
                workspace::unregister_document(hwnd);
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                drop(content);
                workspace::refresh_document_tabs();
                return result;
            }
            _ => {}
        }
        DefMDIChildProcW(hwnd, msg, w, l)
    }
}
unsafe extern "system" fn editor_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    owner: usize,
) -> isize {
    unsafe {
        match msg {
            WM_GETDLGCODE if w == VK_TAB as usize => {
                return (DLGC_WANTARROWS | DLGC_WANTCHARS) as isize;
            }
            // RichEdit must not collapse an existing selection on a contextual click.
            WM_RBUTTONDOWN => return 0,
            WM_RBUTTONUP => {
                let mut point = POINT {
                    x: l as i16 as i32,
                    y: (l >> 16) as i16 as i32,
                };
                ClientToScreen(hwnd, &mut point);
                send(
                    owner as HWND,
                    WM_CONTEXTMENU,
                    hwnd as usize,
                    ((point.x as u16 as u32) | ((point.y as u16 as u32) << 16)) as isize,
                );
                return 0;
            }
            WM_CONTEXTMENU => return send(owner as HWND, msg, hwnd as usize, l),
            WM_KEYDOWN
                if w == VK_APPS as usize
                    || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0 =>
            {
                return send(owner as HWND, WM_CONTEXTMENU, hwnd as usize, -1);
            }
            WM_KEYUP
                if w == VK_APPS as usize
                    || w == VK_F10 as usize && GetKeyState(VK_SHIFT as i32) < 0 =>
            {
                return 0;
            }
            WM_NCDESTROY => {
                remove_subclass(hwnd, Some(editor_proc), 1);
            }
            _ => {}
        }
        default_subclass(hwnd, msg, w, l)
    }
}

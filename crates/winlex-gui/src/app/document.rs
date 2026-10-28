//! MDI document host. Content owns its controls, commands and asynchronous work.
use super::{document, dpi, metadata, read, workspace};
use crate::native::*;

#[derive(Clone)]
pub(super) enum Content {
    Metadata(metadata::Document),
}
pub(super) struct Init {
    pub content: Option<Content>,
    pub preview: bool,
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
    fn command(&self, hwnd: HWND, id: u16, code: u32) -> bool {
        match self {
            Self::Metadata(d) => d.command(hwnd, id, code),
        }
    }
}
pub(super) fn layout_document(hwnd: HWND) {
    if let Some(content) = document(hwnd, |d| d.content.clone()) {
        content.layout(hwnd);
    }
}
pub(super) fn focus_document(hwnd: HWND) {
    if let Some(body) = document(hwnd, |d| d.content.focus()) {
        focus(body);
    }
}
pub(super) fn track_editor(owner: HWND, editor: HWND) -> Result<(), String> {
    if editor.is_null()
        || unsafe { set_subclass(editor, Some(editor_proc), 1, owner as usize) } == 0
    {
        Err(error("Create document control"))
    } else {
        Ok(())
    }
}
/// Read-only text keeps arrows and characters but lets Tab reach the dialog manager.
pub(super) fn track_text(control: HWND) -> Result<(), String> {
    if control.is_null() || unsafe { set_subclass(control, Some(text_tab_proc), 1, 0) } == 0 {
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
                    content,
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
            WM_SIZING => {
                // A WM_GETMINMAXINFO minimum can block a DPI downscale.
                // Use WM_SIZING for interactive limits; allow programmatic MDI sizing.
                let rect = &mut *(l as *mut RECT);
                let value = dpi().window(hwnd);
                let width = crate::layout::scale(320, value);
                let height = crate::layout::scale(400, value);
                if rect.right - rect.left < width {
                    match w as u32 {
                        WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT => {
                            rect.left = rect.right - width
                        }
                        WMSZ_RIGHT | WMSZ_TOPRIGHT | WMSZ_BOTTOMRIGHT => {
                            rect.right = rect.left + width
                        }
                        _ => {}
                    }
                }
                if rect.bottom - rect.top < height {
                    match w as u32 {
                        WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT => rect.top = rect.bottom - height,
                        WMSZ_BOTTOM | WMSZ_BOTTOMLEFT | WMSZ_BOTTOMRIGHT => {
                            rect.bottom = rect.top + height
                        }
                        _ => {}
                    }
                }
                return 1;
            }
            WM_WINDOWPOSCHANGING => {
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                let r = if IsZoomed(hwnd) != 0 {
                    Some(dpi().child_outer(hwnd, client(read(|a| a.workspace.mdi))))
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
                if let Some(content) = document(hwnd, |d| d.content.clone())
                    && content.command(hwnd, id, code)
                {
                    return 0;
                }
            }
            WM_CONTEXTMENU => {
                if let Some(doc) = metadata::get(hwnd, |_, d| d.clone())
                    && w == doc.editor as usize
                {
                    if let Err(error) = crate::rich_text::context_menu(doc.editor, l, &doc.numbers)
                    {
                        super::catalog::set_status(error);
                    }
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
unsafe extern "system" fn text_tab_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        if msg == WM_GETDLGCODE && w == VK_TAB as usize {
            return (DLGC_WANTARROWS | DLGC_WANTCHARS) as isize;
        }
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(text_tab_proc), 1);
        }
        default_subclass(hwnd, msg, w, l)
    }
}

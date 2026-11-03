//! Native entry points and MDI messages for declaration documents.
use super::{dpi, history, metadata::MetadataView, read, workspace};
use crate::native::*;
use std::rc::Rc;
use windows::Win32::UI::Controls::RichEdit::{EN_LINK, ENLINK};

pub(super) const FOLLOW_REFERENCE: u32 = WM_APP + 42;

pub(super) struct Init {
    pub content: Option<super::metadata::MetadataView>,
    pub error: Option<anyhow::Error>,
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
                // These messages also arrive before WM_CREATE registers the document.
                // Native MDI uses system DPI; match the PMv2 child's nonclient metrics.
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
            WM_CREATE => {
                let create = &*(l as *const CREATESTRUCTW);
                let mdi_create = &*(create.lpCreateParams as *const MDICREATESTRUCTW);
                let init = &mut *(mdi_create.lParam as *mut Init);
                let mut content = init.content.take().unwrap();
                if let Err(error) = content.create(hwnd) {
                    init.error = Some(error);
                    return -1;
                }
                workspace::register_document(Rc::new(content));
                crate::theme::apply(hwnd);
                return 0;
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
        if let Some(document) = workspace::view(hwnd) {
            document.message(msg, w, l)
        } else {
            DefMDIChildProcW(hwnd, msg, w, l)
        }
    }
}
impl MetadataView {
    unsafe fn message(&self, msg: u32, w: usize, l: isize) -> isize {
        let hwnd = self.hwnd;
        unsafe {
            match msg {
                FOLLOW_REFERENCE => {
                    if self.id == w as u32 as u64 | ((l as u32 as u64) << 32) {
                        self.follow_reference();
                    }
                    return 0;
                }
                WM_NOTIFY => {
                    let header = (l as *const NMHDR).read_unaligned();
                    if header.code == windows::Win32::UI::Controls::RichEdit::EN_SELCHANGE {
                        self.selection_changed();
                        return 0;
                    }
                    if header.code == EN_LINK {
                        self.link_notification((l as *const ENLINK).read_unaligned());
                        return 0;
                    }
                }
                WM_CLOSE => {
                    workspace::close_document(hwnd);
                    return 0;
                }
                WM_SIZE => {
                    let result = DefMDIChildProcW(hwnd, msg, w, l);
                    if w != SIZE_MINIMIZED as usize && self.is_live() {
                        self.layout();
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
                    if self.is_live() {
                        self.focus();
                    }
                    return result;
                }
                WM_MDIACTIVATE => {
                    if w as HWND == hwnd {
                        history::activation(self, false);
                    }
                    let result = DefMDIChildProcW(hwnd, msg, w, l);
                    workspace::sync_document_selection();
                    if l as HWND == hwnd {
                        history::activation(self, true);
                    }
                    return result;
                }
                WM_COMMAND => {
                    let id = (w & 0xffff) as u16;
                    let code = (w >> 16) as u32;
                    if self.command(id, code) {
                        return 0;
                    }
                }
                WM_CONTEXTMENU => {
                    if self.context_menu(w as HWND, l) {
                        return 0;
                    }
                }
                WM_CTLCOLORSTATIC => {
                    if let Some(brush) = self.color(l as HWND, w as HDC) {
                        return brush;
                    }
                }
                WM_DESTROY => self.destroy(),
                _ => {}
            }
            DefMDIChildProcW(hwnd, msg, w, l)
        }
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
        let document = workspace::view(owner as HWND);
        if let Some(document) = &document {
            if matches!(
                msg,
                WM_LBUTTONDOWN
                    | WM_LBUTTONDBLCLK
                    | WM_RBUTTONDOWN
                    | WM_MOUSEWHEEL
                    | WM_MOUSEHWHEEL
                    | WM_VSCROLL
                    | WM_HSCROLL
            ) || msg == WM_KEYDOWN
                && (matches!(
                    w as u16,
                    VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT
                ) || w == VK_A as usize && GetKeyState(VK_CONTROL as i32) < 0)
            {
                document.cancel_restore();
            }
            match msg {
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_KILLFOCUS
                | WM_CANCELMODE | WM_SIZE | WM_SETFONT | WM_MOUSEWHEEL | WM_MOUSEHWHEEL
                | WM_VSCROLL | WM_HSCROLL | WM_KEYDOWN | WM_SYSKEYDOWN => {
                    document.cancel_link_press()
                }
                WM_CAPTURECHANGED if !document.releasing_link.get() || l != 0 => {
                    document.cancel_link_press();
                }
                WM_LBUTTONUP => document.releasing_link.set(true),
                WM_RBUTTONUP => document.clear_context_link(),
                WM_MOUSEMOVE => document.move_link_press(POINT {
                    x: l as i16 as i32,
                    y: (l >> 16) as i16 as i32,
                }),
                _ => {}
            }
        }
        match msg {
            WM_GETDLGCODE if w == VK_TAB as usize => {
                return (DLGC_WANTARROWS | DLGC_WANTCHARS) as isize;
            }
            // RichEdit must not collapse an existing selection on a contextual click.
            WM_RBUTTONDOWN => return 0,
            WM_RBUTTONUP => {
                // Native right-up focuses before WM_CONTEXTMENU and scrolls to the old caret.
                let view = crate::rich_text::ViewState::capture(hwnd);
                focus(hwnd);
                view.restore(hwnd);
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
        let result = default_subclass(hwnd, msg, w, l);
        if msg == WM_LBUTTONUP
            && let Some(document) = document
        {
            document.releasing_link.set(false);
            document.cancel_link_press();
        }
        result
    }
}

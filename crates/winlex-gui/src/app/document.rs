//! Native MDI shell lifecycle and content-specific message dispatch.
use super::content::Content;
use super::{dpi, history, metadata::MetadataView, read, workspace};
use crate::native::*;
use std::{cell::RefCell, rc::Rc};
use windows::Win32::UI::Controls::RichEdit::{EN_LINK, ENLINK};

pub(super) const FOLLOW_REFERENCE: u32 = WM_APP + 42;
const CONTENT_CHANGED: u32 = WM_APP + 43;
const STATUS_CHANGED: u32 = WM_APP + 44;
pub(super) fn status_changed(hwnd: HWND) {
    send(hwnd, STATUS_CHANGED, 0, 0);
}
pub(super) fn update_title(hwnd: HWND, title: &str) {
    set_text(hwnd, title);
    send(hwnd, CONTENT_CHANGED, 0, 0);
}
pub(super) fn update_icons(
    hwnd: HWND,
    icon: crate::icons::Icon,
    current: &RefCell<Option<Rc<crate::icons::WindowIcons>>>,
) -> anyhow::Result<()> {
    let value = dpi().window(hwnd);
    let icons = super::write(|a| a.workspace.icons.get(icon, value))?;
    if current
        .borrow()
        .as_ref()
        .is_some_and(|old| Rc::ptr_eq(old, &icons))
    {
        return Ok(());
    }
    icons.apply(hwnd);
    current.replace(Some(icons));
    if unsafe { IsZoomed(hwnd) } != 0 && workspace::active() == hwnd {
        unsafe { DrawMenuBar(read(|a| a.frame)) };
    }
    Ok(())
}

pub(super) struct Init {
    pub content: Option<Seed>,
    pub error: Option<anyhow::Error>,
}
pub(super) enum Seed {
    Declaration(Box<MetadataView>),
    Tool(Box<super::tools::ToolView>),
}
impl Seed {
    fn create(self, hwnd: HWND) -> anyhow::Result<Content> {
        match self {
            Self::Declaration(mut v) => {
                v.create(hwnd)?;
                Ok(Content::Declaration(Rc::from(v)))
            }
            Self::Tool(mut v) => {
                v.create(hwnd)?;
                Ok(Content::Tool(Rc::from(v)))
            }
        }
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
                match init.content.take().unwrap().create(hwnd) {
                    Ok(content) => workspace::register_document(content),
                    Err(error) => {
                        init.error = Some(error);
                        return -1;
                    }
                }
                crate::theme::apply(hwnd);
                return 0;
            }
            WM_NCDESTROY => {
                let content = workspace::content(hwnd);
                workspace::unregister_document(hwnd);
                let result = DefMDIChildProcW(hwnd, msg, w, l);
                drop(content);
                workspace::refresh_document_tabs();
                return result;
            }
            _ => {}
        }
        if let Some(content) = workspace::content(hwnd) {
            match msg {
                STATUS_CHANGED => {
                    if workspace::active() == hwnd {
                        super::frame::refresh_status();
                    }
                    return 0;
                }
                CONTENT_CHANGED => {
                    workspace::refresh_document_tabs();
                    super::frame::update_commands();
                    return 0;
                }
                WM_CLOSE => {
                    if workspace::active() == hwnd {
                        super::frame::begin_status(super::frame::StatusContext::Document(hwnd));
                    }
                    workspace::close_document(hwnd);
                    return 0;
                }
                WM_SIZE => {
                    let result = DefMDIChildProcW(hwnd, msg, w, l);
                    if w != SIZE_MINIMIZED as usize
                        && workspace::content_by_id(content.id()).is_some()
                    {
                        content.layout();
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
                    super::frame::status_context(super::frame::StatusContext::Document(hwnd));
                    let result = DefMDIChildProcW(hwnd, msg, w, l);
                    if workspace::content_by_id(content.id()).is_some() {
                        content.focus();
                    }
                    return result;
                }
                WM_MDIACTIVATE => {
                    if w as HWND == hwnd {
                        history::content_activation(&content, false);
                    }
                    let result = DefMDIChildProcW(hwnd, msg, w, l);
                    workspace::sync_document_selection();
                    if l as HWND == hwnd {
                        super::frame::begin_status(super::frame::StatusContext::Document(hwnd));
                        history::content_activation(&content, true);
                    }
                    super::frame::update_commands();
                    return result;
                }
                WM_DESTROY => {
                    content.destroy();
                    return DefMDIChildProcW(hwnd, msg, w, l);
                }
                WM_COMMAND if l == 0 || matches!((w >> 16) as u32, BN_CLICKED | CBN_SELENDOK) => {
                    if workspace::active() == hwnd {
                        super::frame::begin_status(super::frame::StatusContext::Document(hwnd));
                    }
                }
                WM_COMMAND if (w >> 16) as u32 == EN_SETFOCUS => {
                    super::frame::status_context(super::frame::StatusContext::Document(hwnd));
                }
                _ => {}
            }
            match content {
                Content::Declaration(document) => document.message(msg, w, l),
                Content::Tool(tool) => tool
                    .message(msg, w, l)
                    .unwrap_or_else(|| DefMDIChildProcW(hwnd, msg, w, l)),
            }
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
        if msg == WM_NCDESTROY {
            remove_subclass(hwnd, Some(editor_proc), 1);
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

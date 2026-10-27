//! Registered panel host: visibility, toolbar, width and focus handoff.
use super::{
    bind, dpi,
    panel::{PANELS, Panel, Registration},
    read, write,
};
use crate::{
    config::Config,
    layout,
    native::*,
    resources::{self, *},
};
use std::ptr;

#[derive(Clone)]
pub(super) struct Sidebar {
    toolbar: HWND,
    splitter: HWND,
    panels: Vec<(&'static Registration, Panel)>,
    pub active: u32,
    pub collapsed: bool,
    pub width: i32,
}
impl Sidebar {
    pub fn new(config: &Config) -> Self {
        let active = PANELS
            .iter()
            .find(|r| r.id == config.panel)
            .unwrap_or(&PANELS[0])
            .id;
        Self {
            toolbar: ptr::null_mut(),
            splitter: ptr::null_mut(),
            panels: vec![],
            active,
            collapsed: config.collapsed,
            width: config.sidebar,
        }
    }
    pub fn panel(&self, id: u32) -> &Panel {
        &self.panels.iter().find(|(r, _)| r.id == id).unwrap().1
    }
    pub fn panel_mut(&mut self, id: u32) -> &mut Panel {
        &mut self.panels.iter_mut().find(|(r, _)| r.id == id).unwrap().1
    }
    pub fn owner(&self, hwnd: HWND) -> Option<Panel> {
        self.panels
            .iter()
            .find(|(_, p)| p.controls().any(|h| h == hwnd))
            .map(|(_, p)| p.clone())
    }
}
pub(super) fn focus_target() -> HWND {
    read(|a| {
        if a.sidebar.collapsed {
            a.sidebar.toolbar
        } else {
            a.sidebar.panel(a.sidebar.active).focus()
        }
    })
}
pub(super) fn create(frame: HWND) -> bool {
    let toolbar = child(
        frame,
        "ToolbarWindow32",
        "",
        WS_TABSTOP
            | TBSTYLE_FLAT
            | TBSTYLE_LIST
            | TBSTYLE_TOOLTIPS
            | TBSTYLE_WRAPABLE
            | (CCS_NORESIZE | CCS_NOPARENTALIGN | CCS_NODIVIDER) as u32,
        PANEL_TOOLBAR,
    );
    let splitter = child(frame, "Winlex.Splitter", "", 0, 0);
    if toolbar.is_null() || splitter.is_null() {
        return false;
    }
    write(|a| {
        a.sidebar.toolbar = toolbar;
        a.sidebar.splitter = splitter;
    });
    send(
        toolbar,
        TB_BUTTONSTRUCTSIZE,
        std::mem::size_of::<TBBUTTON>(),
        0,
    );
    send(toolbar, TB_SETMAXTEXTROWS, 1, 0);
    send(toolbar, TB_SETBITMAPSIZE, 0, 0);
    for registration in PANELS {
        let mut text = wide(&resources::text(registration.label));
        text.push(0);
        let index = send(toolbar, TB_ADDSTRINGW, 0, text.as_ptr() as isize);
        if index < 0 {
            return false;
        }
        let button = TBBUTTON {
            iBitmap: I_IMAGENONE,
            idCommand: registration.command as i32,
            fsState: TBSTATE_ENABLED as u8,
            fsStyle: BTNS_CHECK as u8 | BTNS_SHOWTEXT as u8,
            iString: index,
            ..unsafe { std::mem::zeroed() }
        };
        if send(toolbar, TB_ADDBUTTONSW, 1, &button as *const _ as isize) == 0 {
            return false;
        }
        let Some(panel) = (registration.create)(frame) else {
            return false;
        };
        write(|a| a.sidebar.panels.push((registration, panel)));
    }
    true
}
pub(super) fn show(id: u32, toggle: bool) {
    write(|a| {
        let s = &mut a.sidebar;
        s.collapsed = toggle && s.active == id && !s.collapsed;
        s.active = id;
    });
    super::frame::layout_frame();
    unsafe {
        let focused = GetFocus();
        if !focused.is_null() && IsWindowVisible(focused) == 0 {
            focus(focus_target());
        }
    }
}
pub(super) fn prepare() {
    let (frame, sidebar) = read(|a| (a.frame, a.sidebar.clone()));
    let m = super::metrics(frame);
    bind(sidebar.toolbar, m.font);
    let width = m.label_width(PANELS.iter().map(|r| resources::text(r.label)), 66) as u32;
    let row = m.row() as u32;
    send(
        sidebar.toolbar,
        TB_SETBUTTONSIZE,
        0,
        (width | (row << 16)) as isize,
    );
    send(
        sidebar.toolbar,
        TB_SETBUTTONWIDTH,
        0,
        (width | (width << 16)) as isize,
    );
    for (_, panel) in sidebar.panels {
        panel.prepare(&m);
    }
}
pub(super) fn layout(bottom: i32) -> i32 {
    let (frame, sidebar) = read(|a| (a.frame, a.sidebar.clone()));
    let m = super::metrics(frame);
    let s = |n| m.px(n);
    let x = (send(sidebar.toolbar, TB_GETBUTTONSIZE, 0, 0) & 0xffff) as i32 + s(6);
    let width = s(sidebar.width).min((client(frame).right - x - s(160)).max(0));
    place(
        sidebar.toolbar,
        layout::rect(s(3), s(4), x - s(6), bottom - s(8)),
    );
    for (registration, panel) in sidebar.panels {
        let visible = !sidebar.collapsed && sidebar.active == registration.id;
        send(
            sidebar.toolbar,
            TB_CHECKBUTTON,
            registration.command as usize,
            isize::from(visible),
        );
        for control in panel.controls() {
            crate::native::show(control, visible);
        }
        if visible {
            panel.layout(layout::rect(x, 0, width, bottom), &m);
        }
    }
    crate::native::show(sidebar.splitter, !sidebar.collapsed);
    if sidebar.collapsed {
        return x;
    }
    place(sidebar.splitter, layout::rect(x + width, 0, s(5), bottom));
    x + width + s(5)
}
pub(super) fn command(w: usize) -> bool {
    let id = w as u16;
    if let Some(r) = PANELS.iter().find(|r| r.command == id) {
        show(r.id, true);
        return true;
    }
    let panels = read(|a| a.sidebar.panels.clone());
    panels.iter().any(|(_, p)| p.command(id, (w >> 16) as u32))
}
pub(super) unsafe fn notify(l: isize) -> Option<isize> {
    let hwnd = unsafe { (*(l as *const NMHDR)).hwndFrom };
    let panel = read(|a| a.sidebar.owner(hwnd))?;
    unsafe { panel.notify(l) }
}
pub(super) unsafe fn translate(frame: HWND, msg: &MSG) -> bool {
    let (toolbar, panel) = read(|a| (a.sidebar.toolbar, a.sidebar.owner(msg.hwnd)));
    if let Some(panel) = &panel
        && msg.message == WM_KEYDOWN
        && panel.key(msg.hwnd, msg.wParam)
    {
        return true;
    }
    (msg.hwnd == toolbar || panel.is_some()) && unsafe { IsDialogMessageW(frame, msg) != 0 }
}
pub(super) unsafe extern "system" fn splitter_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
) -> isize {
    unsafe {
        match msg {
            WM_SETCURSOR => {
                SetCursor(LoadCursorW(ptr::null_mut(), IDC_SIZEWE));
                return 1;
            }
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                return 0;
            }
            WM_MOUSEMOVE if GetCapture() == hwnd => {
                let mut p = POINT {
                    x: l as u16 as i16 as i32,
                    y: (l >> 16) as u16 as i16 as i32,
                };
                let (frame, toolbar) = read(|a| (a.frame, a.sidebar.toolbar));
                MapWindowPoints(hwnd, frame, &mut p, 1);
                let x = bounds(toolbar, frame).right + layout::scale(3, dpi().window(frame));
                let width = MulDiv(p.x - x, 96, dpi().window(frame) as i32).clamp(120, 2000);
                write(|a| a.sidebar.width = width);
                super::frame::layout_frame();
                return 0;
            }
            WM_LBUTTONUP => {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

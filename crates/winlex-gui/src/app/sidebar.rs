//! Sidebar visibility, toolbar, width and focus handoff.
use super::{
    bind, dpi,
    panel::{self, Panel, PanelId, Search},
    read, write,
};
use crate::{
    config::Config,
    icons::Images,
    layout,
    native::*,
    resources::{self, *},
};
use std::{ptr, rc::Rc};

pub(super) struct Sidebar {
    toolbar: HWND,
    images: Option<Images>,
    splitter: HWND,
    panels: Vec<Rc<Panel>>,
    pub search: Option<Rc<Search>>,
    pub active: PanelId,
    pub collapsed: bool,
    pub width: i32,
}
impl Sidebar {
    pub fn new(config: &Config) -> Self {
        let key = config.key.as_ref();
        Self {
            toolbar: ptr::null_mut(),
            images: None,
            splitter: ptr::null_mut(),
            panels: vec![],
            search: None,
            active: PanelId::from_saved(key.and_then(|k| k.number("ActivePanel")).unwrap_or(0)),
            collapsed: key.and_then(|k| k.number("PanelCollapsed")) == Some(1),
            width: key
                .and_then(|k| k.number("SidebarWidth"))
                .filter(|v| (120..=2000).contains(v))
                .unwrap_or(240) as i32,
        }
    }
    pub fn save(&self, key: &crate::config::Key) -> anyhow::Result<()> {
        key.set_number("SidebarWidth", self.width as u32)?;
        key.set_number("ActivePanel", self.active as u32)?;
        key.set_number("PanelCollapsed", u32::from(self.collapsed))
    }
    pub fn panel(&self, id: PanelId) -> Rc<Panel> {
        self.panels[id as usize].clone()
    }
    pub fn owner(&self, hwnd: HWND) -> Option<Rc<Panel>> {
        self.panels
            .iter()
            .find(|p| p.controls.contains(&hwnd))
            .cloned()
    }
}
pub(super) fn focus() {
    let (toolbar, panel) = read(|a| {
        (
            a.sidebar.toolbar,
            (!a.sidebar.collapsed).then(|| a.sidebar.panel(a.sidebar.active)),
        )
    });
    if let Some(panel) = panel {
        panel.focus();
    } else {
        crate::native::focus(toolbar);
    }
}
pub(super) fn create(frame: HWND) -> anyhow::Result<()> {
    let toolbar = unsafe {
        child(
            frame,
            w!("ToolbarWindow32"),
            &[0],
            WS_TABSTOP
                | TBSTYLE_FLAT
                | TBSTYLE_TOOLTIPS
                | TBSTYLE_WRAPABLE
                | (CCS_NORESIZE | CCS_NOPARENTALIGN | CCS_NODIVIDER) as u32,
            PANEL_TOOLBAR,
        )
    }?;
    let splitter = unsafe { child(frame, w!("Winlex.Splitter"), &[0], 0, 0) }?;
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
    send(toolbar, TB_SETMAXTEXTROWS, 0, 0);
    for id in PanelId::ALL {
        let (command, label, _) = id.presentation();
        let mut text = resources::wide_text(label).to_vec();
        text.push(0);
        let index = send(toolbar, TB_ADDSTRINGW, 0, text.as_ptr() as isize);
        if index < 0 {
            anyhow::bail!("Add panel toolbar string failed");
        }
        let button = TBBUTTON {
            iBitmap: id as i32,
            idCommand: command as i32,
            fsState: TBSTATE_ENABLED as u8,
            fsStyle: BTNS_CHECK as u8,
            iString: index,
            ..unsafe { std::mem::zeroed() }
        };
        if send_ref(toolbar, TB_ADDBUTTONSW, 1, &button) == 0 {
            anyhow::bail!("Add panel toolbar button failed");
        }
    }
    let (panels, search) = panel::create(frame)?;
    write(|a| {
        a.sidebar.panels = panels.into();
        a.sidebar.search = Some(search);
    });
    sync_visibility();
    Ok(())
}
pub(super) fn show(id: PanelId, toggle: bool) {
    write(|a| {
        let s = &mut a.sidebar;
        s.collapsed = toggle && s.active == id && !s.collapsed;
        s.active = id;
    });
    sync_visibility();
    super::frame::layout_frame();
    focus();
}
fn sync_visibility() {
    let (toolbar, splitter, collapsed, active, panels) = read(|a| {
        let s = &a.sidebar;
        (
            s.toolbar,
            s.splitter,
            s.collapsed,
            s.active,
            s.panels.clone(),
        )
    });
    for (id, panel) in PanelId::ALL.into_iter().zip(panels) {
        let visible = !collapsed && active == id;
        send(
            toolbar,
            TB_CHECKBUTTON,
            id.presentation().0 as usize,
            isize::from(visible),
        );
        for &control in &panel.controls {
            crate::native::show(control, visible);
        }
    }
    crate::native::show(splitter, !collapsed);
}
pub(super) fn prepare() {
    let (frame, toolbar, panels) = read(|a| (a.frame, a.sidebar.toolbar, a.sidebar.panels.clone()));
    let m = super::metrics(frame);
    bind(toolbar, m.font);
    update_images(toolbar, m.px(24));
    for (message, size) in [
        (TB_SETPADDING, 12),
        (TB_SETBUTTONWIDTH, 36),
        (TB_SETBUTTONSIZE, 36),
    ] {
        let size = m.px(size) as u32;
        send(toolbar, message, 0, (size | (size << 16)) as isize);
    }
    for panel in panels {
        panel.prepare(&m);
    }
    panel::search().prepare(&m);
}
fn update_images(toolbar: HWND, size: i32) {
    if let Some(images) = super::checked(Images::toolbar(
        size,
        PanelId::ALL.into_iter().map(|id| id.presentation().2),
    )) {
        send(toolbar, TB_SETIMAGELIST, 0, images.handle());
        write(|a| a.sidebar.images = Some(images));
    }
}
pub(super) fn recolor() {
    let (frame, toolbar, panels) = read(|a| (a.frame, a.sidebar.toolbar, a.sidebar.panels.clone()));
    update_images(toolbar, layout::scale(24, dpi().window(frame)));
    for panel in panels {
        panel.recolor();
    }
}
pub(super) fn layout(bottom: i32, positions: &mut Vec<(HWND, RECT)>) -> i32 {
    let (frame, toolbar, splitter, width, panel) = read(|a| {
        let s = &a.sidebar;
        (
            a.frame,
            s.toolbar,
            s.splitter,
            s.width,
            (!s.collapsed).then(|| s.panel(s.active)),
        )
    });
    let m = super::metrics(frame);
    let s = |n| m.px(n);
    let x = (send(toolbar, TB_GETBUTTONSIZE, 0, 0) & 0xffff) as i32 + s(8);
    let width = s(width).min((client(frame).right - x - s(160)).max(0));
    positions.push((toolbar, layout::rect(s(4), s(4), x - s(8), bottom - s(8))));
    let Some(panel) = panel else {
        return x;
    };
    let mut area = layout::inset(layout::rect(x, 0, width, bottom), m.px(4));
    match read(|a| a.sidebar.active) {
        PanelId::Search => panel::search().layout(&mut area, &m, positions),
        PanelId::Sources => m.buttons(&mut area, panel.controls[1..].iter().copied(), positions),
    }
    positions.push((panel.list.hwnd, area));
    positions.push((splitter, layout::rect(x + width, 0, s(3), bottom)));
    x + width + s(3)
}
pub(super) unsafe fn translate(frame: HWND, msg: &MSG) -> bool {
    let (toolbar, panel) = read(|a| (a.sidebar.toolbar, a.sidebar.owner(msg.hwnd)));
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
                let x = bounds(toolbar, frame).right + layout::scale(4, dpi().window(frame));
                let width = MulDiv(
                    p.x - x,
                    USER_DEFAULT_SCREEN_DPI as i32,
                    dpi().window(frame) as i32,
                )
                .clamp(120, 2000);
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

//! Sidebar visibility, toolbar, width and focus handoff.
use super::{
    bind,
    panel::{self, Panel, PanelId, Search},
};
use crate::app::context::Context;
use crate::{
    config::Config,
    icons::Images,
    layout,
    native::*,
    resources::{self, *},
};
use std::{ptr, rc::Rc};

const MIN_WIDTH: i32 = 120;
const MAX_WIDTH: i32 = 2000;

pub(super) struct Sidebar {
    toolbar: HWND,
    images: Option<Images>,
    splitter: HWND,
    panels: Vec<Rc<Panel>>,
    search: Option<Rc<Search>>,
    active: PanelId,
    collapsed: bool,
    width: i32,
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
                .filter(|v| (MIN_WIDTH as u32..=MAX_WIDTH as u32).contains(v))
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
pub(super) fn active(ctx: &Context) -> PanelId {
    ctx.read(|a| a.sidebar.active)
}
pub(super) fn panel(ctx: &Context, id: PanelId) -> Rc<Panel> {
    ctx.read(|a| a.sidebar.panel(id))
}
pub(super) fn owner(ctx: &Context, hwnd: HWND) -> Option<Rc<Panel>> {
    ctx.read(|a| a.sidebar.owner(hwnd))
}
pub(super) fn search(ctx: &Context) -> Option<Rc<Search>> {
    ctx.read(|a| a.sidebar.search.clone())
}
pub(super) fn current(ctx: &Context) -> Option<Rc<Panel>> {
    ctx.read(|a| {
        let s = &a.sidebar;
        (!s.collapsed).then(|| s.panel(s.active))
    })
}
pub(super) fn focus(ctx: &Context) {
    let (toolbar, panel) = ctx.read(|a| {
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
pub(super) fn create(ctx: &Context, frame: HWND) -> anyhow::Result<()> {
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
    ctx.write(|a| {
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
    let (panels, search) = panel::create(ctx, frame)?;
    ctx.write(|a| {
        a.sidebar.panels = panels.into();
        a.sidebar.search = Some(search);
    });
    sync_visibility(ctx);
    Ok(())
}
pub(super) fn show(ctx: &Context, id: PanelId, toggle: bool) {
    super::status::begin_status(ctx, super::status::StatusContext::Catalog);
    ctx.write(|a| {
        let s = &mut a.sidebar;
        s.collapsed = toggle && s.active == id && !s.collapsed;
        s.active = id;
    });
    sync_visibility(ctx);
    super::frame::layout_frame(ctx);
    focus(ctx);
}
fn sync_visibility(ctx: &Context) {
    let (toolbar, splitter, collapsed, active, panels) = ctx.read(|a| {
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
pub(super) fn prepare(ctx: &Context) {
    let (frame, toolbar, panels) =
        ctx.read(|a| (a.frame, a.sidebar.toolbar, a.sidebar.panels.clone()));
    let m = ctx.metrics(frame);
    bind(toolbar, m.font);
    update_images(ctx, toolbar, m.px(24));
    for (message, size) in [
        (TB_SETPADDING, 12),
        (TB_SETBUTTONWIDTH, 36),
        (TB_SETBUTTONSIZE, 36),
    ] {
        let size = m.px(size) as u32;
        send(toolbar, message, 0, (size | (size << 16)) as isize);
    }
    for panel in panels {
        panel.prepare(ctx, &m);
    }
    panel::search(ctx).prepare(&m);
}
fn update_images(ctx: &Context, toolbar: HWND, size: i32) {
    if let Some(images) = ctx.checked(Images::toolbar(
        size,
        PanelId::ALL.into_iter().map(|id| id.presentation().2),
    )) {
        send(toolbar, TB_SETIMAGELIST, 0, images.handle());
        ctx.write(|a| a.sidebar.images = Some(images));
    }
}
pub(super) fn recolor(ctx: &Context) {
    let (frame, toolbar, panels) =
        ctx.read(|a| (a.frame, a.sidebar.toolbar, a.sidebar.panels.clone()));
    update_images(ctx, toolbar, layout::scale(24, ctx.dpi().window(frame)));
    for panel in panels {
        panel.recolor(ctx);
    }
}
pub(super) fn layout(ctx: &Context, bottom: i32, positions: &mut Vec<(HWND, RECT)>) -> i32 {
    let (frame, toolbar, splitter, width, panel) = ctx.read(|a| {
        let s = &a.sidebar;
        (
            a.frame,
            s.toolbar,
            s.splitter,
            s.width,
            (!s.collapsed).then(|| s.panel(s.active)),
        )
    });
    let m = ctx.metrics(frame);
    let s = |n| m.px(n);
    let x = (send(toolbar, TB_GETBUTTONSIZE, 0, 0) & 0xffff) as i32 + s(8);
    let width = s(width).min((client(frame).right - x - s(160)).max(0));
    positions.push((toolbar, layout::rect(s(4), s(4), x - s(8), bottom - s(8))));
    let Some(panel) = panel else {
        return x;
    };
    let mut area = layout::inset(layout::rect(x, 0, width, bottom), m.px(4));
    match ctx.read(|a| a.sidebar.active) {
        PanelId::Search => panel::search(ctx).layout(ctx, &mut area, &m, positions),
        PanelId::Sources => m.buttons(&mut area, panel.controls[1..].iter().copied(), positions),
        PanelId::Tools => m.buttons(&mut area, panel.controls[1..].iter().copied(), positions),
    }
    positions.push((panel.list.hwnd, area));
    positions.push((splitter, layout::rect(x + width, 0, s(3), bottom)));
    x + width + s(3)
}
pub(super) unsafe fn translate(ctx: &Context, frame: HWND, msg: &MSG) -> bool {
    let (toolbar, panel) = ctx.read(|a| (a.sidebar.toolbar, a.sidebar.owner(msg.hwnd)));
    (msg.hwnd == toolbar || panel.is_some()) && unsafe { IsDialogMessageW(frame, msg) != 0 }
}
pub(super) unsafe extern "system" fn splitter_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
) -> isize {
    unsafe {
        let Some(ctx) = Context::from_window(hwnd) else {
            return DefWindowProcW(hwnd, msg, w, l);
        };
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
                let mut p = point_from_lparam(l);
                let (frame, toolbar) = ctx.read(|a| (a.frame, a.sidebar.toolbar));
                MapWindowPoints(hwnd, frame, &mut p, 1);
                let x = bounds(toolbar, frame).right + layout::scale(4, ctx.dpi().window(frame));
                let width = MulDiv(
                    p.x - x,
                    USER_DEFAULT_SCREEN_DPI as i32,
                    ctx.dpi().window(frame) as i32,
                )
                .clamp(MIN_WIDTH, MAX_WIDTH);
                ctx.write(|a| a.sidebar.width = width);
                super::frame::layout_frame(ctx);
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

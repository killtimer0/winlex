//! Internal registration and content contract. No dynamic plugins or persisted sessions.
use super::{content::Content, history, read, workspace};
use crate::{
    icons::Icon,
    layout,
    list::{Display, TextSegment},
    native::*,
    resources::{self, *},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

mod error_code;

pub(super) trait ToolContent {
    fn create(&mut self, hwnd: HWND) -> anyhow::Result<()>;
    fn layout(&self);
    fn focus(&self);
    fn prepare(&self);
    fn recolor(&self);
    fn status(&self) -> String;
    fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize>;
    fn key(&self, _msg: &MSG) -> bool {
        false
    }
    fn destroy(&self) {}
}
pub(super) struct Descriptor {
    pub id: &'static str,
    pub title: u16,
    pub description: u16,
    pub icon: Icon,
    pub factory: fn() -> Box<dyn ToolContent>,
}
pub(super) static REGISTRY: &[Descriptor] = &[Descriptor {
    id: "error_code",
    title: IDS_ERROR_CODE_TOOL,
    description: IDS_ERROR_CODE_DESCRIPTION,
    icon: Icon::Constant,
    factory: error_code::create,
}];
pub(super) struct ToolView {
    pub id: u64,
    pub hwnd: HWND,
    pub descriptor: &'static Descriptor,
    pub content: Box<dyn ToolContent>,
    pub lifetime: Rc<()>,
    pub placement: Cell<Option<layout::Placement>>,
    recent: Cell<u64>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
}
impl ToolView {
    pub fn new(descriptor: &'static Descriptor) -> Self {
        Self {
            id: workspace::next_id(),
            hwnd: std::ptr::null_mut(),
            descriptor,
            content: (descriptor.factory)(),
            lifetime: Rc::new(()),
            placement: Cell::new(None),
            recent: Cell::new(0),
            icons: RefCell::new(None),
        }
    }
    pub fn title(&self) -> String {
        if self.hwnd.is_null() {
            resources::text(self.descriptor.title)
        } else {
            crate::native::text(self.hwnd)
        }
    }
    fn update_icons(&self) {
        let value = super::dpi().window(self.hwnd);
        if let Some(icons) = super::catalog::report(super::write(|a| {
            a.workspace.icons.get(self.descriptor.icon, value)
        })) {
            icons.apply(self.hwnd);
            self.icons.replace(Some(icons));
        }
    }
    pub fn prepare(&self) {
        self.update_icons();
        self.content.prepare();
    }
    pub fn recolor(&self) {
        self.update_icons();
        self.content.recolor();
    }
    pub fn activated(&self) {
        self.recent.set(CLOCK.with(|clock| {
            let next = clock.get() + 1;
            clock.set(next);
            next
        }));
    }
}
thread_local! {static CLOCK:Cell<u64>=const{Cell::new(0)};}
pub(super) fn activated(id: u64) {
    if let Some(Content::Tool(v)) = workspace::content_by_id(id) {
        v.activated();
    }
}
pub(super) fn open(index: isize, new: bool) {
    let Some(descriptor) = REGISTRY.get(index as usize) else {
        return;
    };
    history::visit(|| {
        let existing = (!new)
            .then(|| {
                workspace::contents()
                    .into_iter()
                    .filter_map(|c| match c {
                        Content::Tool(v) if v.descriptor.id == descriptor.id => Some(v),
                        _ => None,
                    })
                    .max_by_key(|v| v.recent.get())
            })
            .flatten();
        let hwnd = if let Some(view) = existing {
            workspace::show_document(view.hwnd);
            Some(view.hwnd)
        } else {
            super::catalog::report(workspace::create_tool(descriptor))
        };
        if let Some(content) = hwnd.and_then(workspace::content) {
            activated(content.id());
            content.focus();
        }
    });
}
pub(super) fn activate(index: isize, _double: bool) {
    open(index, false)
}
pub(super) fn selected(new: bool) {
    let list = unsafe { GetDlgItem(read(|a| a.frame), TOOL_LIST as i32) };
    open(
        send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize),
        new,
    );
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    if let Some(d) = REGISTRY.get(index) {
        let mut display = Display {
            title: resources::text(d.title).encode_utf16().collect(),
            icon: Some(d.icon),
            ..Default::default()
        };
        display
            .detail
            .push(TextSegment::plain(&resources::text(d.description)));
        display.tooltip = resources::text(d.description).encode_utf16().collect();
        visit(&display);
    }
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    REGISTRY
        .get(index)
        .map(|d| {
            format!(
                "{}\r\n{}",
                resources::text(d.title),
                resources::text(d.description)
            )
            .encode_utf16()
            .collect()
        })
        .unwrap_or_default()
}
pub(super) fn context_menu(hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_TOOL_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_TOOL_NEW), true)?;
    let point = if l == -1 {
        let b = bounds(hwnd, std::ptr::null_mut());
        POINT {
            x: b.left + 8,
            y: b.top + 8,
        }
    } else {
        POINT {
            x: l as i16 as i32,
            y: (l >> 16) as i16 as i32,
        }
    };
    match menu.track(read(|a| a.frame), point) {
        1 => selected(false),
        2 => selected(true),
        _ => {}
    }
    Ok(())
}

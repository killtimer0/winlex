//! Snapshot-bound declaration documents. Catalog changes update only the version hint.
use super::{bind, catalog, document, dpi, font_for, history, read, workspace, write};
use crate::{
    layout,
    native::*,
    resources::{self, *},
    tasks::Job,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};
use windows::Win32::UI::Controls::RichEdit::ENLINK;
use winlex_core::{Architecture, Catalog, EntityId};

mod find;
mod info;

#[derive(Clone)]
pub(super) struct Binding {
    pub id: EntityId,
    pub catalog: Arc<Catalog>,
    pub architecture: Architecture,
    pub architecture_mask: i32,
}
struct State {
    binding: Binding,
    name: String,
    ticket: Option<u64>,
    restore: Option<crate::rich_text::ViewState>,
    content: Option<Rc<crate::rich_text::Content>>,
    press: Option<LinkPress>,
    navigation: Option<Navigation>,
    context_link: Option<(Rc<crate::rich_text::Content>, std::ops::Range<usize>)>,
}
struct Navigation {
    content: Rc<crate::rich_text::Content>,
    binding: Binding,
    target: EntityId,
}
struct LinkPress {
    navigation: Navigation,
    range: std::ops::Range<usize>,
    origin: POINT,
}
impl State {
    fn contains(&self, navigation: &Navigation) -> bool {
        self.content
            .as_ref()
            .is_some_and(|content| Rc::ptr_eq(content, &navigation.content))
            && self.binding.id == navigation.binding.id
            && self.binding.architecture == navigation.binding.architecture
            && Arc::ptr_eq(&self.binding.catalog, &navigation.binding.catalog)
    }
}
pub(super) struct MetadataView {
    pub id: u64,
    pub hwnd: HWND,
    pub placement: Cell<Option<layout::Placement>>,
    pub preview: Cell<bool>,
    pub releasing_link: Cell<bool>,
    state: RefCell<State>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    editor: HWND,
    header: HWND,
    info: info::Info,
    find: find::Find,
}
pub(super) fn binding(hwnd: HWND) -> Option<Binding> {
    Some(workspace::view(hwnd)?.binding())
}
pub(super) fn find(binding: &Binding) -> Option<HWND> {
    workspace::views().into_iter().find_map(|content| {
        let state = content.state.borrow();
        let candidate = &state.binding;
        (candidate.id == binding.id
            && candidate.architecture == binding.architecture
            && Arc::ptr_eq(&candidate.catalog, &binding.catalog))
        .then_some(content.hwnd)
    })
}
pub(super) fn open(name: &str, binding: Binding, preview: bool) -> anyhow::Result<HWND> {
    let content = MetadataView {
        id: workspace::next_id(),
        hwnd: std::ptr::null_mut(),
        placement: Cell::new(None),
        preview: Cell::new(preview),
        releasing_link: Cell::new(false),
        icons: RefCell::new(None),
        state: RefCell::new(State {
            binding,
            name: name.into(),
            ticket: None,
            restore: None,
            content: None,
            press: None,
            navigation: None,
            context_link: None,
        }),
        editor: std::ptr::null_mut(),
        header: std::ptr::null_mut(),
        info: info::Info::default(),
        find: find::Find::default(),
    };
    let title = content.title();
    let content = workspace::create_document(&title, content)?;
    content.prepare();
    content.request();
    Ok(content.hwnd)
}
impl MetadataView {
    pub(super) fn is_live(&self) -> bool {
        read(|a| a.workspace.documents.iter().any(|doc| doc.id == self.id))
    }
    pub(super) fn binding(&self) -> Binding {
        self.state.borrow().binding.clone()
    }
    pub(super) fn rebind(&self, name: &str, binding: Binding) {
        {
            let mut state = self.state.borrow_mut();
            state.binding = binding;
            state.name = name.into();
            state.press = None;
            state.navigation = None;
            state.context_link = None;
        }
        self.fill_architectures();
        let binding = self.binding();
        if let Err(error) = self.info.fill(&binding) {
            write(|a| a.failure = Some(error));
        }
        if let Err(error) = self.update_icons() {
            write(|a| a.failure = Some(error));
        }
        self.request();
    }
    pub(super) fn title(&self) -> String {
        let state = self.state.borrow();
        format!("{} - {}", state.name, state.binding.architecture.name())
    }
    fn update_icons(&self) -> anyhow::Result<()> {
        let hwnd = self.hwnd;
        let icon = {
            let state = self.state.borrow();
            let entity = state.binding.catalog.entity(state.binding.id)?;
            crate::icons::Icon::for_kind(&entity.kind)
        };
        let value = dpi().window(hwnd);
        let icons = write(|a| a.workspace.icons.get(icon, value))?;
        if self
            .icons
            .borrow()
            .as_ref()
            .is_some_and(|old| Rc::ptr_eq(old, &icons))
        {
            return Ok(());
        }
        icons.apply(hwnd);
        self.icons.replace(Some(icons));
        if unsafe { IsZoomed(hwnd) } != 0 && workspace::active() == hwnd {
            unsafe { DrawMenuBar(read(|a| a.frame)) };
        }
        Ok(())
    }
    fn fill_architectures(&self) {
        let binding = self.state.borrow().binding.clone();
        send(self.info.architecture, CB_RESETCONTENT, 0, 0);
        for arch in super::architecture::supported(binding.architecture_mask) {
            let index = send(
                self.info.architecture,
                CB_ADDSTRING,
                0,
                wide(arch.name()).as_ptr() as isize,
            );
            if arch == binding.architecture {
                send(self.info.architecture, CB_SETCURSEL, index as usize, 0);
            }
        }
    }
    fn request(&self) {
        if !self.is_live() {
            return;
        }
        let ticket = write(|a| a.session.ticket());
        let (binding, previous) = {
            let mut state = self.state.borrow_mut();
            state.restore = None;
            let previous = state.ticket.replace(ticket.id);
            (state.binding.clone(), previous)
        };
        set_text(self.hwnd, &self.title());
        workspace::refresh_document_tabs();
        self.layout();
        let job = Job::Describe {
            document: self.id,
            catalog: binding.catalog,
            entity: binding.id,
            arch: binding.architecture,
        };
        let body = match read(|a| a.worker.replace(previous, ticket, job)) {
            Ok(()) => resources::text(IDS_QUERYING),
            Err(error) => {
                self.state.borrow_mut().ticket = None;
                format!("{error:#}")
            }
        };
        self.set_content(crate::rich_text::Content::new(body));
    }
    fn set_content(&self, content: crate::rich_text::Content) {
        let content = Rc::new(content);
        {
            let mut state = self.state.borrow_mut();
            state.press = None;
            state.navigation = None;
            state.context_link = None;
            state.content = Some(content.clone());
        }
        if let Err(error) = crate::rich_text::set_content(self.editor, &content) {
            catalog::set_status(format!("{error:#}"));
        }
        self.find.refresh(self.editor, false);
    }
    fn select_architecture(&self, code: u32) {
        let binding = self.state.borrow().binding.clone();
        let supported: Vec<_> = super::architecture::supported(binding.architecture_mask).collect();
        if code == CBN_SELENDCANCEL {
            if let Some(index) = supported
                .iter()
                .position(|arch| *arch == binding.architecture)
            {
                send(self.info.architecture, CB_SETCURSEL, index, 0);
            }
            return;
        }
        if code != CBN_SELENDOK {
            return;
        }
        let index = send(self.info.architecture, CB_GETCURSEL, 0, 0);
        let Some(&arch) = supported.get(index as usize) else {
            return;
        };
        if supported.len() > 1
            && let Err(error) = write(|a| a.settings.set_preferred_architecture(&a.config, arch))
        {
            catalog::set_status(format!("{error:#}"));
        }
        if arch != binding.architecture {
            history::visit(|| {
                self.state.borrow_mut().binding.architecture = arch;
                self.request();
            });
        }
    }
}
impl MetadataView {
    pub(super) fn position(&self) -> crate::rich_text::ViewState {
        let pending = self.state.borrow().restore;
        pending.unwrap_or_else(|| crate::rich_text::ViewState::capture(self.editor))
    }
    pub(super) fn restore_position(&self, position: crate::rich_text::ViewState) {
        let loading = {
            let mut state = self.state.borrow_mut();
            let loading = state.ticket.is_some();
            state.restore = loading.then_some(position);
            loading
        };
        if !loading {
            position.restore(self.editor);
        }
    }
    pub(super) fn cancel_restore(&self) {
        self.state.borrow_mut().restore = None;
    }
    pub(super) fn focus(&self) {
        focus(self.editor);
    }
    pub(super) fn cancel(&self) {
        if let Some(ticket) = &self.state.borrow().ticket {
            read(|a| a.worker.cancel(*ticket));
        }
    }
    pub(super) fn complete(&self, ticket: u64, result: anyhow::Result<crate::rich_text::Content>) {
        let restore = {
            let mut state = self.state.borrow_mut();
            if state.ticket != Some(ticket) {
                return;
            }
            state.ticket = None;
            state.restore.take().filter(|_| result.is_ok())
        };
        let content = match result {
            Ok(content) => content,
            Err(error) => crate::rich_text::Content::new(format!("{error:#}")),
        };
        self.set_content(content);
        if let Some(position) = restore {
            position.restore(self.editor);
        }
    }
    pub(super) fn worker_stopped(&self) {
        self.cancel_restore();
        let pending = self.state.borrow_mut().ticket.take().is_some();
        if pending {
            self.set_content(crate::rich_text::Content::new(resources::text(
                IDS_WORKER_STOPPED,
            )));
        }
    }
    pub(super) fn context_menu(&self, control: HWND, location: isize) -> bool {
        if control != self.editor {
            return false;
        }
        let (content, binding, mouse_link) = {
            let mut state = self.state.borrow_mut();
            (
                state.content.clone(),
                state.binding.clone(),
                state.context_link.take(),
            )
        };
        let regions = content.as_ref().map_or(&[][..], |c| c.regions.as_slice());
        let mouse_region = mouse_link.as_ref().and_then(|(clicked, range)| {
            content
                .as_ref()
                .filter(|current| Rc::ptr_eq(current, clicked))?;
            regions.iter().find(|region| &region.range == range)
        });
        match crate::rich_text::context_menu(self.editor, location, regions, mouse_region) {
            Ok(Some(target)) => self.queue_navigation(Navigation {
                content: content.expect("reference content"),
                binding,
                target,
            }),
            Ok(None) => {}
            Err(error) => catalog::set_status(format!("{error:#}")),
        }
        true
    }
    pub(super) fn cancel_link_press(&self) {
        self.state.borrow_mut().press = None;
    }
    pub(super) fn clear_context_link(&self) {
        self.state.borrow_mut().context_link = None;
    }
    pub(super) fn move_link_press(&self, point: POINT) {
        let mut state = self.state.borrow_mut();
        if state
            .press
            .as_ref()
            .is_some_and(|press| !click_distance(press.origin, point))
        {
            state.press = None;
        }
    }
    pub(super) fn link_notification(&self, link: ENLINK) {
        if link.nmhdr.hwndFrom.0 != self.editor
            || !matches!(link.msg, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP)
        {
            return;
        }
        let point = POINT {
            x: link.lParam.0 as i16 as i32,
            y: (link.lParam.0 >> 16) as i16 as i32,
        };
        if link.msg != WM_RBUTTONUP && !link_modifiers(link.wParam.0) {
            self.cancel_link_press();
            return;
        }
        let (content, binding) = {
            let state = self.state.borrow();
            (state.content.clone(), state.binding.clone())
        };
        let Some(content) = content else { return };
        let Some(region) = crate::rich_text::link_region(self.editor, &content, &link) else {
            return;
        };
        let winlex_format::declaration::RegionKind::Reference(target) = region.kind else {
            return;
        };
        if link.msg == WM_RBUTTONUP {
            let range = region.range.clone();
            self.state.borrow_mut().context_link = Some((content, range));
            return;
        }
        if link.msg == WM_LBUTTONDOWN {
            let range = region.range.clone();
            self.state.borrow_mut().press = Some(LinkPress {
                navigation: Navigation {
                    content,
                    binding,
                    target,
                },
                range,
                origin: point,
            });
        } else {
            let press = self.state.borrow_mut().press.take();
            if let Some(press) = press
                && self.releasing_link.get()
                && press.range == region.range
                && press.navigation.target == target
                && click_distance(press.origin, point)
            {
                self.queue_navigation(press.navigation);
            }
        }
    }
    fn queue_navigation(&self, navigation: Navigation) {
        if !self.is_live() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if !state.contains(&navigation) {
                return;
            }
            state.navigation = Some(navigation);
        }
        if unsafe {
            PostMessageW(
                self.hwnd,
                document::FOLLOW_REFERENCE,
                self.id as u32 as usize,
                (self.id >> 32) as isize,
            )
        } == 0
        {
            self.state.borrow_mut().navigation = None;
            catalog::set_status(format!("{:#}", error("Queue declaration navigation")));
        }
    }
    pub(super) fn follow_reference(&self) {
        let navigation = {
            let mut state = self.state.borrow_mut();
            state
                .navigation
                .take()
                .filter(|request| state.contains(request))
        };
        let Some(navigation) = navigation else { return };
        let entity = match navigation.binding.catalog.entity(navigation.target) {
            Ok(entity) => entity,
            Err(error) => {
                catalog::set_status(format!("{error:#}"));
                return;
            }
        };
        if !navigation
            .binding
            .architecture
            .supports(entity.architecture_mask)
        {
            catalog::set_status(resources::text(IDS_UNSUPPORTED_ARCH));
            return;
        }
        let binding = Binding {
            id: entity.id,
            architecture_mask: entity.architecture_mask,
            ..navigation.binding
        };
        history::visit(|| {
            if let Some(hwnd) = catalog::open_binding(&entity.name, binding, catalog::Open::Preview)
                && let Some(document) = workspace::view(hwnd)
            {
                document.focus();
            }
        });
    }
    pub(super) fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        self.editor = crate::rich_text::create(hwnd, DECLARATION)?;
        self.header = unsafe {
            child(
                hwnd,
                w!("STATIC"),
                resources::wide_text(IDS_OLD_VERSION),
                0,
                DOCUMENT_HEADER,
            )
        }?;
        document::track_editor(hwnd, self.editor)?;
        show(self.header, false);
        self.info.create(hwnd, self.editor)?;
        self.find.create(hwnd)?;
        use windows::Win32::UI::Controls::RichEdit::{
            EM_GETEVENTMASK, EM_SETEVENTMASK, ENM_SELCHANGE,
        };
        let mask = send(self.editor, EM_GETEVENTMASK, 0, 0);
        send(
            self.editor,
            EM_SETEVENTMASK,
            0,
            mask | ENM_SELCHANGE as isize,
        );
        self.fill_architectures();
        let binding = self.state.borrow().binding.clone();
        self.info.fill(&binding)?;
        self.update_icons()?;
        Ok(())
    }
    pub(super) fn prepare(&self) {
        let hwnd = self.hwnd;
        if let Err(error) = self.update_icons() {
            write(|a| a.failure = Some(error));
        }
        let (font, _) = font_for(dpi().window(hwnd));
        for h in [self.editor, self.header] {
            bind(h, font);
        }
        self.find.prepare(font);
        if let Err(error) = self.info.prepare(&super::metrics(hwnd)) {
            write(|a| a.failure = Some(error));
        }
        crate::rich_text::font(self.editor);
        self.layout();
    }
    pub(super) fn recolor(&self) {
        self.cancel_link_press();
        let content = self.state.borrow().content.clone();
        if let Some(content) = content
            && let Err(error) = crate::rich_text::recolor(self.editor, &content)
        {
            catalog::set_status(format!("{error:#}"));
        }
    }
    pub(super) fn layout(&self) {
        let hwnd = self.hwnd;
        let old = old_version(&self.state.borrow().binding);
        show(self.header, old);
        let m = super::metrics(hwnd);
        let mut area = layout::inset(client(hwnd), m.px(8));
        let mut positions = Vec::new();
        if old {
            positions.push((self.header, layout::take_top(&mut area, m.height + m.px(8))));
        }
        self.info.layout(&mut area, &m, &mut positions);
        self.find.layout(&mut area, &m, &mut positions);
        positions.push((self.editor, area));
        if let Err(error) = place_all(&positions) {
            write(|a| a.failure = Some(error));
        }
    }
    pub(super) fn command(&self, id: u16, code: u32) -> bool {
        if self.find.command(self.editor, id, code) {
            self.layout();
            return true;
        }
        match id {
            SELECT_ARCH => self.select_architecture(code),
            DOCUMENT_MORE if code == BN_CLICKED => {
                if let Err(error) = self.info.menu(self.hwnd) {
                    catalog::set_status(format!("{error:#}"));
                }
            }
            DOCUMENT_ONLINE if code == BN_CLICKED => {
                if let Err(error) = self.info.open_online(self.hwnd) {
                    catalog::set_status(format!("{error:#}"));
                }
            }
            _ => return false,
        }
        true
    }
    pub(super) fn color(&self, control: HWND, dc: HDC) -> Option<isize> {
        self.info.color(control, dc)
    }
    pub(super) fn destroy(&self) {
        self.info.clear_accessibility();
    }
    pub(super) fn find_key(&self, msg: &MSG) -> bool {
        self.find.key(self.editor, msg)
    }
    pub(super) fn selection_changed(&self) {
        self.find.count(self.editor, false);
    }
}
pub(super) fn link_modifiers(keys: usize) -> bool {
    use windows_sys::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON};
    keys & !(MK_LBUTTON as usize) == MK_CONTROL as usize
        && unsafe { GetKeyState(VK_MENU as i32) } >= 0
}
fn click_distance(origin: POINT, point: POINT) -> bool {
    (point.x - origin.x).abs() < unsafe { GetSystemMetrics(SM_CXDRAG) }.max(1)
        && (point.y - origin.y).abs() < unsafe { GetSystemMetrics(SM_CYDRAG) }.max(1)
}
fn old_version(binding: &Binding) -> bool {
    let path = &binding.catalog.sources()[binding.id.source_id].path;
    read(|a| {
        a.session.catalog.as_ref().is_some_and(|current| {
            !Arc::ptr_eq(current, &binding.catalog)
                && current.sources().iter().any(|s| &s.path == path)
        })
    })
}

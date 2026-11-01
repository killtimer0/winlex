//! Snapshot-bound declaration documents. Catalog changes update only the version hint.
use super::{bind, catalog, document, dpi, font_for, read, workspace, write};
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
use winlex_core::{Architecture, Catalog, EntityId};

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
    content: Option<Rc<crate::rich_text::Content>>,
}
pub(super) struct MetadataView {
    pub id: u64,
    pub hwnd: HWND,
    pub placement: Cell<Option<layout::Placement>>,
    pub preview: Cell<bool>,
    state: RefCell<State>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    editor: HWND,
    header: HWND,
    info: info::Info,
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
        icons: RefCell::new(None),
        state: RefCell::new(State {
            binding,
            name: name.into(),
            ticket: None,
            content: None,
        }),
        editor: std::ptr::null_mut(),
        header: std::ptr::null_mut(),
        info: info::Info::default(),
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
        self.state.borrow_mut().content = Some(content.clone());
        if let Err(error) = crate::rich_text::set_content(self.editor, &content) {
            catalog::set_status(format!("{error:#}"));
        }
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
            self.state.borrow_mut().binding.architecture = arch;
            self.request();
        }
    }
}
impl MetadataView {
    pub(super) fn focus(&self) {
        focus(self.editor);
    }
    pub(super) fn cancel(&self) {
        if let Some(ticket) = &self.state.borrow().ticket {
            read(|a| a.worker.cancel(*ticket));
        }
    }
    pub(super) fn complete(&self, ticket: u64, result: anyhow::Result<crate::rich_text::Content>) {
        {
            let mut state = self.state.borrow_mut();
            if state.ticket != Some(ticket) {
                return;
            }
            state.ticket = None;
        }
        let content = match result {
            Ok(content) => content,
            Err(error) => crate::rich_text::Content::new(format!("{error:#}")),
        };
        self.set_content(content);
    }
    pub(super) fn worker_stopped(&self) {
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
        let content = self.state.borrow().content.clone();
        let numbers = content.as_ref().map_or(&[][..], |c| c.numbers.as_slice());
        if let Err(error) = crate::rich_text::context_menu(self.editor, location, numbers) {
            catalog::set_status(format!("{error:#}"));
        }
        true
    }
    pub(super) fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        self.editor = crate::rich_text::create(hwnd, DECLARATION)?;
        self.header = child(
            hwnd,
            w!("STATIC"),
            resources::wide_text(IDS_OLD_VERSION),
            0,
            DOCUMENT_HEADER,
        )?;
        document::track_editor(hwnd, self.editor)?;
        show(self.header, false);
        self.info.create(hwnd, self.editor)?;
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
        if let Err(error) = self.info.prepare(&super::metrics(hwnd)) {
            write(|a| a.failure = Some(error));
        }
        crate::rich_text::font(self.editor);
        self.layout();
    }
    pub(super) fn recolor(&self) {
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
        positions.push((self.editor, area));
        if let Err(error) = place_all(&positions) {
            write(|a| a.failure = Some(error));
        }
    }
    pub(super) fn command(&self, id: u16, code: u32) -> bool {
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

//! Snapshot-bound declaration documents. Catalog changes update only the version hint.
use super::{bind, catalog, document, dpi, font_for, read, workspace, write};
use crate::{
    layout,
    native::*,
    resources::{self, *},
    tasks::{Job, Output},
};
use std::{cell::RefCell, rc::Rc, sync::Arc};
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
    numbers: Arc<[winlex_format::declaration::NumberRegion]>,
    old_version: bool,
}
pub(super) struct MetadataView {
    state: RefCell<State>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    editor: HWND,
    header: HWND,
    info: info::Info,
}
pub(super) fn binding(hwnd: HWND) -> Option<Binding> {
    Some(workspace::view(hwnd)?.state.borrow().binding.clone())
}
pub(super) fn find(binding: &Binding) -> Option<HWND> {
    workspace::views().into_iter().find_map(|(hwnd, content)| {
        let state = content.state.borrow();
        let candidate = &state.binding;
        (candidate.id == binding.id
            && candidate.architecture == binding.architecture
            && Arc::ptr_eq(&candidate.catalog, &binding.catalog))
        .then_some(hwnd)
    })
}
pub(super) fn open(name: &str, binding: Binding, preview: bool) -> anyhow::Result<HWND> {
    let title = format!("{name} - {}", binding.architecture.name());
    let hwnd = workspace::create_document(
        &title,
        MetadataView {
            icons: RefCell::new(None),
            state: RefCell::new(State {
                binding,
                name: name.into(),
                ticket: None,
                numbers: Arc::from([]),
                old_version: false,
            }),
            editor: std::ptr::null_mut(),
            header: std::ptr::null_mut(),
            info: info::Info::default(),
        },
        preview,
    )?;
    workspace::view(hwnd).unwrap().request(hwnd);
    Ok(hwnd)
}
pub(super) fn rebind(hwnd: HWND, name: &str, binding: Binding) -> bool {
    let Some(view) = workspace::view(hwnd) else {
        return false;
    };
    {
        let mut state = view.state.borrow_mut();
        state.binding = binding;
        state.name = name.into();
    }
    view.fill_architectures();
    let binding = view.state.borrow().binding.clone();
    if let Err(error) = view.info.fill(&binding) {
        write(|a| a.failure = Some(error));
    }
    if let Err(error) = view.update_icons(hwnd) {
        write(|a| a.failure = Some(error));
    }
    view.request(hwnd);
    true
}
impl MetadataView {
    fn update_icons(&self, hwnd: HWND) -> anyhow::Result<()> {
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
    fn request(&self, hwnd: HWND) {
        let Some(id) = workspace::document(hwnd, |d| d.id) else {
            return;
        };
        let ticket = write(|a| a.session.ticket());
        let (binding, name, previous) = {
            let mut state = self.state.borrow_mut();
            let previous = state.ticket.replace(ticket.id);
            state.numbers = Arc::from([]);
            (state.binding.clone(), state.name.clone(), previous)
        };
        let title = format!("{name} - {}", binding.architecture.name());
        workspace::edit_document(hwnd, |d| d.title = title.clone());
        set_text(hwnd, &title);
        workspace::refresh_document_tabs();
        self.state.borrow_mut().old_version = old_version(&binding);
        self.update_header(hwnd);
        let job = Job::Describe {
            document: id,
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
        crate::rich_text::set_content(self.editor, &body, true);
    }
    fn select_architecture(&self, hwnd: HWND, code: u32) {
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
            self.request(hwnd);
        }
    }
    fn update_header(&self, hwnd: HWND) {
        let old = self.state.borrow().old_version;
        show(self.header, old);
        self.layout(hwnd);
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
    pub(super) fn complete(&self, _hwnd: HWND, completion: crate::tasks::Completion) {
        {
            let mut state = self.state.borrow_mut();
            if state.ticket != Some(completion.id) {
                return;
            }
            state.ticket = None;
        }
        let body = match completion.result {
            Ok(Output::Description { body }) => {
                self.state.borrow_mut().numbers = body.numbers.into();
                body.text
            }
            Err(error) => format!("{error:#}"),
            _ => unreachable!(),
        };
        crate::rich_text::set_content(self.editor, &body, true);
    }
    pub(super) fn worker_stopped(&self, _hwnd: HWND) {
        let pending = self.state.borrow_mut().ticket.take().is_some();
        if pending {
            crate::rich_text::set_content(self.editor, &resources::text(IDS_WORKER_STOPPED), true);
        }
    }
    pub(super) fn catalog_changed(&self, hwnd: HWND) {
        let binding = self.state.borrow().binding.clone();
        let old = old_version(&binding);
        if self.state.borrow().old_version != old {
            self.state.borrow_mut().old_version = old;
            self.update_header(hwnd);
        }
    }
    pub(super) fn context_menu(&self, control: HWND, location: isize) -> bool {
        if control != self.editor {
            return false;
        }
        let numbers = self.state.borrow().numbers.clone();
        if let Err(error) = crate::rich_text::context_menu(self.editor, location, &numbers) {
            catalog::set_status(format!("{error:#}"));
        }
        true
    }
    pub(super) fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
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
        self.update_icons(hwnd)?;
        Ok(())
    }
    pub(super) fn prepare(&self, hwnd: HWND) {
        if let Err(error) = self.update_icons(hwnd) {
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
        self.layout(hwnd);
    }
    pub(super) fn recolor(&self) {
        crate::rich_text::recolor(self.editor, true);
    }
    pub(super) fn layout(&self, hwnd: HWND) {
        let old = self.state.borrow().old_version;
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
    pub(super) fn command(&self, hwnd: HWND, id: u16, code: u32) -> bool {
        match id {
            SELECT_ARCH => self.select_architecture(hwnd, code),
            DOCUMENT_MORE if code == BN_CLICKED => {
                if let Err(error) = self.info.menu(hwnd) {
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

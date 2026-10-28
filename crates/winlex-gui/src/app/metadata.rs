//! Metadata document content: entity snapshots and source inspection share a text view.
use super::document::View;
use super::{bind, catalog, document, dpi, font_for, read, sources, workspace, write};
use crate::tasks::Job;
use crate::{
    layout,
    native::*,
    resources::{self, *},
    worker::Ticket,
};
use std::{any::Any, cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};
use winlex_core::{Architecture, Catalog};

#[derive(Clone)]
pub(super) enum Binding {
    Entity {
        id: winlex_core::EntityId,
        catalog: Arc<Catalog>,
        architecture: Architecture,
        architecture_mask: i32,
    },
    Source {
        path: PathBuf,
        catalog: Option<Arc<Catalog>>,
    },
}
pub(super) struct State {
    pub binding: Binding,
    ticket: Option<Ticket>,
    content: Arc<str>,
    detail_content: Arc<str>,
    numbers: Arc<[winlex_format::declaration::NumberRegion]>,
    name: String,
    details_visible: bool,
    old_version: bool,
}
struct MetadataView {
    state: RefCell<State>,
    editor: HWND,
    header: HWND,
    details: HWND,
    architecture_label: HWND,
    architecture_select: HWND,
    actions: Vec<(u16, HWND)>,
}
pub(super) fn open(name: &str, binding: Binding, preview: bool) -> Result<HWND, String> {
    let title = match &binding {
        Binding::Entity { architecture, .. } => format!("{name} — {}", architecture.name()),
        Binding::Source { .. } => name.to_owned(),
    };
    let hwnd = workspace::create_document(
        &title,
        Box::new(MetadataView {
            state: RefCell::new(State {
                binding,
                name: name.to_owned(),
                ticket: None,
                content: "".into(),
                detail_content: "".into(),
                numbers: Arc::from([]),
                details_visible: false,
                old_version: false,
            }),
            editor: std::ptr::null_mut(),
            header: std::ptr::null_mut(),
            details: std::ptr::null_mut(),
            architecture_label: std::ptr::null_mut(),
            architecture_select: std::ptr::null_mut(),
            actions: vec![],
        }),
        preview,
    )?;
    request_document(hwnd);
    Ok(hwnd)
}
/// Repoint an entity document at another declaration without replacing its window.
pub(super) fn rebind(hwnd: HWND, name: &str, binding: Binding) -> bool {
    let Some(current) = view(hwnd) else {
        return false;
    };
    let current_binding = current.state.borrow().binding.clone();
    let (Binding::Entity { .. }, Binding::Entity { architecture, .. }) =
        (&current_binding, &binding)
    else {
        return false;
    };
    let title = format!("{name} — {}", architecture.name());
    {
        let mut d = current.state.borrow_mut();
        d.name = name.to_owned();
        d.binding = binding.clone();
    }
    fill_architectures(current.architecture_select, &binding);
    workspace::edit_document(hwnd, |d| d.title = title.clone());
    set_text(hwnd, &title);
    workspace::refresh_document_tabs();
    request_document(hwnd);
    true
}
impl MetadataView {
    fn apply(&self, hwnd: HWND, c: crate::tasks::Completion) {
        use crate::tasks::{Output, Target};
        if !self
            .state
            .borrow()
            .ticket
            .as_ref()
            .is_some_and(|t| t.id == c.id)
        {
            return;
        }
        self.state.borrow_mut().ticket = None;
        match c.result {
            Ok(Output::Reopened {
                catalog: snapshot,
                entity,
            }) => {
                let binding = self.state.borrow().binding.clone();
                if let Some(entity) = entity
                    && let Binding::Entity { architecture, .. } = binding
                {
                    if let Err(e) = open(
                        &entity.name,
                        Binding::Entity {
                            architecture,
                            id: entity.id,
                            catalog: snapshot,
                            architecture_mask: entity.architecture_mask,
                        },
                        false,
                    ) {
                        catalog::set_status(e);
                    }
                } else {
                    catalog::set_status(resources::text(IDS_CURRENT_UNAVAILABLE));
                }
                return;
            }
            Ok(Output::Description { body, details }) => {
                let mut d = self.state.borrow_mut();
                d.content = body.text.into();
                d.numbers = body.numbers.into();
                d.detail_content = details.into();
            }
            Ok(Output::Inspected(snapshot)) => {
                sources::inspected(hwnd, snapshot);
                return;
            }
            Err(e) => {
                if matches!(c.target, Target::Reopen(_)) {
                    catalog::set_status(e);
                    return;
                }
                let binding = self.state.borrow().binding.clone();
                if let Binding::Source { path, .. } = binding {
                    super::sources::inspection_failed(&path, &e);
                }
                {
                    let mut d = self.state.borrow_mut();
                    d.content = e.into();
                    d.numbers = Arc::from([]);
                }
            }
            _ => unreachable!(),
        }
        self.update_body();
    }
}
pub(super) fn get<T>(hwnd: HWND, inspect: impl FnOnce(u64, &State) -> T) -> Option<T> {
    let id = workspace::document(hwnd, |d| d.id)?;
    let view = view(hwnd)?;
    Some(inspect(id, &view.state.borrow()))
}
pub(super) fn edit(hwnd: HWND, f: impl FnOnce(&mut State)) {
    if let Some(view) = view(hwnd) {
        f(&mut view.state.borrow_mut());
    }
}
pub(super) fn find(predicate: impl Fn(u64, &State) -> bool) -> Option<HWND> {
    workspace::views().into_iter().find_map(|(hwnd, _)| {
        get(hwnd, |id, state| predicate(id, state))
            .filter(|found| *found)
            .map(|_| hwnd)
    })
}
fn fill_architectures(control: HWND, binding: &Binding) {
    let Binding::Entity {
        architecture,
        architecture_mask,
        ..
    } = binding
    else {
        return;
    };
    send(control, CB_RESETCONTENT, 0, 0);
    for arch in super::architecture::supported(*architecture_mask) {
        let index = send(
            control,
            CB_ADDSTRING,
            0,
            wide(arch.name()).as_ptr() as isize,
        );
        if arch == *architecture {
            send(control, CB_SETCURSEL, index as usize, 0);
        }
    }
}
impl document::View for MetadataView {
    fn focus(&self) {
        focus(self.editor);
    }
    fn complete(&self, hwnd: HWND, completion: crate::tasks::Completion) {
        self.apply(hwnd, completion);
    }
    fn context_menu(&self, control: HWND, location: isize) -> bool {
        if control != self.editor {
            return false;
        }
        let numbers = self.state.borrow().numbers.clone();
        if let Err(error) = crate::rich_text::context_menu(self.editor, location, &numbers) {
            catalog::set_status(error);
        }
        true
    }
    fn cancel(&self) {
        if let Some(ticket) = &self.state.borrow().ticket {
            read(|a| a.worker.cancel(ticket.id));
        }
    }
    fn create(&mut self, hwnd: HWND) -> Result<(), String> {
        self.editor = crate::rich_text::create(hwnd, DECLARATION)?;
        self.header = child(
            hwnd,
            w!("EDIT"),
            &[0],
            WS_TABSTOP | ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOHSCROLL as u32,
            DOCUMENT_HEADER,
        );
        self.details = child(
            hwnd,
            w!("EDIT"),
            &[0],
            WS_TABSTOP
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOHSCROLL as u32
                | ES_AUTOVSCROLL as u32,
            DOCUMENT_DETAILS,
        );
        document::track_editor(hwnd, self.editor)?;
        document::track_text(self.header)?;
        document::track_text(self.details)?;
        send(self.details, EM_SETLIMITTEXT, 0x7ffffffe, 0);
        show(self.details, false);
        let binding = self.state.borrow().binding.clone();
        if matches!(binding, Binding::Entity { .. }) {
            self.architecture_label = child(
                hwnd,
                w!("STATIC"),
                resources::wide_text(IDS_ARCHITECTURE),
                0,
                0,
            );
            self.architecture_select = child(
                hwnd,
                w!("COMBOBOX"),
                &[0],
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                SELECT_ARCH,
            );
            if self.architecture_label.is_null() || self.architecture_select.is_null() {
                return Err(error("Create document control"));
            }
            fill_architectures(self.architecture_select, &binding);
        }
        let commands: &[(u16, u16)] = match binding {
            Binding::Source { .. } => &[
                (LOAD_METADATA, IDS_LOAD_METADATA),
                (REMOVE_SOURCE, IDS_UNLOAD),
                (RELOAD_SOURCE, IDS_RELOAD),
                (REPLACE_SOURCE, IDS_REPLACE),
                (REFRESH_SOURCE, IDS_REFRESH),
            ],
            Binding::Entity { .. } => &[
                (TOGGLE_DETAILS, IDS_DETAILS),
                (OPEN_CURRENT, IDS_OPEN_CURRENT),
            ],
        };
        for &(id, label) in commands {
            let caption = resources::wide_text(label);
            let h = child(hwnd, w!("BUTTON"), caption, WS_TABSTOP, id);
            if h.is_null() {
                return Err(error("Create document control"));
            }
            self.actions.push((id, h));
        }
        Ok(())
    }
    fn prepare(&self, hwnd: HWND) {
        let (font, _) = font_for(dpi().window(hwnd));
        bind(self.editor, font);
        crate::rich_text::font(self.editor);
        let entity = matches!(self.state.borrow().binding, Binding::Entity { .. });
        crate::rich_text::recolor(self.editor, entity);
        for h in [
            self.header,
            self.details,
            self.architecture_label,
            self.architecture_select,
        ] {
            if !h.is_null() {
                bind(h, font);
            }
        }
        for &(_, h) in &self.actions {
            bind(h, font);
        }
        self.layout(hwnd);
    }
    fn layout(&self, hwnd: HWND) {
        let (entity, old_version, details_visible) = {
            let s = self.state.borrow();
            (
                matches!(s.binding, Binding::Entity { .. }),
                s.old_version,
                s.details_visible,
            )
        };
        let m = super::metrics(hwnd);
        let mut area = layout::inset(client(hwnd), m.px(8));
        place(
            self.header,
            layout::take_top(
                &mut area,
                m.height
                    * match entity {
                        false => 1,
                        true => {
                            if old_version {
                                4
                            } else {
                                3
                            }
                        }
                    }
                    + m.px(8),
            ),
        );
        if !self.architecture_select.is_null() {
            let mut row = layout::take_top(&mut area, m.row() + m.px(4));
            let width = m.label_width([resources::wide_text(IDS_ARCHITECTURE)].into_iter(), 70);
            place(self.architecture_label, layout::take_left(&mut row, width));
            place(
                self.architecture_select,
                layout::rect(
                    row.left,
                    row.top,
                    m.px(110).min(row.right - row.left),
                    m.row() * 6,
                ),
            );
        }
        m.buttons(
            &mut area,
            self.actions
                .iter()
                .filter_map(|&(id, h)| (id != OPEN_CURRENT || old_version).then_some(h)),
        );
        if details_visible {
            let height = ((area.bottom - area.top) / 3).min(m.row() * 8);
            place(self.details, layout::take_top(&mut area, height));
            layout::take_top(&mut area, m.px(4));
        }
        place(self.editor, area);
    }
    fn command(&self, hwnd: HWND, id: u16, code: u32) -> bool {
        let binding = self.state.borrow().binding.clone();
        match (&binding, id) {
            (Binding::Entity { .. }, SELECT_ARCH) => self.select_architecture(hwnd, code),
            (Binding::Entity { .. }, TOGGLE_DETAILS) => {
                let visible = {
                    let mut s = self.state.borrow_mut();
                    s.details_visible = !s.details_visible;
                    s.details_visible
                };
                show(self.details, visible);
                document::layout_document(hwnd);
            }
            (Binding::Entity { .. }, OPEN_CURRENT) => open_current(hwnd),
            (
                Binding::Source { .. },
                LOAD_METADATA | REMOVE_SOURCE | REPLACE_SOURCE | RELOAD_SOURCE | REFRESH_SOURCE,
            ) => sources::action(hwnd, id),
            _ => return false,
        }
        true
    }
    fn catalog_changed(&self, hwnd: HWND) {
        let binding = self.state.borrow().binding.clone();
        if let Binding::Source {
            path,
            catalog: Some(snapshot),
        } = &binding
        {
            let content = sources::details(path, snapshot);
            if self.state.borrow().content.as_ref() != content {
                self.state.borrow_mut().content = content.into();
                self.update_body();
            }
        } else {
            let old = old_version(&binding);
            if self.state.borrow().old_version != old {
                self.state.borrow_mut().old_version = old;
                self.update_header(hwnd);
            }
        }
    }
    fn worker_stopped(&self, _hwnd: HWND) {
        let pending = {
            let mut s = self.state.borrow_mut();
            let pending = s.ticket.take().is_some();
            if pending {
                s.content = resources::text(IDS_WORKER_STOPPED).into();
                s.numbers = Arc::from([]);
            }
            pending
        };
        if pending {
            self.update_body();
        }
    }
}
impl State {
    pub fn architecture(&self) -> Option<Architecture> {
        match &self.binding {
            Binding::Entity { architecture, .. } => Some(*architecture),
            Binding::Source { .. } => None,
        }
    }
}
impl MetadataView {
    fn select_architecture(&self, hwnd: HWND, code: u32) {
        let Binding::Entity {
            architecture,
            architecture_mask,
            ..
        } = self.state.borrow().binding.clone()
        else {
            return;
        };
        let supported: Vec<_> = super::architecture::supported(architecture_mask).collect();
        if code == CBN_SELENDCANCEL {
            if let Some(index) = supported
                .iter()
                .position(|candidate| *candidate == architecture)
            {
                send(self.architecture_select, CB_SETCURSEL, index, 0);
            }
            return;
        }
        if code != CBN_SELENDOK {
            return;
        }
        let index = send(self.architecture_select, CB_GETCURSEL, 0, 0);
        if index < 0 {
            return;
        }
        let Some(&arch) = supported.get(index as usize) else {
            return;
        };
        if supported.len() > 1
            && let Err(error) = write(|a| a.settings.set_preferred_architecture(&a.config, arch))
        {
            catalog::set_status(error);
        }
        if arch == architecture {
            return;
        }
        {
            if let Binding::Entity { architecture, .. } = &mut self.state.borrow_mut().binding {
                *architecture = arch;
            }
        }
        let title = format!("{} — {}", self.state.borrow().name, arch.name());
        workspace::edit_document(hwnd, |d| d.title = title.clone());
        set_text(hwnd, &title);
        workspace::refresh_document_tabs();
        request_document(hwnd);
    }
}
impl MetadataView {
    fn update_header(&self, hwnd: HWND) {
        let (binding, name, old) = {
            let s = self.state.borrow();
            (s.binding.clone(), s.name.clone(), s.old_version)
        };
        let header = match &binding {
            Binding::Source { .. } => name.clone(),
            Binding::Entity { id, catalog, .. } => {
                let source = &catalog.sources()[id.source_id];
                let entity = match catalog.entity(*id) {
                    Ok(entity) => entity,
                    Err(error) => {
                        catalog::set_status(error.to_string());
                        return;
                    }
                };
                format!(
                    "{}\r\n{} · {}\r\n{}",
                    name,
                    entity.kind,
                    entity.full_name,
                    source
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                )
            }
        };
        let prefix = if old {
            format!("{}\r\n", resources::text(IDS_OLD_VERSION))
        } else {
            String::new()
        };
        set_text(self.header, &format!("{prefix}{header}"));
        for &(id, h) in &self.actions {
            if id == OPEN_CURRENT {
                show(h, old);
            }
        }
        self.layout(hwnd);
    }
    fn update_body(&self) {
        let (binding, content, details) = {
            let s = self.state.borrow();
            (
                s.binding.clone(),
                s.content.clone(),
                s.detail_content.clone(),
            )
        };
        if let Binding::Entity { id, catalog, .. } = &binding {
            let details = format!(
                "{}\r\n{}\r\n\r\n{}",
                catalog.sources()[id.source_id].path.display(),
                id,
                details.replace('\n', "\r\n")
            );
            if crate::native::text(self.details) != details {
                set_text(self.details, &details);
            }
        }
        crate::rich_text::set_content(
            self.editor,
            &content,
            matches!(binding, Binding::Entity { .. }),
        );
    }
}
fn binding_changed(hwnd: HWND) {
    if let Some(view) = view(hwnd) {
        let binding = view.state.borrow().binding.clone();
        view.state.borrow_mut().old_version = old_version(&binding);
        view.update_header(hwnd);
        view.update_body();
    }
}
fn view(hwnd: HWND) -> Option<Rc<MetadataView>> {
    let view: Rc<dyn Any> = workspace::view(hwnd)?;
    view.downcast().ok()
}
fn old_version(binding: &Binding) -> bool {
    let Binding::Entity { id, catalog, .. } = binding else {
        return false;
    };
    let source = &catalog.sources()[id.source_id];
    read(|a| {
        a.session.catalog.as_ref().is_some_and(|current| {
            !Arc::ptr_eq(current, catalog)
                && current.sources().iter().any(|s| s.path == source.path)
        })
    })
}
fn open_current(hwnd: HWND) {
    let Some((
        document,
        Binding::Entity {
            id,
            catalog: old,
            architecture,
            ..
        },
        pending,
    )) = get(hwnd, |id, d| (id, d.binding.clone(), d.ticket.is_some()))
    else {
        return;
    };
    let current = read(|a| {
        let old_source = &old.sources()[id.source_id];
        let current = a.session.catalog.as_ref()?;
        let mut sources = current
            .sources()
            .iter()
            .filter(|s| s.path == old_source.path);
        let source = sources.next()?.id;
        if sources.next().is_some() {
            return None;
        }
        Some((current.clone(), source))
    });
    let Some((current, source)) = current else {
        catalog::set_status(resources::text(IDS_CURRENT_UNAVAILABLE));
        return;
    };
    if pending {
        return;
    }
    workspace::pin_document(hwnd);
    let ticket = write(|a| a.session.ticket());
    let job = Job::Reopen {
        document,
        previous: old,
        current,
        source,
        entity: id,
        arch: architecture,
    };
    if let Err(e) = read(|a| a.worker.replace(None, ticket.clone(), job)) {
        catalog::set_status(e);
    } else {
        edit(hwnd, |d| d.ticket = Some(ticket));
    }
}
pub(super) fn request_document(hwnd: HWND) {
    let Some((document, binding, previous)) =
        get(hwnd, |id, d| (id, d.binding.clone(), d.ticket.clone()))
    else {
        return;
    };
    let ticket = write(|a| a.session.ticket());
    let job = match binding {
        Binding::Source { path, .. } => Job::Inspect { document, path },
        Binding::Entity {
            id,
            catalog,
            architecture,
            ..
        } => Job::Describe {
            document,
            catalog,
            entity: id,
            arch: architecture,
        },
    };
    edit(hwnd, |d| {
        d.ticket = Some(ticket.clone());
        d.content = resources::text(IDS_QUERYING).into();
        d.detail_content = "".into();
        d.numbers = Arc::from([]);
    });
    if let Err(e) = read(|a| a.worker.replace(previous.map(|t| t.id), ticket, job)) {
        edit(hwnd, |d| {
            d.ticket = None;
            d.content = e.into();
        });
    }
    binding_changed(hwnd);
}

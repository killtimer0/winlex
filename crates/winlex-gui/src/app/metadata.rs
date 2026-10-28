//! Snapshot-bound declaration documents. Catalog changes update only the version hint.
use super::document::View;
use super::{bind, catalog, document, dpi, font_for, read, workspace, write};
use crate::{
    layout,
    native::*,
    resources::{self, *},
    tasks::{Job, Output},
    worker::Ticket,
};
use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};
use winlex_core::{Architecture, Catalog, EntityId};

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
    ticket: Option<Ticket>,
    numbers: Arc<[winlex_format::declaration::NumberRegion]>,
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
    details_button: HWND,
}
fn view(hwnd: HWND) -> Option<Rc<MetadataView>> {
    let view: Rc<dyn Any> = workspace::view(hwnd)?;
    view.downcast().ok()
}
pub(super) fn binding(hwnd: HWND) -> Option<Binding> {
    Some(view(hwnd)?.state.borrow().binding.clone())
}
pub(super) fn find(binding: &Binding) -> Option<HWND> {
    workspace::views().into_iter().find_map(|(hwnd, _)| {
        let candidate = self::binding(hwnd)?;
        (candidate.id == binding.id
            && candidate.architecture == binding.architecture
            && Arc::ptr_eq(&candidate.catalog, &binding.catalog))
        .then_some(hwnd)
    })
}
pub(super) fn open(name: &str, binding: Binding, preview: bool) -> Result<HWND, String> {
    let title = format!("{name} — {}", binding.architecture.name());
    let hwnd = workspace::create_document(
        &title,
        Box::new(MetadataView {
            state: RefCell::new(State {
                binding,
                name: name.into(),
                ticket: None,
                numbers: Arc::from([]),
                details_visible: false,
                old_version: false,
            }),
            editor: std::ptr::null_mut(),
            header: std::ptr::null_mut(),
            details: std::ptr::null_mut(),
            architecture_label: std::ptr::null_mut(),
            architecture_select: std::ptr::null_mut(),
            details_button: std::ptr::null_mut(),
        }),
        preview,
    )?;
    view(hwnd).unwrap().request(hwnd);
    Ok(hwnd)
}
pub(super) fn rebind(hwnd: HWND, name: &str, binding: Binding) -> bool {
    let Some(view) = view(hwnd) else {
        return false;
    };
    {
        let mut state = view.state.borrow_mut();
        state.binding = binding;
        state.name = name.into();
    }
    view.fill_architectures();
    view.request(hwnd);
    true
}
impl MetadataView {
    fn fill_architectures(&self) {
        let binding = self.state.borrow().binding.clone();
        send(self.architecture_select, CB_RESETCONTENT, 0, 0);
        for arch in super::architecture::supported(binding.architecture_mask) {
            let index = send(
                self.architecture_select,
                CB_ADDSTRING,
                0,
                wide(arch.name()).as_ptr() as isize,
            );
            if arch == binding.architecture {
                send(self.architecture_select, CB_SETCURSEL, index as usize, 0);
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
            let previous = state.ticket.replace(ticket.clone());
            state.numbers = Arc::from([]);
            (state.binding.clone(), state.name.clone(), previous)
        };
        let title = format!("{name} — {}", binding.architecture.name());
        workspace::edit_document(hwnd, |d| d.title = title.clone());
        set_text(hwnd, &title);
        workspace::refresh_document_tabs();
        self.state.borrow_mut().old_version = old_version(&binding);
        self.update_header(hwnd);
        self.set_details(&binding, "");
        let job = Job::Describe {
            document: id,
            catalog: binding.catalog,
            entity: binding.id,
            arch: binding.architecture,
        };
        let body = match read(|a| a.worker.replace(previous.map(|t| t.id), ticket, job)) {
            Ok(()) => resources::text(IDS_QUERYING),
            Err(error) => {
                self.state.borrow_mut().ticket = None;
                error
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
                send(self.architecture_select, CB_SETCURSEL, index, 0);
            }
            return;
        }
        if code != CBN_SELENDOK {
            return;
        }
        let index = send(self.architecture_select, CB_GETCURSEL, 0, 0);
        let Some(&arch) = supported.get(index as usize) else {
            return;
        };
        if supported.len() > 1
            && let Err(error) = write(|a| a.settings.set_preferred_architecture(&a.config, arch))
        {
            catalog::set_status(error);
        }
        if arch != binding.architecture {
            self.state.borrow_mut().binding.architecture = arch;
            self.request(hwnd);
        }
    }
    fn set_details(&self, binding: &Binding, details: &str) {
        set_text(
            self.details,
            &format!(
                "{}\r\n{}\r\n\r\n{}",
                binding.catalog.sources()[binding.id.source_id]
                    .path
                    .display(),
                binding.id,
                details.replace('\n', "\r\n")
            ),
        );
    }
    fn update_header(&self, hwnd: HWND) {
        let (binding, name, old) = {
            let state = self.state.borrow();
            (state.binding.clone(), state.name.clone(), state.old_version)
        };
        let entity = match binding.catalog.entity(binding.id) {
            Ok(entity) => entity,
            Err(error) => {
                catalog::set_status(error.to_string());
                return;
            }
        };
        let prefix = if old {
            format!("{}\r\n", resources::text(IDS_OLD_VERSION))
        } else {
            String::new()
        };
        set_text(
            self.header,
            &format!(
                "{prefix}{name}\r\n{} · {}\r\n{}",
                entity.kind,
                entity.full_name,
                binding.catalog.sources()[binding.id.source_id]
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
        );
        self.layout(hwnd);
    }
}
impl document::View for MetadataView {
    fn focus(&self) {
        focus(self.editor);
    }
    fn cancel(&self) {
        if let Some(ticket) = &self.state.borrow().ticket {
            read(|a| a.worker.cancel(ticket.id));
        }
    }
    fn complete(&self, _hwnd: HWND, completion: crate::tasks::Completion) {
        {
            let mut state = self.state.borrow_mut();
            if !state.ticket.as_ref().is_some_and(|t| t.id == completion.id) {
                return;
            }
            state.ticket = None;
        }
        let body = match completion.result {
            Ok(Output::Description { body, details }) => {
                self.state.borrow_mut().numbers = body.numbers.into();
                let binding = self.state.borrow().binding.clone();
                self.set_details(&binding, &details);
                body.text
            }
            Err(error) => error,
            _ => unreachable!(),
        };
        crate::rich_text::set_content(self.editor, &body, true);
    }
    fn worker_stopped(&self, _hwnd: HWND) {
        let pending = self.state.borrow_mut().ticket.take().is_some();
        if pending {
            crate::rich_text::set_content(self.editor, &resources::text(IDS_WORKER_STOPPED), true);
        }
    }
    fn catalog_changed(&self, hwnd: HWND) {
        let binding = self.state.borrow().binding.clone();
        let old = old_version(&binding);
        if self.state.borrow().old_version != old {
            self.state.borrow_mut().old_version = old;
            self.update_header(hwnd);
        }
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
        self.details_button = child(
            hwnd,
            w!("BUTTON"),
            resources::wide_text(IDS_DETAILS),
            WS_TABSTOP,
            TOGGLE_DETAILS,
        );
        if [
            self.architecture_label,
            self.architecture_select,
            self.details_button,
        ]
        .iter()
        .any(|h| h.is_null())
        {
            return Err(error("Create document control"));
        }
        self.fill_architectures();
        Ok(())
    }
    fn prepare(&self, hwnd: HWND) {
        let (font, _) = font_for(dpi().window(hwnd));
        for h in [
            self.editor,
            self.header,
            self.details,
            self.architecture_label,
            self.architecture_select,
            self.details_button,
        ] {
            bind(h, font);
        }
        crate::rich_text::font(self.editor);
        crate::rich_text::recolor(self.editor, true);
        self.layout(hwnd);
    }
    fn layout(&self, hwnd: HWND) {
        let (old, details) = {
            let state = self.state.borrow();
            (state.old_version, state.details_visible)
        };
        let m = super::metrics(hwnd);
        let mut area = layout::inset(client(hwnd), m.px(8));
        place(
            self.header,
            layout::take_top(&mut area, m.height * if old { 4 } else { 3 } + m.px(8)),
        );
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
        m.buttons(&mut area, [self.details_button]);
        if details {
            let height = ((area.bottom - area.top) / 3).min(m.row() * 8);
            place(self.details, layout::take_top(&mut area, height));
            layout::take_top(&mut area, m.px(4));
        }
        place(self.editor, area);
    }
    fn command(&self, hwnd: HWND, id: u16, code: u32) -> bool {
        match id {
            SELECT_ARCH => self.select_architecture(hwnd, code),
            TOGGLE_DETAILS => {
                let visible = {
                    let mut state = self.state.borrow_mut();
                    state.details_visible = !state.details_visible;
                    state.details_visible
                };
                show(self.details, visible);
                self.layout(hwnd);
            }
            _ => return false,
        }
        true
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

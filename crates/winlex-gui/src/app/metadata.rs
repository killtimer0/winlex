//! Metadata document content: entity snapshots and source inspection share a text view.
use super::{
    bind, catalog,
    document::{self, Content},
    dpi, font_for, read, sources, workspace, write,
};
use crate::{
    layout,
    native::*,
    resources::{self, *},
    worker::Ticket,
};
use std::{path::PathBuf, sync::Arc};
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
#[derive(Clone)]
pub(super) struct Document {
    pub binding: Binding,
    pub ticket: Option<Ticket>,
    pub content: Arc<str>,
    pub detail_content: Arc<str>,
    pub numbers: Arc<[winlex_format::declaration::NumberRegion]>,
    pub name: String,
    pub editor: HWND,
    header: HWND,
    details: HWND,
    architecture_label: HWND,
    architecture_select: HWND,
    details_visible: bool,
    actions: Vec<(u16, HWND)>,
    old_version: bool,
}
pub(super) fn open(name: &str, binding: Binding) -> Result<HWND, String> {
    let title = match &binding {
        Binding::Entity { architecture, .. } => format!("{name} — {}", architecture.name()),
        Binding::Source { .. } => name.to_owned(),
    };
    let hwnd = workspace::create_document(
        &title,
        Content::Metadata(Document {
            binding,
            name: name.to_owned(),
            ticket: None,
            content: "".into(),
            detail_content: "".into(),
            numbers: Arc::from([]),
            editor: std::ptr::null_mut(),
            header: std::ptr::null_mut(),
            details: std::ptr::null_mut(),
            architecture_label: std::ptr::null_mut(),
            architecture_select: std::ptr::null_mut(),
            details_visible: false,
            actions: vec![],
            old_version: false,
        }),
    )?;
    refresh_content(hwnd);
    catalog::request_document(hwnd);
    Ok(hwnd)
}
pub(super) fn apply(id: u64, c: crate::tasks::Completion) {
    use crate::tasks::{Output, Target};
    let Some(hwnd) =
        find(|document, d| document == id && d.ticket.as_ref().is_some_and(|t| t.id == c.id))
    else {
        return;
    };
    edit(hwnd, |d| d.ticket = None);
    match c.result {
        Ok(Output::Reopened {
            catalog: snapshot,
            entity,
        }) => {
            if let Some(entity) = entity
                && let Some(Binding::Entity { architecture, .. }) =
                    get(hwnd, |_, d| d.binding.clone())
            {
                if let Err(e) = open(
                    &entity.name,
                    Binding::Entity {
                        architecture,
                        id: entity.id,
                        catalog: snapshot,
                        architecture_mask: entity.architecture_mask,
                    },
                ) {
                    catalog::set_status(e);
                }
            } else {
                catalog::set_status(resources::text(IDS_CURRENT_UNAVAILABLE));
            }
        }
        Ok(Output::Description { body, details }) => edit(hwnd, |d| {
            d.content = body.text.into();
            d.numbers = body.numbers.into();
            d.detail_content = details.into();
        }),
        Ok(Output::Inspected(snapshot)) => super::sources::inspected(hwnd, snapshot),
        Err(e) => {
            if matches!(c.target, Target::Reopen(_)) {
                catalog::set_status(e);
                return;
            }
            if let Some(Binding::Source { path, .. }) = get(hwnd, |_, d| d.binding.clone()) {
                super::sources::inspection_failed(&path, &e);
            }
            edit(hwnd, |d| {
                d.content = e.into();
                d.numbers = Arc::from([]);
            });
        }
        _ => unreachable!(),
    }
    refresh_content(hwnd);
}
pub(super) fn get<T>(hwnd: HWND, inspect: impl FnOnce(u64, &Document) -> T) -> Option<T> {
    workspace::document(hwnd, |d| match &d.content {
        Content::Metadata(m) => inspect(d.id, m),
    })
}
pub(super) fn edit(hwnd: HWND, f: impl FnOnce(&mut Document)) {
    workspace::edit_document(hwnd, |d| match &mut d.content {
        Content::Metadata(m) => f(m),
    });
}
pub(super) fn find(predicate: impl Fn(u64, &Document) -> bool) -> Option<HWND> {
    read(|a| {
        a.workspace.documents.iter().find_map(|d| match &d.content {
            Content::Metadata(m) => predicate(d.id, m).then_some(d.hwnd),
        })
    })
}
impl Document {
    pub fn cancel(&self) {
        if let Some(ticket) = &self.ticket {
            read(|a| a.worker.cancel(ticket.id));
        }
    }
    pub fn create(&mut self, hwnd: HWND) -> Result<(), String> {
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
        document::track_focus(hwnd, self.editor)?;
        document::track_focus(hwnd, self.header)?;
        document::track_focus(hwnd, self.details)?;
        send(self.details, EM_SETLIMITTEXT, 0x7ffffffe, 0);
        show(self.details, false);
        if let Binding::Entity {
            architecture,
            architecture_mask,
            ..
        } = self.binding
        {
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
            document::track_focus(hwnd, self.architecture_select)?;
            for arch in super::architecture::supported(architecture_mask) {
                let index = send(
                    self.architecture_select,
                    CB_ADDSTRING,
                    0,
                    wide(arch.name()).as_ptr() as isize,
                );
                if arch == architecture {
                    send(self.architecture_select, CB_SETCURSEL, index as usize, 0);
                }
            }
        }
        let commands: &[(u16, u16)] = match self.binding {
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
            document::track_focus(hwnd, h)?;
            self.actions.push((id, h));
        }
        Ok(())
    }
    pub fn prepare(&self, hwnd: HWND) {
        let (font, _) = font_for(dpi().window(hwnd));
        bind(self.editor, font);
        crate::rich_text::font(self.editor);
        crate::rich_text::recolor(self.editor, matches!(self.binding, Binding::Entity { .. }));
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
    }
    pub fn layout(&self, hwnd: HWND) {
        let m = super::metrics(hwnd);
        let mut area = layout::inset(client(hwnd), m.px(8));
        place(
            self.header,
            layout::take_top(
                &mut area,
                m.height
                    * match self.binding {
                        Binding::Source { .. } => 1,
                        Binding::Entity { .. } => {
                            if self.old_version {
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
                .filter_map(|&(id, h)| (id != OPEN_CURRENT || self.old_version).then_some(h)),
        );
        if self.details_visible {
            let height = ((area.bottom - area.top) / 3).min(m.row() * 8);
            place(self.details, layout::take_top(&mut area, height));
            layout::take_top(&mut area, m.px(4));
        }
        place(self.editor, area);
    }
    pub fn command(&self, hwnd: HWND, id: u16, code: u32) -> bool {
        match (&self.binding, id) {
            (Binding::Entity { .. }, SELECT_ARCH) => self.select_architecture(hwnd, code),
            (Binding::Entity { .. }, TOGGLE_DETAILS) => {
                edit(hwnd, |d| d.details_visible = !d.details_visible);
                show(self.details, !self.details_visible);
                document::layout_document(hwnd);
            }
            (Binding::Entity { .. }, OPEN_CURRENT) => catalog::open_current(hwnd),
            (
                Binding::Source { .. },
                LOAD_METADATA | REMOVE_SOURCE | REPLACE_SOURCE | RELOAD_SOURCE | REFRESH_SOURCE,
            ) => sources::action(hwnd, id),
            _ => return false,
        }
        true
    }
    fn select_architecture(&self, hwnd: HWND, code: u32) {
        let Binding::Entity {
            architecture,
            architecture_mask,
            ..
        } = &self.binding
        else {
            return;
        };
        let supported: Vec<_> = super::architecture::supported(*architecture_mask).collect();
        if code == CBN_SELENDCANCEL {
            if let Some(index) = supported
                .iter()
                .position(|candidate| candidate == architecture)
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
        if arch == *architecture {
            return;
        }
        self.cancel();
        edit(hwnd, |d| {
            d.ticket = None;
            d.content = resources::text(IDS_QUERYING).into();
            d.detail_content = "".into();
            d.numbers = Arc::from([]);
            if let Binding::Entity { architecture, .. } = &mut d.binding {
                *architecture = arch;
            }
        });
        let title = format!("{} — {}", self.name, arch.name());
        workspace::edit_document(hwnd, |d| d.title = title.clone());
        set_text(hwnd, &title);
        workspace::refresh_document_tabs();
        refresh_content(hwnd);
        catalog::request_document(hwnd);
    }
}
pub(super) fn refresh_content(hwnd: HWND) {
    let Some(mut doc) = get(hwnd, |_, d| d.clone()) else {
        return;
    };
    let old = catalog::old_version(&doc.binding);
    edit(hwnd, |d| d.old_version = old);
    doc.old_version = old;
    let header = match &doc.binding {
        Binding::Source { .. } => doc.name.clone(),
        Binding::Entity { id, catalog, .. } => {
            let source = &catalog.sources()[id.source_id];
            let details = format!(
                "{}\r\n{}\r\n\r\n{}",
                source.path.display(),
                id,
                doc.detail_content.replace('\n', "\r\n")
            );
            if crate::native::text(doc.details) != details {
                set_text(doc.details, &details);
            }
            let entity = catalog.entity(*id).ok();
            format!(
                "{}\r\n{}\r\n{}",
                doc.name,
                entity
                    .map(|e| format!("{} · {}", e.kind, e.full_name))
                    .unwrap_or_default(),
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
    set_text(doc.header, &format!("{prefix}{header}"));
    crate::rich_text::set_content(
        doc.editor,
        &doc.content,
        matches!(doc.binding, Binding::Entity { .. }),
    );
    for &(id, h) in &doc.actions {
        if id == OPEN_CURRENT {
            show(h, old);
        }
    }
    doc.layout(hwnd);
}
pub(super) fn refresh_all() {
    let docs = read(|a| {
        a.workspace
            .documents
            .iter()
            .map(|d| d.hwnd)
            .collect::<Vec<_>>()
    });
    for h in docs {
        let Some(doc) = get(h, |_, d| d.clone()) else {
            continue;
        };
        if let Binding::Source {
            path,
            catalog: Some(snapshot),
        } = &doc.binding
        {
            let content = sources::details(path, snapshot);
            if doc.content.as_ref() == content {
                continue;
            }
            edit(h, |d| d.content = content.into());
        } else if doc.old_version == catalog::old_version(&doc.binding) {
            continue;
        }
        refresh_content(h);
    }
}
pub(super) fn worker_stopped() {
    let docs = write(|a| {
        a.workspace
            .documents
            .iter_mut()
            .filter_map(|d| match &mut d.content {
                Content::Metadata(m) => m.ticket.take().map(|_| {
                    m.content = "Catalog worker stopped".into();
                    m.numbers = Arc::from([]);
                    d.hwnd
                }),
            })
            .collect::<Vec<_>>()
    });
    for h in docs {
        refresh_content(h);
    }
}

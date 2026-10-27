//! Metadata document content: entity snapshots and source inspection share a text view.
use super::{
    Architecture, bind, catalog,
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
use winlex_core::Catalog;

#[derive(Clone)]
pub(super) enum Binding {
    Entity {
        id: String,
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
    pub name: String,
    pub editor: HWND,
    actions: Vec<(u16, HWND)>,
    old_version: bool,
}
pub(super) fn open(name: &str, binding: Binding) -> Result<HWND, String> {
    let title = match &binding {
        Binding::Entity { architecture, .. } => format!("{name} — {}", architecture.label()),
        Binding::Source { .. } => name.to_owned(),
    };
    let hwnd = workspace::create_document(
        &title,
        Content::Metadata(Document {
            binding,
            name: name.to_owned(),
            ticket: None,
            content: "".into(),
            editor: std::ptr::null_mut(),
            actions: vec![],
            old_version: false,
        }),
    )?;
    catalog::request_document(hwnd);
    Ok(hwnd)
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
            read(|a| a.session.worker.cancel(ticket));
        }
    }
    pub fn create(&mut self, hwnd: HWND) -> Result<(), String> {
        self.editor = child(
            hwnd,
            "EDIT",
            "",
            WS_TABSTOP
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            DECLARATION,
        );
        document::track_focus(hwnd, self.editor)?;
        send(self.editor, EM_SETLIMITTEXT, 0x7ffffffe, 0);
        let commands: &[(u16, u16)] = match self.binding {
            Binding::Source { .. } => &[
                (LOAD_METADATA, IDS_LOAD_METADATA),
                (REMOVE_SOURCE, IDS_UNLOAD),
                (RELOAD_SOURCE, IDS_RELOAD),
                (REPLACE_SOURCE, IDS_REPLACE),
                (REFRESH_SOURCE, IDS_REFRESH),
            ],
            Binding::Entity { .. } => &[
                (OPEN_OTHER_ARCH, IDS_OPEN_OTHER_ARCH),
                (OPEN_CURRENT, IDS_OPEN_CURRENT),
            ],
        };
        for &(id, label) in commands {
            let mut caption = resources::text(label);
            if let Binding::Entity { architecture, .. } = self.binding {
                caption = caption.replace("{arch}", &architecture.other().label());
            }
            let h = child(hwnd, "BUTTON", &caption, WS_TABSTOP, id);
            document::track_focus(hwnd, h)?;
            if id == OPEN_OTHER_ARCH
                && let Binding::Entity {
                    architecture,
                    architecture_mask,
                    ..
                } = self.binding
            {
                unsafe {
                    EnableWindow(
                        h,
                        i32::from(architecture.other().supported_by(architecture_mask)),
                    );
                }
            }
            self.actions.push((id, h));
        }
        Ok(())
    }
    pub fn prepare(&self, hwnd: HWND) {
        let (font, _) = font_for(dpi().window(hwnd));
        bind(self.editor, font);
        for &(_, h) in &self.actions {
            bind(h, font);
        }
    }
    pub fn layout(&self, hwnd: HWND) {
        let m = super::metrics(hwnd);
        let mut area = layout::inset(client(hwnd), m.px(8));
        m.buttons(
            &mut area,
            self.actions
                .iter()
                .filter_map(|&(id, h)| (id != OPEN_CURRENT || self.old_version).then_some(h)),
        );
        place(self.editor, area);
    }
    pub fn command(&self, hwnd: HWND, id: u16) -> bool {
        match (&self.binding, id) {
            (Binding::Entity { .. }, OPEN_OTHER_ARCH) => self.open_other_arch(),
            (Binding::Entity { .. }, OPEN_CURRENT) => catalog::open_current(hwnd),
            (
                Binding::Source { .. },
                LOAD_METADATA | REMOVE_SOURCE | REPLACE_SOURCE | RELOAD_SOURCE | REFRESH_SOURCE,
            ) => sources::action(hwnd, id),
            _ => return false,
        }
        true
    }
    fn open_other_arch(&self) {
        let Binding::Entity {
            id,
            catalog: snapshot,
            architecture,
            architecture_mask,
        } = &self.binding
        else {
            return;
        };
        let arch = architecture.other();
        if !arch.supported_by(*architecture_mask) {
            return;
        }
        if let Some(h) = find(
            |_, d| matches!(&d.binding, Binding::Entity { id: other, catalog, architecture, .. } if *architecture == arch && other == id && Arc::ptr_eq(snapshot, catalog)),
        ) {
            workspace::activate_document(h);
        } else {
            let mut binding = self.binding.clone();
            if let Binding::Entity { architecture, .. } = &mut binding {
                *architecture = arch;
            }
            if let Err(e) = open(&self.name, binding) {
                catalog::set_status(e);
            }
        }
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
        Binding::Source { .. } => format!("{}\r\n\r\n", doc.name),
        Binding::Entity {
            id,
            catalog,
            architecture,
            ..
        } => {
            let source = catalog
                .sources()
                .iter()
                .find(|s| id.starts_with(&s.id))
                .unwrap();
            format!(
                "{} — {}\r\n{}\r\n{}\r\n{}\r\n\r\n",
                doc.name,
                architecture.label(),
                source.path,
                source.id,
                id
            )
        }
    };
    let prefix = if old {
        format!("{}\r\n", resources::text(IDS_OLD_VERSION))
    } else {
        String::new()
    };
    set_text(doc.editor, &format!("{prefix}{header}{}", doc.content));
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
                    d.hwnd
                }),
            })
            .collect::<Vec<_>>()
    });
    for h in docs {
        refresh_content(h);
    }
}

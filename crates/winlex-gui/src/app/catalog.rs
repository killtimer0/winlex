//! UI transactions and immutable document bindings. HWNDs never cross the worker boundary.
use super::{Architecture, document, edit_document, read, workspace, write};
use crate::native::text;
use crate::{
    native::*,
    resources::{self, *},
    worker::{Completion, Job, Output, Ticket, Worker},
};
use std::{path::PathBuf, sync::Arc};
use winlex_core::{Catalog, EntitySummary, SourceData};

pub(super) const SEARCH_TIMER: usize = 1;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Panel {
    Results,
    Sources,
}
pub(super) struct Session {
    pub worker: Worker,
    serial: u64,
    pub catalog: Option<Arc<Catalog>>,
    pub inputs: Vec<SourceData>,
    desired_paths: Vec<PathBuf>,
    load: Option<Ticket>,
    search: Option<Ticket>,
    debounce: Option<std::time::Instant>,
    pub results: Vec<EntitySummary>,
    pub panel: Panel,
    pub sources: super::sources::Sources,
    pub status: String,
}
#[derive(Clone)]
pub(super) enum Binding {
    Entity {
        id: String,
        catalog: Arc<Catalog>,
        architecture_mask: i32,
    },
    Source {
        path: PathBuf,
        catalog: Option<Arc<Catalog>>,
    },
}
#[derive(Clone)]
pub(super) struct DocumentQuery {
    pub binding: Binding,
    pub ticket: Option<Ticket>,
    pub content: Arc<str>,
    pub source: String,
}
impl DocumentQuery {
    pub fn new(binding: Binding) -> Self {
        Self {
            binding,
            ticket: None,
            content: "".into(),
            source: String::new(),
        }
    }
    pub fn cancel(&self) {
        if let Some(t) = &self.ticket {
            t.cancel();
        }
    }
    pub fn is_source(&self) -> bool {
        matches!(self.binding, Binding::Source { .. })
    }
    pub fn entity(&self) -> Option<&str> {
        match &self.binding {
            Binding::Entity { id, .. } => Some(id),
            _ => None,
        }
    }
}
impl Session {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            worker: Worker::new()?,
            serial: 0,
            catalog: None,
            inputs: vec![],
            desired_paths: vec![],
            load: None,
            search: None,
            debounce: None,
            results: vec![],
            panel: Panel::Results,
            sources: Default::default(),
            status: resources::text(IDS_EMPTY_CATALOG),
        })
    }
    pub fn next_id(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }
    pub fn ticket(&mut self) -> Ticket {
        Ticket::new(self.next_id())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(t) = &self.load {
            t.cancel();
        }
        if let Some(t) = &self.search {
            t.cancel();
        }
        self.sources.cancel();
    }
}
pub(super) fn set_status(message: String) {
    let hwnd = write(|a| {
        a.session.status = message.clone();
        a.status
    });
    set_status_text(hwnd, &message);
}
pub(super) fn paths() -> Vec<PathBuf> {
    read(|a| a.session.desired_paths.clone())
}
pub(super) fn load(paths: Vec<PathBuf>) {
    reload(paths, vec![]);
}
pub(super) fn reload(paths: Vec<PathBuf>, reload: Vec<PathBuf>) {
    let result = write(|a| {
        let s = &mut a.session;
        if let Some(t) = s.load.take() {
            t.cancel();
        }
        let ticket = s.ticket();
        s.load = Some(ticket.clone());
        s.desired_paths = paths.clone();
        s.sources.pending = paths
            .iter()
            .filter(|p| reload.contains(p) || !s.inputs.iter().any(|i| i.path() == p.as_path()))
            .cloned()
            .collect();
        for path in &s.sources.pending {
            s.sources.errors.remove(path);
        }
        let result = s.worker.submit(
            ticket,
            Job::Load {
                paths,
                previous: s.inputs.clone(),
                reload,
            },
        );
        if result.is_err() {
            s.load = None;
            s.sources.pending.clear();
            s.desired_paths = s.inputs.iter().map(|s| s.path().to_path_buf()).collect();
        }
        result
    });
    set_status(result.map_or_else(|e| e, |_| resources::text(IDS_LOADING)));
    super::sources::refresh_views();
}

pub(super) fn input_changed() {
    let frame = read(|a| a.frame);
    unsafe {
        KillTimer(frame, SEARCH_TIMER);
    }
    write(|a| {
        if let Some(t) = a.session.search.take() {
            t.cancel();
        }
        a.session.results.clear();
        a.session.debounce = None;
    });
    update_results(false);
    if !super::frame::composing() && !text(read(|a| a.search)).trim().is_empty() {
        set_status(resources::text(IDS_SEARCHING));
        write(|a| {
            a.session.debounce =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(200))
        });
        if unsafe { SetTimer(frame, SEARCH_TIMER, 200, None) } == 0 {
            set_status(error("Search debounce"));
        }
    } else {
        set_status(resources::text(IDS_ENTER_QUERY));
    }
}
pub(super) fn search() {
    write(|a| a.session.debounce = None);
    let frame = read(|a| a.frame);
    unsafe {
        KillTimer(frame, SEARCH_TIMER);
    }
    if super::frame::composing() {
        return;
    }
    let query = text(read(|a| a.search)).trim().to_owned();
    let sent = write(|a| {
        let s = &mut a.session;
        if let Some(t) = s.search.take() {
            t.cancel();
        }
        s.results.clear();
        let Some(catalog) = s.catalog.clone() else {
            return Ok(false);
        };
        if query.is_empty() {
            return Ok(false);
        }
        let ticket = s.ticket();
        s.search = Some(ticket.clone());
        let result = s
            .worker
            .submit(ticket, Job::Search { catalog, query })
            .map(|_| true);
        if result.is_err() {
            s.search = None;
        }
        result
    });
    update_results(false);
    set_status(match sent {
        Ok(true) => resources::text(IDS_SEARCHING),
        Ok(false) => resources::text(IDS_ENTER_QUERY),
        Err(e) => e,
    });
}
pub(super) fn timer() {
    if read(|a| {
        a.session
            .debounce
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }) {
        search();
    }
}
fn update_results(append: bool) {
    let (hwnd, count) = read(|a| (a.results, a.session.results.len()));
    send(
        hwnd,
        LVM_SETITEMCOUNT,
        count,
        if append {
            (LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL) as isize
        } else {
            0
        },
    );
    super::sidebar::fit(hwnd);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}
pub(super) fn cell(row: usize, detail: bool) -> String {
    read(|a| {
        let Some(e) = a.session.results.get(row) else {
            return String::new();
        };
        let source = a
            .session
            .catalog
            .as_ref()
            .and_then(|c| c.sources().iter().find(|s| s.id == e.source_id));
        let label = source
            .map(|s| {
                s.assembly
                    .as_ref()
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| {
                        s.native_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    })
            })
            .unwrap_or_default();
        let arches = [(1, "x86"), (2, "x64"), (4, "ARM64")]
            .into_iter()
            .filter_map(|(mask, name)| (e.architecture_mask & mask != 0).then_some(name))
            .collect::<Vec<_>>()
            .join("/");
        if detail {
            format!(
                "{}\r\n{} · {} · {}\r\n{}\r\n{}",
                e.full_name,
                e.kind,
                label,
                arches,
                source.map(|s| s.path.as_str()).unwrap_or(""),
                e.id
            )
        } else {
            format!(
                "{}\n{} · {} · {} [{}]",
                e.name,
                e.kind,
                arches,
                label,
                &e.source_id[..8]
            )
        }
    })
}
pub(super) fn open_selected() {
    let list = read(|a| a.results);
    open_row(send(
        list,
        LVM_GETNEXTITEM,
        usize::MAX,
        LVNI_SELECTED as isize,
    ));
}
pub(super) fn open_row(row: isize) {
    if row < 0 {
        return;
    }
    let item = read(|a| {
        a.session.results.get(row as usize).map(|e| {
            (
                e.id.clone(),
                e.name.clone(),
                e.architecture_mask,
                a.session.catalog.clone(),
            )
        })
    });
    let Some((id, title, mask, Some(catalog))) = item else {
        return;
    };
    let arch = if Architecture::X64.supported_by(mask) {
        Architecture::X64
    } else if Architecture::X86.supported_by(mask) {
        Architecture::X86
    } else {
        set_status(resources::text(IDS_UNSUPPORTED_ARCH));
        return;
    };
    if let Err(e) = workspace::create_document(
        &title,
        arch,
        Binding::Entity {
            id,
            catalog,
            architecture_mask: mask,
        },
    ) {
        set_status(e);
    }
}
pub(super) fn old_version(binding: &Binding) -> bool {
    let Binding::Entity { id, catalog, .. } = binding else {
        return false;
    };
    let Some(source) = catalog.sources().iter().find(|s| id.starts_with(&s.id)) else {
        return false;
    };
    read(|a| {
        a.session
            .inputs
            .iter()
            .any(|s| s.path() == source.native_path && s.id() != source.id)
            || a.session.sources.entries.iter().any(|e| {
                e.path == source.native_path && e.id.as_ref().is_some_and(|id| id != &source.id)
            })
    })
}
pub(super) fn open_current(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    let Binding::Entity {
        id, catalog: old, ..
    } = &doc.query.binding
    else {
        return;
    };
    let current = read(|a| {
        let old_source = old.sources().iter().find(|s| id.starts_with(&s.id))?;
        let input = a
            .session
            .inputs
            .iter()
            .find(|s| s.path() == old_source.native_path)?;
        Some((a.session.catalog.clone()?, input.id().to_owned()))
    });
    let Some((current, source)) = current else {
        set_status(resources::text(IDS_CURRENT_UNAVAILABLE));
        return;
    };
    if doc.query.ticket.is_some() {
        return;
    }
    let ticket = write(|a| a.session.ticket());
    let job = Job::Reopen {
        previous: old.clone(),
        current,
        source,
        entity: id.clone(),
        arch: doc.architecture.core(),
    };
    if let Err(e) = read(|a| a.session.worker.submit(ticket.clone(), job)) {
        set_status(e);
    } else {
        edit_document(hwnd, |d| d.query.ticket = Some(ticket));
    }
}
pub(super) fn request_document(hwnd: HWND) {
    let Some(doc) = document(hwnd) else { return };
    doc.query.cancel();
    let ticket = write(|a| a.session.ticket());
    let job = match &doc.query.binding {
        Binding::Source { path, .. } => Job::Inspect {
            document: doc.id,
            path: path.clone(),
        },
        Binding::Entity { id, catalog, .. } => {
            let source_id = id.split_once("::").unwrap().0;
            let source = catalog
                .sources()
                .iter()
                .find(|s| s.id == source_id)
                .unwrap();
            edit_document(hwnd, |d| {
                d.query.source = format!("{}\r\n{}", source.path, source.id)
            });
            Job::Describe {
                document: doc.id,
                catalog: catalog.clone(),
                entity: id.clone(),
                arch: doc.architecture.core(),
            }
        }
    };
    edit_document(hwnd, |d| {
        d.query.ticket = Some(ticket.clone());
        d.query.content = resources::text(IDS_QUERYING).into();
    });
    if let Err(e) = read(|a| a.session.worker.submit(ticket, job)) {
        edit_document(hwnd, |d| {
            d.query.ticket = None;
            d.query.content = e.into();
        });
    }
    super::document::refresh_content(hwnd);
}
pub(super) fn event() -> HANDLE {
    read(|a| a.session.worker.event())
}
pub(super) fn poll() {
    loop {
        match read(|a| a.session.worker.rx.try_recv()) {
            Ok(c) => apply(c),
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let docs = write(|a| {
                    a.session.load = None;
                    a.session.search = None;
                    a.session.sources.cancel();
                    a.session.sources.pending.clear();
                    a.session.desired_paths = a
                        .session
                        .inputs
                        .iter()
                        .map(|s| s.path().to_path_buf())
                        .collect();
                    a.documents
                        .iter_mut()
                        .filter_map(|d| {
                            d.query.ticket.take().map(|_| {
                                d.query.content = "Catalog worker stopped".into();
                                d.hwnd
                            })
                        })
                        .collect::<Vec<_>>()
                });
                for h in docs {
                    super::document::refresh_content(h);
                }
                set_status("Catalog worker stopped".into());
                break;
            }
        }
    }
}
fn apply(c: Completion) {
    if read(|a| a.session.load.as_ref().is_some_and(|t| t.id == c.id)) {
        write(|a| {
            a.session.load = None;
            let pending = std::mem::take(&mut a.session.sources.pending);
            if let Err(error) = &c.result {
                for path in pending {
                    a.session.sources.errors.insert(path, error.clone());
                }
            }
        });
        match c.result {
            Ok(Output::Loaded(snapshot)) => {
                write(|a| {
                    let s = &mut a.session;
                    s.catalog = snapshot.catalog;
                    s.inputs = snapshot.inputs;
                    s.desired_paths = s.inputs.iter().map(|i| i.path().to_path_buf()).collect();
                });
                search();
                super::sources::refresh();
            }
            Err(e) => {
                write(|a| {
                    a.session.desired_paths = a
                        .session
                        .inputs
                        .iter()
                        .map(|s| s.path().to_path_buf())
                        .collect()
                });
                set_status(resources::text(IDS_LOAD_FAILED).replace("{error}", &e));
            }
            _ => unreachable!(),
        }
        super::sources::refresh_views();
        return;
    }
    if read(|a| a.session.search.as_ref().is_some_and(|t| t.id == c.id)) {
        match c.result {
            Ok(Output::Matches(batch)) => {
                let count = write(|a| {
                    a.session.results.extend(batch);
                    a.session.results.len()
                });
                update_results(true);
                set_status(
                    resources::text(IDS_SEARCH_PROGRESS).replace("{count}", &count.to_string()),
                );
            }
            Ok(Output::SearchDone(total)) => {
                write(|a| a.session.search = None);
                set_status(
                    resources::text(IDS_SEARCH_STATUS).replace("{total}", &total.to_string()),
                );
            }
            Err(e) => {
                write(|a| a.session.search = None);
                set_status(e);
            }
            _ => unreachable!(),
        }
        return;
    }
    if super::sources::apply_discovery(&c) {
        return;
    }
    let target = read(|a| {
        a.documents
            .iter()
            .find(|d| d.query.ticket.as_ref().is_some_and(|t| t.id == c.id))
            .map(|d| d.hwnd)
    });
    if let Some(hwnd) = target {
        edit_document(hwnd, |d| d.query.ticket = None);
        match c.result {
            Ok(Output::Reopened { catalog, entity }) => {
                if let Some(entity) = entity
                    && let Some(doc) = document(hwnd)
                {
                    if let Err(e) = workspace::create_document(
                        &entity.name,
                        doc.architecture,
                        Binding::Entity {
                            id: entity.id,
                            catalog,
                            architecture_mask: entity.architecture_mask,
                        },
                    ) {
                        set_status(e);
                    }
                } else {
                    set_status(resources::text(IDS_CURRENT_UNAVAILABLE));
                }
            }
            Ok(Output::Description { document: id, text }) => {
                if document(hwnd).is_some_and(|d| d.id == id) {
                    edit_document(hwnd, |d| d.query.content = text.into());
                }
            }
            Ok(Output::Inspected {
                document: id,
                catalog,
            }) => super::sources::inspected(hwnd, id, catalog),
            Err(e) => {
                if let Some(doc) = document(hwnd)
                    && let Binding::Source { path, .. } = doc.query.binding
                {
                    super::sources::inspection_failed(&path, &e);
                }
                edit_document(hwnd, |d| d.query.content = e.into());
            }
            _ => unreachable!(),
        }
        super::document::refresh_content(hwnd);
    }
}

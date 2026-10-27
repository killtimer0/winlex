//! UI transactions and immutable document bindings. HWNDs never cross the worker boundary.
use super::{
    Architecture,
    metadata::{self, Binding},
    read, write,
};
use crate::native::text;
use crate::{
    native::*,
    resources::{self, *},
    tasks::{Completion, Job, Output, Target},
    worker::Ticket,
};
use std::{path::PathBuf, sync::Arc};
use winlex_core::{Catalog, EntitySummary, SourceData};

struct Load {
    ticket: Ticket,
    paths: Vec<PathBuf>,
    pending: Vec<PathBuf>,
}
enum Search {
    Idle,
    Waiting,
    Running(Ticket),
    Done(usize),
    Failed(String),
}
pub(super) struct Session {
    serial: u64,
    pub catalog: Option<Arc<Catalog>>,
    pub inputs: Vec<SourceData>,
    load: Option<Load>,
    load_error: Option<String>,
    search: Search,
    pub results: Vec<EntitySummary>,
    pub sources: super::sources::Sources,
    notice: Option<String>,
}
impl Session {
    pub fn new() -> Self {
        Self {
            serial: 0,
            catalog: None,
            inputs: vec![],
            load: None,
            load_error: None,
            search: Search::Idle,
            results: vec![],
            sources: Default::default(),
            notice: None,
        }
    }
    pub fn ticket(&mut self) -> Ticket {
        self.serial += 1;
        Ticket::new(self.serial)
    }
    fn cancel_search(&mut self) -> Option<Ticket> {
        if let Search::Running(t) = std::mem::replace(&mut self.search, Search::Idle) {
            t.cancel();
            Some(t)
        } else {
            None
        }
    }
    fn accept_search(&mut self, completion: Completion) -> bool {
        if !matches!(&self.search, Search::Running(t) if t.id == completion.id) {
            return false;
        }
        match completion.result {
            Ok(Output::Matches(batch)) => self.results.extend(batch),
            Ok(Output::SearchDone(total)) => self.search = Search::Done(total),
            Err(e) => self.search = Search::Failed(e),
            _ => unreachable!(),
        }
        true
    }
    pub fn pending(&self, path: &std::path::Path) -> bool {
        self.load
            .as_ref()
            .is_some_and(|load| load.pending.iter().any(|p| p == path))
    }
    fn status(&self) -> String {
        if let Some(message) = self.notice.as_ref().or(self.load_error.as_ref()) {
            return message.clone();
        }
        if self.load.is_some() {
            return resources::text(IDS_LOADING);
        }
        match &self.search {
            Search::Idle => resources::text(if self.catalog.is_some() {
                IDS_ENTER_QUERY
            } else {
                IDS_EMPTY_CATALOG
            }),
            Search::Waiting => resources::text(IDS_SEARCHING),
            Search::Running(_) => resources::text(IDS_SEARCH_PROGRESS)
                .replace("{count}", &self.results.len().to_string()),
            Search::Done(total) => {
                resources::text(IDS_SEARCH_STATUS).replace("{total}", &total.to_string())
            }
            Search::Failed(e) => e.clone(),
        }
    }
}
pub(super) fn set_status(message: String) {
    write(|a| a.session.notice = Some(message));
    refresh_status();
}
fn refresh_status() {
    let (hwnd, message) = read(|a| (a.status, a.session.status()));
    set_status_text(hwnd, &message);
}
pub(super) fn paths() -> Vec<PathBuf> {
    read(|a| {
        a.session.load.as_ref().map_or_else(
            || {
                a.session
                    .inputs
                    .iter()
                    .map(|i| i.path().to_path_buf())
                    .collect()
            },
            |load| load.paths.clone(),
        )
    })
}
pub(super) fn load(paths: Vec<PathBuf>) {
    reload(paths, vec![]);
}
pub(super) fn reload(paths: Vec<PathBuf>, reload: Vec<PathBuf>) {
    let result = write(|a| {
        let s = &mut a.session;
        let previous = s.load.take().map(|load| load.ticket.id);
        s.notice = None;
        s.load_error = None;
        let ticket = s.ticket();
        let pending = paths
            .iter()
            .filter(|p| reload.contains(p) || !s.inputs.iter().any(|i| i.path() == p.as_path()))
            .cloned()
            .collect();
        s.load = Some(Load {
            ticket: ticket.clone(),
            paths: paths.clone(),
            pending,
        });
        let result = a.worker.replace(
            previous,
            ticket,
            Job::Load {
                paths,
                previous: s.inputs.clone(),
                reload,
            },
        );
        if result.is_err() {
            s.load = None;
        }
        result
    });
    if let Err(e) = result {
        write(|a| a.session.load_error = Some(e));
    }
    refresh_status();
    super::sources::refresh_views();
}

pub(super) fn input_changed() {
    let (input, composing) = super::panel::cancel_debounce();
    write(|a| {
        if let Some(ticket) = a.session.cancel_search() {
            a.worker.cancel(ticket.id);
        }
        a.session.notice = None;
        a.session.load_error = None;
        a.session.results.clear();
    });
    update_results(false);
    if !composing && !text(input).trim().is_empty() {
        let state = match super::panel::debounce(input) {
            Ok(()) => Search::Waiting,
            Err(e) => Search::Failed(e),
        };
        write(|a| a.session.search = state);
    }
    refresh_status();
}
pub(super) fn search() {
    let (input, composing) = super::panel::cancel_debounce();
    if composing {
        return;
    }
    let query = text(input).trim().to_owned();
    let sent = write(|a| {
        let s = &mut a.session;
        let previous = match std::mem::replace(&mut s.search, Search::Idle) {
            Search::Running(ticket) => Some(ticket),
            _ => None,
        };
        s.results.clear();
        let Some(catalog) = s.catalog.clone() else {
            if let Some(ticket) = previous {
                a.worker.cancel(ticket.id);
            }
            return Ok(false);
        };
        if query.is_empty() {
            if let Some(ticket) = previous {
                a.worker.cancel(ticket.id);
            }
            return Ok(false);
        }
        let ticket = s.ticket();
        s.search = Search::Running(ticket.clone());
        a.worker
            .replace(
                previous.map(|t| t.id),
                ticket,
                Job::Search { catalog, query },
            )
            .map(|_| true)
    });
    update_results(false);
    if let Err(e) = sent {
        write(|a| a.session.search = Search::Failed(e));
    }
    refresh_status();
}
fn update_results(append: bool) {
    let (hwnd, count) = read(|a| {
        (
            a.sidebar.panel(super::panel::SEARCH_PANEL).list(),
            a.session.results.len(),
        )
    });
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
    super::panel::fit(hwnd);
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
    if let Err(e) = metadata::open(
        &title,
        Binding::Entity {
            architecture: arch,
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
    let Some((
        document,
        Binding::Entity {
            id,
            catalog: old,
            architecture,
            ..
        },
        pending,
    )) = metadata::get(hwnd, |id, d| (id, d.binding.clone(), d.ticket.is_some()))
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
    if pending {
        return;
    }
    let ticket = write(|a| a.session.ticket());
    let job = Job::Reopen {
        document,
        previous: old,
        current,
        source,
        entity: id,
        arch: architecture.core(),
    };
    if let Err(e) = read(|a| a.worker.replace(None, ticket.clone(), job)) {
        set_status(e);
    } else {
        metadata::edit(hwnd, |d| d.ticket = Some(ticket));
    }
}
pub(super) fn request_document(hwnd: HWND) {
    let Some((document, binding, previous)) =
        metadata::get(hwnd, |id, d| (id, d.binding.clone(), d.ticket.clone()))
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
            arch: architecture.core(),
        },
    };
    metadata::edit(hwnd, |d| {
        d.ticket = Some(ticket.clone());
        d.content = resources::text(IDS_QUERYING).into();
    });
    if let Err(e) = read(|a| a.worker.replace(previous.map(|t| t.id), ticket, job)) {
        metadata::edit(hwnd, |d| {
            d.ticket = None;
            d.content = e.into();
        });
    }
    metadata::refresh_content(hwnd);
}
pub(super) fn worker_stopped() {
    write(|a| {
        a.session.load = None;
        a.session.cancel_search();
        a.session.sources.cancel();
    });
    set_status("Background worker stopped".into());
    super::sources::refresh_views();
}
pub(super) fn apply(c: Completion) {
    match c.target {
        Target::Load => {
            if !read(|a| {
                a.session
                    .load
                    .as_ref()
                    .is_some_and(|load| load.ticket.id == c.id)
            }) {
                return;
            }
            write(|a| a.session.load = None);
            match c.result {
                Ok(Output::Loaded(snapshot)) => {
                    write(|a| {
                        let s = &mut a.session;
                        s.catalog = snapshot.catalog;
                        s.inputs = snapshot.inputs;
                    });
                    search();
                    super::sources::refresh();
                }
                Err(e) => {
                    write(|a| {
                        a.session.load_error =
                            Some(resources::text(IDS_LOAD_FAILED).replace("{error}", &e))
                    });
                }
                _ => unreachable!(),
            }
            super::sources::refresh_views();
            refresh_status();
        }
        Target::Search => {
            if write(|a| a.session.accept_search(c)) {
                update_results(true);
                refresh_status();
            }
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_search_results_cannot_overwrite_new_search_or_load_failure() {
        let mut session = Session::new();
        let old = Ticket::new(1);
        session.search = Search::Running(old.clone());
        session.cancel_search();
        assert!(old.cancel.load(std::sync::atomic::Ordering::Relaxed));
        session.search = Search::Running(Ticket::new(2));
        session.load_error = Some("load transaction failed".into());
        let completion = |id, result| Completion {
            id,
            target: Target::Search,
            result,
        };
        assert!(!session.accept_search(completion(1, Ok(Output::SearchDone(100)))));
        assert!(session.accept_search(completion(2, Ok(Output::Matches(vec![])))));
        assert_eq!(session.status(), "load transaction failed");
        assert!(session.accept_search(completion(2, Ok(Output::SearchDone(0)))));
        assert_eq!(session.status(), "load transaction failed");
        assert!(!session.accept_search(completion(2, Err("late failure".into()))));
        assert!(matches!(session.search, Search::Done(0)));
    }
}

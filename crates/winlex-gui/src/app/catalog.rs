//! UI transactions and immutable document bindings. HWNDs never cross the worker boundary.
use super::{
    metadata::{self, Binding},
    read, workspace, write,
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
    Done,
    Failed(String),
}
pub(super) struct Session {
    serial: u64,
    pub catalog: Option<Arc<Catalog>>,
    load: Option<Load>,
    load_error: Option<String>,
    search: Search,
    pub results: Vec<EntitySummary>,
    pub sources: super::sources::Sources,
    notice: Option<String>,
}
impl Session {
    pub fn inputs(&self) -> &[SourceData] {
        self.catalog.as_ref().map_or(&[], |c| c.inputs())
    }
    pub fn new() -> Self {
        Self {
            serial: 0,
            catalog: None,
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
            Ok(Output::SearchDone) => self.search = Search::Done,
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
            Search::Done => resources::text(IDS_SEARCH_STATUS)
                .replace("{total}", &self.results.len().to_string()),
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
                    .inputs()
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
            .filter(|p| reload.contains(p) || !s.inputs().iter().any(|i| i.path() == p.as_path()))
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
                previous: s.catalog.clone(),
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
            unsafe { GetDlgItem(a.frame, RESULTS as i32) },
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
        // Results are cleared when the catalog changes; each ID belongs to this snapshot.
        let catalog = a.session.catalog.as_ref().expect("search result snapshot");
        let source = &catalog.sources()[e.id.source_id];
        let label = source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let arches = super::architecture::supported(e.architecture_mask)
            .map(|architecture| architecture.name())
            .collect::<Vec<_>>()
            .join("/");
        if detail {
            format!(
                "{}\r\n{} · {} · {}\r\n{}\r\n{}",
                e.full_name,
                e.kind,
                label,
                arches,
                source.path.display(),
                e.id
            )
        } else {
            format!("{}\n{} · {} · {}", e.name, e.kind, arches, label)
        }
    })
}
/// How a result reaches a document: transient preview, pinned open, or forced duplicate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Open {
    Preview,
    Pin,
    New,
}
pub(super) fn open_selected(mode: Open) {
    let frame = read(|a| a.frame);
    let list = unsafe { GetDlgItem(frame, RESULTS as i32) };
    let row = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    open(row, mode);
}
pub(super) fn open(row: isize, mode: Open) {
    match mode {
        // A preview shows the document but leaves the keyboard in the sidebar.
        Open::Preview => workspace::keep_focus(|| {
            open_now(row, mode);
        }),
        // An explicit open enters the document: show it, then focus its body.
        Open::Pin | Open::New => {
            if let Some(hwnd) = open_now(row, mode) {
                super::document::focus_document(hwnd);
            }
        }
    }
}
fn open_now(row: isize, mode: Open) -> Option<HWND> {
    if row < 0 {
        return None;
    }
    let item = read(|a| {
        a.session.results.get(row as usize).map(|e| {
            (
                e.id,
                e.name.clone(),
                e.architecture_mask,
                a.session.catalog.clone(),
                a.settings.preferred_architecture,
            )
        })
    });
    let (id, title, mask, Some(catalog), preferred) = item? else {
        return None;
    };
    let Some(fallback) = super::architecture::preferred(mask, preferred) else {
        set_status(resources::text(IDS_UNSUPPORTED_ARCH));
        return None;
    };
    let preview = workspace::preview_document();
    // A new result keeps the architecture of the declaration being read.
    let architecture = if mode == Open::New {
        fallback
    } else {
        metadata::get(workspace::active(), |_, d| d.architecture())
            .flatten()
            .filter(|arch| arch.supports(mask))
            .unwrap_or(fallback)
    };
    let binding = Binding::Entity {
        architecture,
        id,
        catalog: catalog.clone(),
        architecture_mask: mask,
    };
    if mode != Open::New
        && let Some(hwnd) = metadata::find(|_, d| {
            matches!(
                &d.binding,
                Binding::Entity { architecture: found_arch, id: found_id, catalog: found_catalog, .. }
                    if *found_arch == architecture
                        && *found_id == id
                        && Arc::ptr_eq(found_catalog, &catalog)
            )
        })
    {
        if mode == Open::Pin {
            workspace::pin_document(hwnd);
        }
        workspace::show_document(hwnd);
        return Some(hwnd);
    }
    if mode != Open::New
        && let Some(hwnd) = preview
        && metadata::rebind(hwnd, &title, binding.clone())
    {
        if mode == Open::Pin {
            workspace::pin_document(hwnd);
        }
        workspace::show_document(hwnd);
        return Some(hwnd);
    }
    match metadata::open(&title, binding, mode == Open::Preview) {
        Ok(hwnd) => Some(hwnd),
        Err(e) => {
            set_status(e);
            None
        }
    }
}
pub(super) fn worker_stopped() {
    write(|a| {
        a.session.load = None;
        a.session.cancel_search();
        a.session.sources.cancel();
    });
    set_status(resources::text(IDS_WORKER_STOPPED));
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
                        s.catalog = snapshot;
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
        assert!(!session.accept_search(completion(1, Ok(Output::SearchDone))));
        assert!(session.accept_search(completion(2, Ok(Output::Matches(vec![])))));
        assert_eq!(session.status(), "load transaction failed");
        assert!(session.accept_search(completion(2, Ok(Output::SearchDone))));
        assert_eq!(session.status(), "load transaction failed");
        assert!(!session.accept_search(completion(2, Err("late failure".into()))));
        assert!(matches!(session.search, Search::Done));
    }
}

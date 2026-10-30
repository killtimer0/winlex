//! UI transactions and immutable document bindings. HWNDs never cross the worker boundary.
use super::{
    metadata::{self, Binding},
    read, workspace, write,
};
use crate::native::text;
use crate::results::Items;
use crate::{
    native::*,
    resources::{self, *},
    tasks::{Completion, Job, Output, Target},
    worker::Ticket,
};
use std::{collections::BTreeMap, os::windows::ffi::OsStrExt, path::PathBuf, sync::Arc};
use winlex_core::{Catalog, SourceData};

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
    Failed(anyhow::Error),
}
pub(super) struct Session {
    serial: u64,
    pub catalog: Option<Arc<Catalog>>,
    load: Option<Load>,
    load_error: Option<String>,
    search: Search,
    pub results: Items,
    pub source_rows: Vec<super::sources::Row>,
    pub updating_sources: bool,
    source_errors: BTreeMap<PathBuf, String>,
    pending_results: Items,
    merge_buffer: Items,
    pub remember_sources: bool,
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
            source_rows: vec![],
            updating_sources: false,
            source_errors: BTreeMap::new(),
            pending_results: vec![],
            merge_buffer: vec![],
            remember_sources: true,
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
            Ok(Output::Matches(batch)) => self.pending_results.extend(batch),
            Ok(Output::SearchDone) => self.search = Search::Done,
            Err(e) => {
                let error = if let Some(input) = e.downcast_ref::<winlex_core::InvalidSearchInput>()
                {
                    let label = match input.0 {
                        winlex_core::SearchMode::Name => IDS_INVALID_NAME_QUERY,
                        winlex_core::SearchMode::Value => IDS_INVALID_VALUE_QUERY,
                        winlex_core::SearchMode::Guid => IDS_INVALID_GUID_QUERY,
                        winlex_core::SearchMode::Combined => unreachable!(),
                    };
                    anyhow::anyhow!(resources::text(label))
                } else {
                    e
                };
                self.search = Search::Failed(error);
            }
            _ => unreachable!(),
        }
        true
    }
    pub fn pending(&self, path: &std::path::Path) -> bool {
        self.load
            .as_ref()
            .is_some_and(|load| load.pending.iter().any(|p| p == path))
    }
    pub fn source_error(&self, path: &std::path::Path) -> Option<&str> {
        self.source_errors.get(path).map(String::as_str)
    }
    fn fail_load(&mut self, error: String) {
        if let Some(load) = self.load.take() {
            for path in load.pending {
                if self.inputs().iter().any(|source| source.path() == path) {
                    self.source_errors.insert(path, error.clone());
                }
            }
        }
        self.load_error = Some(error);
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
            Search::Failed(e) => format!("{e:#}"),
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
pub(super) fn reload(paths: Vec<PathBuf>, mut reload: Vec<PathBuf>) {
    let result = write(|a| {
        let s = &mut a.session;
        let previous = s.load.take().map(|load| {
            for path in load.pending {
                if !reload.contains(&path) {
                    reload.push(path);
                }
            }
            load.ticket.id
        });
        s.notice = None;
        s.load_error = None;
        let ticket = s.ticket();
        let pending: Vec<PathBuf> = paths
            .iter()
            .filter(|p| reload.contains(p) || !s.inputs().iter().any(|i| i.path() == p.as_path()))
            .cloned()
            .collect();
        s.source_errors.retain(|path, _| !pending.contains(path));
        s.load = Some(Load {
            ticket: ticket.clone(),
            paths: paths.clone(),
            pending,
        });
        a.worker.replace(
            previous,
            ticket,
            Job::Load {
                paths,
                previous: s.catalog.clone(),
                reload,
            },
        )
    });
    if let Err(e) = result {
        write(|a| a.session.fail_load(format!("{e:#}")));
    }
    refresh_status();
    super::sources::refresh();
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
        a.session.pending_results.clear();
    });
    update_results();
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
    let mode = super::panel::search_mode();
    let sent = write(|a| {
        let s = &mut a.session;
        let previous = match std::mem::replace(&mut s.search, Search::Idle) {
            Search::Running(ticket) => Some(ticket),
            _ => None,
        };
        s.results.clear();
        s.pending_results.clear();
        let Some(catalog) = s.catalog.clone().filter(|_| !query.is_empty()) else {
            if let Some(ticket) = previous {
                a.worker.cancel(ticket.id);
            }
            return Ok(());
        };
        let ticket = s.ticket();
        s.search = Search::Running(ticket.clone());
        a.worker.replace(
            previous.map(|t| t.id),
            ticket,
            Job::Search {
                catalog,
                query,
                mode,
                categories: a.settings.preferences.categories,
            },
        )
    });
    update_results();
    if let Err(e) = sent {
        write(|a| a.session.search = Search::Failed(e));
    }
    refresh_status();
}
fn update_results() {
    let (hwnd, count) = read(|a| {
        (
            unsafe { GetDlgItem(a.frame, RESULTS as i32) },
            a.session.results.len(),
        )
    });
    send(hwnd, LVM_SETITEMCOUNT, count, 0);
    crate::list::fit(hwnd);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

fn merge_results(
    results: &mut Items,
    batch: &mut Items,
    buffer: &mut Items,
    tracked: &mut [isize; 4],
) {
    batch.sort_unstable_by(|a, b| a.compare(b));
    buffer.clear();
    buffer.reserve(results.len() + batch.len());
    let mut old = results.drain(..).enumerate().peekable();
    let mut new = batch.drain(..).peekable();
    let original = *tracked;
    while old.peek().is_some() || new.peek().is_some() {
        if new
            .peek()
            .is_some_and(|n| old.peek().is_none_or(|(_, o)| n.compare(o).is_lt()))
        {
            buffer.push(new.next().unwrap());
        } else {
            let (index, item) = old.next().unwrap();
            for (current, &previous) in tracked.iter_mut().zip(&original) {
                if previous == index as isize {
                    *current = buffer.len() as isize;
                }
            }
            buffer.push(item);
        }
    }
    drop(old);
    drop(new);
    std::mem::swap(results, buffer);
}

pub(super) fn flush_results() {
    if !read(|a| !a.session.pending_results.is_empty()) {
        refresh_status();
        return;
    }
    let hwnd = read(|a| unsafe { GetDlgItem(a.frame, RESULTS as i32) });
    let mut state = crate::list::State::capture(hwnd);
    write(|a| {
        let s = &mut a.session;
        merge_results(
            &mut s.results,
            &mut s.pending_results,
            &mut s.merge_buffer,
            &mut state.indices,
        );
    });
    state.apply(hwnd, read(|a| a.session.results.len()));
    read(|a| a.sidebar.panel(super::panel::SEARCH_PANEL)).refresh_tooltip();
    refresh_status();
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&crate::list::Display)) {
    read(|a| {
        if let Some(item) = a.session.results.get(index) {
            visit(&item.display);
        }
    });
}
pub(super) fn row_text(row: usize) -> Vec<u16> {
    read(|a| {
        let Some(e) = a.session.results.get(row) else {
            return Vec::new();
        };
        // Results are cleared when the catalog changes; each ID belongs to this snapshot.
        let catalog = a.session.catalog.as_ref().expect("search result snapshot");
        let source = &catalog.sources()[e.id.source_id];
        let mut value: Vec<u16> = format!("{}\r\n{} · ", e.full_name, e.kind)
            .encode_utf16()
            .collect();
        value.extend(source.path.file_name().unwrap_or_default().encode_wide());
        value.extend("\r\n".encode_utf16());
        value.extend(&e.display.detail);
        value.extend("\r\n".encode_utf16());
        value.extend(source.path.as_os_str().encode_wide());
        value.extend(format!("\r\n{}", e.id).encode_utf16());
        value
    })
}
/// How a result reaches a document: transient preview, pinned open, or forced duplicate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Open {
    Preview,
    Pin,
    New,
}
pub(super) fn activate(row: isize, double: bool) {
    open(row, if double { Open::Pin } else { Open::Preview });
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
        metadata::binding(workspace::active())
            .map(|binding| binding.architecture)
            .filter(|arch| arch.supports(mask))
            .unwrap_or(fallback)
    };
    let binding = Binding {
        architecture,
        id,
        catalog: catalog.clone(),
        architecture_mask: mask,
    };
    let existing = (mode != Open::New)
        .then(|| {
            metadata::find(&binding)
                .or_else(|| preview.filter(|&hwnd| metadata::rebind(hwnd, &title, binding.clone())))
        })
        .flatten();
    let hwnd = match existing {
        Some(hwnd) => hwnd,
        None => match metadata::open(&title, binding, mode == Open::Preview) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                set_status(format!("{error:#}"));
                return None;
            }
        },
    };
    if mode == Open::Pin {
        workspace::pin_document(hwnd);
    }
    workspace::show_document(hwnd);
    Some(hwnd)
}
pub(super) fn worker_stopped() {
    write(|a| {
        a.session.fail_load(resources::text(IDS_WORKER_STOPPED));
        a.session.cancel_search();
    });
    set_status(resources::text(IDS_WORKER_STOPPED));
    super::sources::refresh();
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
            match c.result {
                Ok(Output::Loaded(snapshot)) => {
                    write(|a| {
                        let s = &mut a.session;
                        s.load = None;
                        s.source_errors.retain(|path, _| {
                            snapshot
                                .as_ref()
                                .is_some_and(|c| c.inputs().iter().any(|i| i.path() == path))
                        });
                        s.catalog = snapshot;
                    });
                    search();
                    if let Err(error) = read(|a| {
                        if a.session.remember_sources {
                            let paths: Vec<_> = a
                                .session
                                .inputs()
                                .iter()
                                .map(|s| s.path().to_path_buf())
                                .collect();
                            a.config.write_key()?.set_paths("AutoLoad", &paths)?;
                        }
                        Ok::<_, anyhow::Error>(())
                    }) {
                        set_status(format!("{error:#}"));
                    }
                    for (hwnd, view) in workspace::views() {
                        view.catalog_changed(hwnd);
                    }
                }
                Err(e) => {
                    write(|a| {
                        a.session.fail_load(
                            resources::text(IDS_LOAD_FAILED).replace("{error}", &format!("{e:#}")),
                        )
                    });
                }
                _ => unreachable!(),
            };
            super::sources::refresh();
            refresh_status();
        }
        Target::Search => {
            write(|a| a.session.accept_search(c));
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::ResultItem;
    fn hit(token: u32, score: u16) -> Box<ResultItem> {
        Box::new(ResultItem {
            display: Default::default(),
            matched: winlex_core::SearchMatch {
                entity: winlex_core::EntitySummary {
                    id: winlex_core::EntityId {
                        source_id: 0,
                        token,
                    },
                    name: format!("symbol{token}"),
                    full_name: format!("Fixture.symbol{token}"),
                    kind: "type".into(),
                    architecture_mask: 7,
                },
                quality: winlex_core::MatchQuality::Fuzzy {
                    score,
                    field: winlex_core::MatchField::Name,
                },
            },
        })
    }
    #[test]
    fn incremental_merge_matches_global_ranking_for_every_batch_size() {
        for size in 1..=80 {
            let mut results = vec![];
            let mut buffer = vec![];
            let mut tracked = [-1; 4];
            for chunk in (1..=200).collect::<Vec<u32>>().chunks(size) {
                let mut batch = chunk
                    .iter()
                    .map(|&id| hit(id, (id * 71 % 23) as u16))
                    .collect();
                merge_results(&mut results, &mut batch, &mut buffer, &mut tracked);
                assert!(results.windows(2).all(|p| !p[0].compare(&p[1]).is_gt()));
                assert!(batch.is_empty());
            }
            let mut expected: Vec<_> = (1..=200).map(|id| hit(id, (id * 71 % 23) as u16)).collect();
            expected.sort_unstable_by(|a, b| a.compare(b));
            assert_eq!(
                results.iter().map(|m| m.id).collect::<Vec<_>>(),
                expected.iter().map(|m| m.id).collect::<Vec<_>>()
            );
        }
    }
    #[test]
    fn merge_migrates_selection_focus_mark_and_scroll_without_identity_lookup() {
        let mut results = vec![hit(10, 100), hit(20, 80), hit(30, 60)];
        let mut batch = vec![hit(1, 100), hit(2, 70), hit(3, 120)];
        let mut tracked = [1, 2, 1, 0];
        merge_results(&mut results, &mut batch, &mut vec![], &mut tracked);
        assert_eq!(tracked, [3, 5, 3, 2]);
        assert_eq!(results[tracked[0] as usize].id.token, 20);
        assert_eq!(results[tracked[3] as usize].id.token, 10);
        let mut missing = [-1; 4];
        merge_results(
            &mut results,
            &mut vec![hit(99, 200)],
            &mut vec![],
            &mut missing,
        );
        assert_eq!(missing, [-1; 4]);
    }
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
        assert!(!session.accept_search(completion(2, Err(anyhow::anyhow!("late failure")))));
        assert!(matches!(session.search, Search::Done));
    }
}

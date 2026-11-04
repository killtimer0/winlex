//! UI transactions and immutable document bindings. HWNDs never cross the worker boundary.
use super::{
    frame, history,
    metadata::{self, ArchitectureChoice, Binding},
    settings, workspace,
};
use crate::app::context::Context;
use crate::native::text;
use crate::results::Items;
use crate::{
    native::*,
    resources::{self, *},
    tasks::{Job, LoadInput},
    worker::Ticket,
};
use std::{collections::BTreeMap, os::windows::ffi::OsStrExt, path::PathBuf, sync::Arc};
use winlex_core::{Catalog, SourceData};

struct Load {
    id: u64,
    inputs: Arc<[LoadInput]>,
}
impl Load {
    fn pending(&self) -> impl Iterator<Item = &std::path::Path> {
        self.inputs.iter().filter_map(|input| match input {
            LoadInput::Read(path) => Some(path.as_path()),
            LoadInput::Reuse(_) => None,
        })
    }
}
#[derive(Default)]
enum Search {
    #[default]
    Idle,
    Waiting,
    Running(u64),
    Done,
    Failed(anyhow::Error),
}
#[derive(Default)]
pub(super) struct Session {
    serial: u64,
    pub catalog: Option<Arc<Catalog>>,
    load: Option<Load>,
    load_error: Option<String>,
    search: Search,
    pub results: Items,
    source_errors: BTreeMap<PathBuf, String>,
    pending_results: Items,
    pub remember_sources: bool,
}
impl Session {
    pub fn inputs(&self) -> &[SourceData] {
        self.catalog.as_ref().map_or(&[], |c| c.inputs())
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.load.as_ref().map_or_else(
            || {
                self.inputs()
                    .iter()
                    .map(|input| input.path().to_path_buf())
                    .collect()
            },
            |load| {
                load.inputs
                    .iter()
                    .map(|input| input.path().to_path_buf())
                    .collect()
            },
        )
    }
    pub fn new(remember_sources: bool) -> Self {
        Self {
            remember_sources,
            ..Self::default()
        }
    }
    pub fn ticket(&mut self) -> Ticket {
        self.serial += 1;
        Ticket::new(self.serial)
    }
    fn begin_load(
        &mut self,
        paths: Vec<PathBuf>,
        mut reload: Vec<PathBuf>,
    ) -> (Option<u64>, Ticket, Job) {
        let previous = self.load.take().map(|load| {
            for path in load.pending() {
                if !reload.iter().any(|p| p == path) {
                    reload.push(path.to_path_buf());
                }
            }
            load.id
        });
        self.load_error = None;
        let inputs: Arc<[LoadInput]> = paths
            .into_iter()
            .map(|path| {
                if !reload.contains(&path)
                    && let Some(data) = self.inputs().iter().find(|data| data.path() == path)
                {
                    LoadInput::Reuse(data.clone())
                } else {
                    LoadInput::Read(path)
                }
            })
            .collect();
        let ticket = self.ticket();
        let load = Load {
            id: ticket.id,
            inputs: inputs.clone(),
        };
        self.source_errors
            .retain(|path, _| !load.pending().any(|p| p == path));
        self.load = Some(load);
        (previous, ticket, Job::Load { inputs })
    }
    fn reset_search(&mut self) -> Option<u64> {
        self.results.clear();
        self.pending_results.clear();
        if let Search::Running(id) = std::mem::replace(&mut self.search, Search::Idle) {
            Some(id)
        } else {
            None
        }
    }
    fn accept_matches(&mut self, ticket: u64, items: Items) -> bool {
        if !matches!(self.search, Search::Running(id) if id == ticket) {
            return false;
        }
        self.pending_results.extend(items);
        true
    }
    fn finish_search(&mut self, ticket: u64, result: anyhow::Result<()>) -> bool {
        if !matches!(self.search, Search::Running(id) if id == ticket) {
            return false;
        }
        self.search = match result {
            Ok(()) => Search::Done,
            Err(error) => Search::Failed(error),
        };
        true
    }
    pub fn pending(&self, path: &std::path::Path) -> bool {
        self.load
            .as_ref()
            .is_some_and(|load| load.pending().any(|p| p == path))
    }
    pub fn source_error(&self, path: &std::path::Path) -> Option<&str> {
        self.source_errors.get(path).map(String::as_str)
    }
    fn fail_load(&mut self, error: String) {
        if let Some(load) = self.load.take() {
            for path in load.pending() {
                if self.inputs().iter().any(|source| source.path() == path) {
                    self.source_errors.insert(path.to_path_buf(), error.clone());
                }
            }
        }
        self.load_error = Some(error);
    }
    fn accept_load(&mut self, id: u64, result: Result<Option<Arc<Catalog>>, String>) -> bool {
        if !self.load.as_ref().is_some_and(|load| load.id == id) {
            return false;
        }
        match result {
            Ok(snapshot) => {
                self.load = None;
                self.source_errors.retain(|path, _| {
                    snapshot
                        .as_ref()
                        .is_some_and(|c| c.inputs().iter().any(|i| i.path() == path))
                });
                self.catalog = snapshot;
            }
            Err(error) => self.fail_load(error),
        }
        true
    }
    pub fn status(&self) -> String {
        if let Some(message) = self.load_error.as_ref() {
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
pub(super) fn paths(ctx: &Context) -> Vec<PathBuf> {
    ctx.with_session(|session| session.paths())
}
pub(super) fn next_ticket(ctx: &Context) -> Ticket {
    ctx.with_session(|session| session.ticket())
}
pub(super) fn pending(ctx: &Context, path: &std::path::Path) -> bool {
    ctx.with_session(|session| session.pending(path))
}
pub(super) fn contains_path(ctx: &Context, path: &std::path::Path) -> bool {
    ctx.with_session(|session| session.inputs().iter().any(|source| source.path() == path))
}
pub(super) fn status(ctx: &Context) -> String {
    ctx.with_session(|session| session.status())
}
pub(super) fn current_catalog(ctx: &Context) -> Option<Arc<Catalog>> {
    ctx.with_session(|session| session.catalog.clone())
}
pub(super) struct SourceState {
    pub path: PathBuf,
    pub len: usize,
    pub error: Option<String>,
    pub pending: bool,
}
pub(super) fn source_states(ctx: &Context) -> Vec<SourceState> {
    ctx.with_session(|session| {
        session
            .inputs()
            .iter()
            .map(|source| {
                let path = source.path();
                SourceState {
                    path: path.to_path_buf(),
                    len: source.len(),
                    error: session.source_error(path).map(str::to_owned),
                    pending: session.pending(path),
                }
            })
            .collect()
    })
}
pub(super) fn reload(ctx: &Context, paths: Vec<PathBuf>, reload: Vec<PathBuf>) {
    let (previous, ticket, job) = ctx.with_session(|session| session.begin_load(paths, reload));
    let result = ctx.replace_job(previous, ticket, job);
    if let Err(e) = result {
        ctx.with_session(|session| session.fail_load(format!("{e:#}")));
    }
    super::status::refresh_status(ctx);
    super::sources::refresh(ctx);
}

pub(super) fn input_changed(ctx: &Context) {
    super::status::begin_status(ctx, super::status::StatusContext::Catalog);
    let (input, composing) = super::panel::cancel_debounce(ctx);
    let cancelled = ctx.with_session(|session| {
        let cancelled = session.reset_search();
        session.load_error = None;
        cancelled
    });
    if let Some(id) = cancelled {
        ctx.cancel_job(id);
    }
    update_results(ctx);
    if !composing && !text(input).trim().is_empty() {
        let state = match super::panel::debounce(ctx, input) {
            Ok(()) => Search::Waiting,
            Err(e) => Search::Failed(e),
        };
        ctx.with_session(|session| session.search = state);
    }
    super::status::refresh_status(ctx);
}
pub(super) fn search(ctx: &Context) {
    ctx.with_session(|session| {
        session.load_error = None;
    });
    submit_search(ctx);
}
fn submit_search(ctx: &Context) {
    let (input, composing) = super::panel::cancel_debounce(ctx);
    if composing {
        return;
    }
    let query = text(input).trim().to_owned();
    let mode = super::panel::search_mode(ctx);
    let categories = settings::categories(ctx);
    let mut submitted = None;
    let cancelled = ctx.with_session(|session| {
        let s = &mut *session;
        let cancelled = s.reset_search();
        let Some(catalog) = s.catalog.clone().filter(|_| !query.is_empty()) else {
            return cancelled;
        };
        let compiled = match winlex_core::CompiledQuery::compile(&query, mode, false) {
            Ok(compiled) => compiled,
            Err(error) => {
                let diagnostic =
                    if let Some(error) = error.downcast_ref::<winlex_core::QueryError>() {
                        use winlex_core::QueryErrorKind;
                        let label = if error.incomplete {
                            IDS_INCOMPLETE_CONDITION_QUERY
                        } else {
                            match error.kind {
                                QueryErrorKind::Syntax => IDS_INVALID_CONDITION_QUERY,
                                QueryErrorKind::Field => IDS_QUERY_FIELD,
                                QueryErrorKind::Operator => IDS_QUERY_OPERATOR,
                                QueryErrorKind::Value => IDS_QUERY_VALUE,
                                QueryErrorKind::Limit => IDS_QUERY_LIMIT,
                            }
                        };
                        let start = query[..error.span.start].encode_utf16().count();
                        let end = query[..error.span.end].encode_utf16().count();
                        format!("{} ({start}..{end})", resources::text(label))
                    } else {
                        resources::text(IDS_QUERY_LIMIT)
                    };
                s.search = Search::Failed(anyhow::anyhow!(diagnostic));
                return cancelled;
            }
        };
        let ticket = s.ticket();
        s.search = Search::Running(ticket.id);
        submitted = Some((
            ticket,
            Job::Search {
                catalog,
                compiled: Box::new(compiled),
                categories,
            },
        ));
        cancelled
    });
    if let Some(id) = cancelled {
        ctx.cancel_job(id);
    }
    let sent = match submitted {
        Some((ticket, job)) => ctx.replace_job(None, ticket, job),
        None => Ok(()),
    };
    update_results(ctx);
    if let Err(e) = sent {
        ctx.with_session(|session| session.search = Search::Failed(e));
    }
    super::status::refresh_status(ctx);
}
fn results_list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(frame::hwnd(ctx), RESULTS as i32) }
}
fn update_results(ctx: &Context) {
    let hwnd = results_list(ctx);
    let count = ctx.with_session(|session| session.results.len());
    send(hwnd, LVM_SETITEMCOUNT, count, 0);
    crate::list::fit(hwnd);
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 0);
    }
}

fn merge_results(results: &mut Items, batch: &mut Items, tracked: &mut [isize; 4]) {
    let original = tracked.map(|index| {
        results
            .get(index as usize)
            .map(|item| std::ptr::from_ref(&**item))
    });
    results.append(batch);
    results.sort_by(|a, b| a.compare(b));
    for (index, item) in tracked.iter_mut().zip(original) {
        if let Some(item) = item {
            *index = results
                .iter()
                .position(|result| std::ptr::eq(&**result, item))
                .unwrap() as isize;
        }
    }
}

pub(super) fn flush_results(ctx: &Context) {
    if !ctx.with_session(|session| !session.pending_results.is_empty()) {
        super::status::refresh_status(ctx);
        return;
    }
    let hwnd = results_list(ctx);
    let mut state = crate::list::State::capture(hwnd);
    ctx.with_session(|session| {
        let s = &mut *session;
        merge_results(&mut s.results, &mut s.pending_results, &mut state.indices);
    });
    state.apply(hwnd, ctx.with_session(|session| session.results.len()));
    super::sidebar::panel(ctx, super::panel::SEARCH_PANEL)
        .list
        .refresh_tooltip();
    super::status::refresh_status(ctx);
}
pub(super) fn row(ctx: &Context, index: usize, visit: &mut dyn FnMut(&crate::list::Display)) {
    ctx.with_session(|session| {
        if let Some(item) = session.results.get(index) {
            visit(&item.display);
        }
    });
}
pub(super) fn row_text(ctx: &Context, row: usize) -> Vec<u16> {
    ctx.with_session(|session| {
        let Some(e) = session.results.get(row) else {
            return Vec::new();
        };
        // Results are cleared when the catalog changes; each ID belongs to this snapshot.
        let catalog = session.catalog.as_ref().expect("search result snapshot");
        let source = &catalog.sources()[e.id.source_id];
        let mut value: Vec<u16> = format!("{}\r\n{} · ", e.full_name, e.kind)
            .encode_utf16()
            .collect();
        value.extend(source.path.file_name().unwrap_or_default().encode_wide());
        value.extend("\r\n".encode_utf16());
        value.extend(e.display.detail.text());
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
pub(super) fn activate(ctx: &Context, row: isize, double: bool) {
    open(ctx, row, if double { Open::Pin } else { Open::Preview });
}
pub(super) fn open_selected(ctx: &Context, mode: Open) {
    let row = crate::list::selected(results_list(ctx)).map_or(-1, |index| index as isize);
    open(ctx, row, mode);
}
pub(super) fn open(ctx: &Context, row: isize, mode: Open) {
    history::visit(ctx, || open_document(ctx, row, mode));
}
fn open_document(ctx: &Context, row: isize, mode: Open) {
    match mode {
        // A preview shows the document but leaves the keyboard in the sidebar.
        Open::Preview => workspace::keep_focus(ctx, || {
            open_now(ctx, row, mode);
        }),
        // An explicit open enters the document: show it, then focus its body.
        Open::Pin | Open::New => {
            if let Some(hwnd) = open_now(ctx, row, mode)
                && let Some(document) = workspace::view(ctx, hwnd)
            {
                document.focus();
            }
        }
    }
}
fn open_now(ctx: &Context, row: isize, mode: Open) -> Option<HWND> {
    if row < 0 {
        return None;
    }
    let item = ctx.with_session(|session| {
        session
            .results
            .get(row as usize)
            .map(|e| (e.id, session.catalog.clone()))
    });
    let (id, Some(catalog)) = item? else {
        return None;
    };
    let preferred = settings::preferred_architecture(ctx);
    // A new result keeps the architecture of the declaration being read.
    let current = if mode == Open::New {
        None
    } else {
        metadata::binding(ctx, workspace::active(ctx)).map(|binding| binding.architecture)
    };
    let binding = super::status::report(
        ctx,
        Binding::resolve(
            catalog,
            id,
            ArchitectureChoice::Preferred { current, preferred },
        ),
    )?;
    open_binding(ctx, binding, mode, None)
}
pub(super) fn open_binding(
    ctx: &Context,
    binding: Binding,
    mode: Open,
    original: Option<u64>,
) -> Option<HWND> {
    let preview = workspace::preview_document(ctx);
    let existing = (mode != Open::New)
        .then(|| {
            original
                .and_then(|id| workspace::by_id(ctx, id))
                .filter(|doc| doc.binding().same_entity(&binding))
                .map(|doc| {
                    if !doc.binding().same_location(&binding) {
                        doc.rebind(binding.clone());
                    }
                    doc.hwnd
                })
                .or_else(|| metadata::find(ctx, &binding))
                .or_else(|| {
                    preview.and_then(|hwnd| {
                        workspace::view(ctx, hwnd)?.rebind(binding.clone());
                        Some(hwnd)
                    })
                })
        })
        .flatten();
    let hwnd = existing.or_else(|| {
        super::status::report(ctx, metadata::open(ctx, binding, mode == Open::Preview))
    })?;
    if mode == Open::Pin {
        workspace::pin_document(ctx, hwnd);
    }
    workspace::show_document(ctx, hwnd);
    Some(hwnd)
}
pub(super) fn worker_stopped(ctx: &Context) {
    ctx.with_session(|session| {
        session.fail_load(resources::text(IDS_WORKER_STOPPED));
        session.search = Search::Idle;
    });
    super::status::set_status(ctx, resources::text(IDS_WORKER_STOPPED));
    super::sources::refresh(ctx);
}
pub(super) fn loaded(ctx: &Context, id: u64, result: anyhow::Result<Option<Arc<Catalog>>>) {
    let result = result.map_err(|error| {
        resources::text(IDS_LOAD_FAILED).replace("{error}", &format!("{error:#}"))
    });
    let committed = result.is_ok();
    if !ctx.with_session(|session| session.accept_load(id, result)) {
        return;
    }
    if committed {
        submit_search(ctx);
        super::status::report(
            ctx,
            ctx.with_session_and_config(|session, config| {
                if session.remember_sources {
                    config
                        .write_key()?
                        .set_paths("AutoLoad", &session.paths())?;
                }
                Ok::<_, anyhow::Error>(())
            }),
        );
        for view in workspace::views(ctx) {
            view.layout();
        }
    }
    super::sources::refresh(ctx);
    super::status::refresh_status(ctx);
}
pub(super) fn matches(ctx: &Context, id: u64, items: Items) {
    ctx.with_session(|session| session.accept_matches(id, items));
}
pub(super) fn search_done(ctx: &Context, id: u64, result: anyhow::Result<()>) {
    ctx.with_session(|session| session.finish_search(id, result));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::ResultItem;
    fn execute_load(ticket: &Ticket, job: Job) -> Result<Option<Arc<Catalog>>, String> {
        let result = std::cell::RefCell::new(None);
        crate::tasks::run(job, ticket, &|completion| {
            let crate::tasks::Completion::Loaded { result: loaded, .. } = completion else {
                panic!("unexpected load completion");
            };
            result.replace(Some(loaded.map_err(|error| format!("{error:#}"))));
            Ok(())
        });
        result.into_inner().unwrap()
    }
    #[test]
    fn replacement_retains_reload_intent_and_failure_preserves_snapshot() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../inputs/Windows.Win32.winmd");
        let path =
            std::env::temp_dir().join(format!("Winlex.LoadPlan.{}.winmd", std::process::id()));
        std::fs::copy(fixture, &path).unwrap();
        let document = Arc::new(Catalog::open_many([&path]).unwrap());
        std::fs::remove_file(&path).unwrap();
        let mut session = Session::new(true);
        session.catalog = Some(document.clone());
        let (_, first, _) = session.begin_load(vec![path.clone()], vec![path.clone()]);
        let (previous, second, job) = session.begin_load(vec![path.clone()], vec![]);
        assert_eq!(previous, Some(first.id));
        assert!(session.pending(&path));
        assert!(!session.accept_load(first.id, Ok(None)));
        let result = execute_load(&second, job);
        assert!(
            result.is_err(),
            "replacement must still reread the deleted source"
        );
        assert!(session.accept_load(second.id, result));
        assert!(Arc::ptr_eq(session.catalog.as_ref().unwrap(), &document));
        assert!(session.source_error(&path).is_some());
        assert!(!session.pending(&path));

        let (_, retry, job) = session.begin_load(vec![path.clone()], vec![]);
        assert!(!session.pending(&path));
        let result = execute_load(&retry, job).expect("unchanged input reuses retained bytes");
        assert!(session.accept_load(retry.id, Ok(result)));
        assert!(!session.accept_load(second.id, Err("late failure".into())));
        assert!(session.load_error.is_none());
        let (_, unload, job) = session.begin_load(vec![], vec![]);
        assert!(session.accept_load(unload.id, execute_load(&unload, job)));
        assert!(session.catalog.is_none());
        assert!(
            document
                .search("CreateFileW", Default::default())
                .unwrap()
                .next()
                .unwrap()
                .is_ok()
        );
    }
    fn hit(token: u32, score: u16) -> Box<ResultItem> {
        Box::new(ResultItem {
            display: Default::default(),
            matched: winlex_core::SearchMatch {
                evidence: Vec::new(),
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
            let mut tracked = [-1; 4];
            for chunk in (1..=200).collect::<Vec<u32>>().chunks(size) {
                let mut batch = chunk
                    .iter()
                    .map(|&id| hit(id, (id * 71 % 23) as u16))
                    .collect();
                merge_results(&mut results, &mut batch, &mut tracked);
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
    fn merging_equal_ranked_rows_keeps_existing_order_and_tracked_occurrences() {
        let tagged = |label: &str| {
            let mut row = hit(10, 100);
            row.display.title = label.encode_utf16().collect();
            row
        };
        let mut results = vec![tagged("first"), tagged("second")];
        let mut tracked = [1, 0, 1, -1];
        for label in ["third", "fourth"] {
            let mut batch = vec![tagged(label), hit(1, 120)];
            merge_results(&mut results, &mut batch, &mut tracked);
            assert!(batch.is_empty());
            for (index, expected) in tracked[..3].iter().zip(["second", "first", "second"]) {
                assert_eq!(
                    String::from_utf16(&results[*index as usize].display.title).unwrap(),
                    expected
                );
            }
            assert_eq!(tracked[3], -1);
        }
        let equal: Vec<_> = results
            .iter()
            .filter(|row| row.id.token == 10)
            .map(|row| String::from_utf16(&row.display.title).unwrap())
            .collect();
        assert_eq!(equal, ["first", "second", "third", "fourth"]);
    }

    #[test]
    fn merge_preserves_selection_focus_mark_and_scroll_rows() {
        let mut results = vec![hit(10, 100), hit(20, 80), hit(30, 60)];
        let mut batch = vec![hit(1, 100), hit(2, 70), hit(3, 120)];
        let mut tracked = [1, 2, 1, 0];
        merge_results(&mut results, &mut batch, &mut tracked);
        for (index, token) in tracked.into_iter().zip([20, 30, 20, 10]) {
            assert_eq!(results[index as usize].id.token, token);
        }
        let mut missing = [-1; 4];
        merge_results(&mut results, &mut vec![hit(99, 200)], &mut missing);
        assert_eq!(missing, [-1; 4]);
    }
    #[test]
    fn late_search_results_cannot_overwrite_new_search_or_load_failure() {
        let mut session = Session::new(true);
        session.search = Search::Running(1);
        assert_eq!(session.reset_search(), Some(1));
        session.search = Search::Running(2);
        session.load_error = Some("load transaction failed".into());
        assert!(!session.finish_search(1, Ok(())));
        assert!(session.accept_matches(2, vec![]));
        assert_eq!(session.status(), "load transaction failed");
        assert!(session.finish_search(2, Ok(())));
        assert_eq!(session.status(), "load transaction failed");
        assert!(!session.finish_search(2, Err(anyhow::anyhow!("late failure"))));
        assert!(matches!(session.search, Search::Done));
    }
}

//! UI transactions and immutable document bindings. HWNDs never cross the worker boundary.
use super::{
    history,
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
    fn accept_matches(&mut self, ticket: u64, items: Items) {
        if !matches!(self.search, Search::Running(id) if id == ticket) {
            return;
        }
        self.pending_results.extend(items);
    }
    fn finish_search(&mut self, ticket: u64, result: anyhow::Result<()>) {
        if !matches!(self.search, Search::Running(id) if id == ticket) {
            return;
        }
        self.search = match result {
            Ok(()) => Search::Done,
            Err(error) => Search::Failed(error),
        };
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
    fn search_status(&self) -> String {
        match &self.search {
            Search::Idle => resources::text(if self.catalog.is_some() {
                IDS_ENTER_QUERY
            } else {
                IDS_EMPTY_CATALOG
            }),
            Search::Waiting => resources::text(IDS_SEARCHING),
            Search::Running(_) => {
                resources::format(IDS_SEARCH_PROGRESS, &[&self.results.len().to_string()])
            }
            Search::Done => {
                resources::format(IDS_SEARCH_STATUS, &[&self.results.len().to_string()])
            }
            Search::Failed(e) => format!("{e:#}"),
        }
    }
    fn source_status(&self) -> String {
        if let Some(error) = &self.load_error {
            return error.clone();
        }
        if self.load.is_some() {
            return resources::text(IDS_LOADING);
        }
        resources::format(IDS_SOURCE_STATUS, &[&self.inputs().len().to_string()])
    }
    pub fn empty_status(&self) -> Option<String> {
        if !self.inputs().is_empty() {
            return None;
        }
        Some(if self.load_error.is_some() || self.load.is_some() {
            self.source_status()
        } else if !matches!(self.search, Search::Idle) {
            self.search_status()
        } else {
            resources::text(IDS_METADATA_EMPTY)
        })
    }
}
pub(super) fn paths(ctx: &Context) -> Vec<PathBuf> {
    ctx.with_session(|session| session.paths())
}
pub(super) fn next_ticket(ctx: &Context) -> Ticket {
    ctx.with_session_mut(|session| session.ticket())
}
pub(super) fn pending(ctx: &Context, path: &std::path::Path) -> bool {
    ctx.with_session(|session| session.pending(path))
}
pub(super) fn contains_path(ctx: &Context, path: &std::path::Path) -> bool {
    ctx.with_session(|session| session.inputs().iter().any(|source| source.path() == path))
}
pub(super) fn search_status(ctx: &Context) -> String {
    ctx.with_session(|session| session.search_status())
}
pub(super) fn source_status(ctx: &Context) -> String {
    ctx.with_session(|session| session.source_status())
}
pub(super) fn current_catalog(ctx: &Context) -> Option<Arc<Catalog>> {
    ctx.with_session(|session| session.catalog.clone())
}
pub(super) fn reload(ctx: &Context, paths: Vec<PathBuf>, reload: Vec<PathBuf>) {
    let (previous, ticket, job) = ctx.with_session_mut(|session| session.begin_load(paths, reload));
    let result = ctx.replace_job(previous, ticket, job);
    if let Err(e) = result {
        ctx.with_session_mut(|session| session.fail_load(format!("{e:#}")));
    }
    super::status::refresh_status(ctx);
    super::sources::refresh(ctx);
}

pub(super) fn input_changed(ctx: &Context) {
    super::status::begin_status(
        ctx,
        super::status::StatusContext::Panel(super::panel::PanelId::Search),
    );
    let (input, composing) = super::search::cancel_debounce(ctx);
    let cancelled = ctx.with_session_mut(|session| session.reset_search());
    if let Some(id) = cancelled {
        ctx.cancel_job(id);
    }
    update_results(ctx);
    if !composing && !text(input).trim().is_empty() {
        let state = match super::search::debounce(ctx, input) {
            Ok(()) => Search::Waiting,
            Err(e) => Search::Failed(e),
        };
        ctx.with_session_mut(|session| session.search = state);
    }
    super::status::refresh_status(ctx);
}
pub(super) fn search(ctx: &Context) {
    let (input, composing) = super::search::cancel_debounce(ctx);
    if composing {
        return;
    }
    let query = text(input).trim().to_owned();
    let mode = super::search::search_mode(ctx);
    let categories = settings::categories(ctx);
    let mut submitted = None;
    let cancelled = ctx.with_session_mut(|session| {
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
        ctx.with_session_mut(|session| session.search = Search::Failed(e));
    }
    super::status::refresh_status(ctx);
}
fn results_list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), RESULTS as i32) }
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
    results.sort_by(|a, b| a.matched.compare(&b.matched));
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
    ctx.with_session_mut(|session| {
        let s = &mut *session;
        merge_results(&mut s.results, &mut s.pending_results, &mut state.indices);
    });
    state.apply(hwnd, ctx.with_session(|session| session.results.len()));
    super::sidebar::panel(ctx, super::panel::PanelId::Search)
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
        let matched = &e.matched;
        let source = &catalog.sources()[matched.id.source_id];
        let mut value: Vec<u16> = format!("{}\r\n{} · ", matched.full_name, matched.kind)
            .encode_utf16()
            .collect();
        value.extend(source.path.file_name().unwrap_or_default().encode_wide());
        value.extend("\r\n".encode_utf16());
        value.extend(e.display.detail.text());
        value.extend("\r\n".encode_utf16());
        value.extend(source.path.as_os_str().encode_wide());
        value.extend(format!("\r\n{}", matched.id).encode_utf16());
        value
    })
}
/// Resolve an entity in the active document's architecture, falling back to the preference.
fn preferred_binding(
    ctx: &Context,
    catalog: Arc<Catalog>,
    id: winlex_core::EntityId,
    current: Option<winlex_core::Architecture>,
) -> anyhow::Result<Binding> {
    Binding::resolve(
        catalog,
        id,
        ArchitectureChoice::Preferred {
            current,
            preferred: settings::preferred_architecture(ctx),
        },
    )
}
pub(super) fn context_menu(ctx: &Context, hwnd: HWND, location: isize) -> anyhow::Result<()> {
    let Some((row, point)) = crate::list::context_target(hwnd, location) else {
        return Ok(());
    };
    let Some((catalog, id, name)) = ctx.with_session(|session| {
        let item = &session.results.get(row)?.matched;
        Some((session.catalog.clone()?, item.id, item.full_name.clone()))
    }) else {
        return Ok(());
    };
    let current =
        workspace::view(ctx, workspace::active(ctx)).map(|doc| doc.binding().architecture);
    let binding = preferred_binding(ctx, catalog, id, current)?;
    let value = super::result_value::read(&binding.catalog, id, binding.architecture)?;
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_RESULT_NEW), true)?;
    menu.separator()?;
    menu.item(3, resources::wide_text(IDS_RESULT_COPY_NAME), true)?;
    if let Some(value) = &value {
        menu.item(4, resources::wide_text(IDS_RESULT_COPY_VALUE), true)?;
        if value.hexadecimal.is_some() {
            menu.item(5, resources::wide_text(IDS_RESULT_COPY_HEX), true)?;
        }
    }
    let actions = super::tools::actions(value.as_ref());
    if !actions.is_empty() {
        menu.separator()?;
        for (index, action) in actions.iter().enumerate() {
            menu.item(100 + index as u16, resources::wide_text(action.label), true)?;
        }
    }
    match menu.track(hwnd, point) {
        command @ (1 | 2) => {
            let mode = if command == 1 { Open::Pin } else { Open::New };
            open(ctx, Some(row), mode);
        }
        3 => copy_unicode(hwnd, &name.encode_utf16().collect::<Vec<_>>())?,
        command @ (4 | 5) => {
            if let Some(value) = &value {
                let text = if command == 4 {
                    Some(&value.text)
                } else {
                    value.hexadecimal.as_ref()
                };
                if let Some(text) = text {
                    copy_unicode(hwnd, &text.encode_utf16().collect::<Vec<_>>())?;
                }
            }
        }
        command if command >= 100 => {
            if let Some(action) = actions.get(usize::from(command - 100)) {
                super::tools::open_with(ctx, action.request.clone());
            }
        }
        _ => {}
    }
    Ok(())
}
/// How a result reaches a document: transient preview, pinned open, or forced duplicate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Open {
    Preview,
    Pin,
    New,
}
pub(super) fn activate(ctx: &Context, row: usize, double: bool) {
    let mode = if double { Open::Pin } else { Open::Preview };
    open(ctx, Some(row), mode);
}
pub(super) fn open_selected(ctx: &Context, mode: Open) {
    let row = crate::list::selected(results_list(ctx));
    open(ctx, row, mode);
}
pub(super) fn open(ctx: &Context, row: Option<usize>, mode: Open) {
    history::visit(ctx, || match mode {
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
    });
}
fn open_now(ctx: &Context, row: Option<usize>, mode: Open) -> Option<HWND> {
    let row = row?;
    let item = ctx.with_session(|session| {
        session
            .results
            .get(row)
            .map(|e| (e.matched.id, session.catalog.clone()))
    });
    let (id, Some(catalog)) = item? else {
        return None;
    };
    // A new result keeps the architecture of the declaration being read.
    let current = if mode == Open::New {
        None
    } else {
        workspace::view(ctx, workspace::active(ctx)).map(|doc| doc.binding().architecture)
    };
    let binding = super::status::report(ctx, preferred_binding(ctx, catalog, id, current))?;
    open_binding(ctx, binding, mode, None)
}
pub(super) fn open_binding(
    ctx: &Context,
    binding: Binding,
    mode: Open,
    original: Option<u64>,
) -> Option<HWND> {
    let existing = if mode == Open::New {
        None
    } else {
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
                workspace::preview_document(ctx).map(|doc| {
                    doc.rebind(binding.clone());
                    doc.hwnd
                })
            })
    };
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
    ctx.with_session_mut(|session| {
        session.fail_load(resources::text(IDS_WORKER_STOPPED));
        session.search = Search::Idle;
    });
    super::status::show_notice(ctx, resources::text(IDS_WORKER_STOPPED));
    super::sources::refresh(ctx);
}
pub(super) fn loaded(ctx: &Context, id: u64, result: anyhow::Result<Option<Arc<Catalog>>>) {
    let result =
        result.map_err(|error| resources::format(IDS_LOAD_FAILED, &[&format!("{error:#}")]));
    let committed = result.is_ok();
    if !ctx.with_session_mut(|session| session.accept_load(id, result)) {
        return;
    }
    if committed {
        search(ctx);
        if ctx.with_session(|session| session.remember_sources) {
            let paths = paths(ctx);
            super::status::report(ctx, ctx.with_config(|config| config.write_paths(&paths)));
        }
        for view in workspace::views(ctx) {
            view.layout();
        }
    }
    super::sources::refresh(ctx);
    super::status::refresh_status(ctx);
}
pub(super) fn matches(ctx: &Context, id: u64, items: Items) {
    ctx.with_session_mut(|session| session.accept_matches(id, items));
}
pub(super) fn search_done(ctx: &Context, id: u64, result: anyhow::Result<()>) {
    ctx.with_session_mut(|session| session.finish_search(id, result));
}

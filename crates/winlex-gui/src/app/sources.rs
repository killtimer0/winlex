//! Source locations, loading intent and inspection documents.
use super::{
    catalog,
    metadata::{self, Binding},
    read, workspace, write,
};
use crate::{
    input,
    native::*,
    resources::{self, *},
    tasks::{Completion, Discovered, Job, Output},
    worker::Ticket,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use winlex_core::Catalog;

#[derive(Default)]
pub(super) struct Sources {
    pub settings: Settings,
    pub entries: Vec<Discovered>,
    pub extra: Vec<PathBuf>,
    ticket: Option<Ticket>,
}
#[derive(Default)]
pub(super) struct Settings {
    pub files: Vec<PathBuf>,
    pub directories: Vec<PathBuf>,
    pub auto_load: Vec<PathBuf>,
}
impl Settings {
    pub fn load(config: &crate::config::Config) -> Self {
        config.key.as_ref().map_or_else(Self::default, |key| Self {
            files: key.paths("Files"),
            directories: key.paths("Directories"),
            auto_load: key.paths("AutoLoad"),
        })
    }
    fn set_source_intent(
        &mut self,
        config: &crate::config::Config,
        remove: &[PathBuf],
        add: &[PathBuf],
    ) -> Result<(), String> {
        let mut auto_load = self.auto_load.clone();
        auto_load.retain(|p| !remove.contains(p));
        let mut files = self.files.clone();
        for path in add {
            if !auto_load.contains(path) {
                auto_load.push(path.clone());
            }
            if !files.contains(path) {
                files.push(path.clone());
            }
        }
        let key = config.write_key()?;
        key.set_paths("Files", &files)?;
        key.set_paths("AutoLoad", &auto_load)?;
        self.files = files;
        self.auto_load = auto_load;
        Ok(())
    }
    fn remember_locations(
        &mut self,
        config: &crate::config::Config,
        paths: &[PathBuf],
    ) -> Result<(), String> {
        let mut files = self.files.clone();
        let mut directories = self.directories.clone();
        for path in paths {
            let path = std::path::absolute(path).map_err(|e| e.to_string())?;
            let list = if path.is_dir() {
                &mut directories
            } else {
                &mut files
            };
            if !list.contains(&path) {
                list.push(path);
            }
        }
        let key = config.write_key()?;
        key.set_paths("Files", &files)?;
        key.set_paths("Directories", &directories)?;
        self.files = files;
        self.directories = directories;
        Ok(())
    }
    fn forget_location(
        &mut self,
        config: &crate::config::Config,
        path: &Path,
    ) -> Result<(), String> {
        let files: Vec<_> = self
            .files
            .iter()
            .filter(|p| p.as_path() != path)
            .cloned()
            .collect();
        let directories: Vec<_> = self
            .directories
            .iter()
            .filter(|p| p.as_path() != path)
            .cloned()
            .collect();
        let key = config.write_key()?;
        key.set_paths("Files", &files)?;
        key.set_paths("Directories", &directories)?;
        self.files = files;
        self.directories = directories;
        Ok(())
    }
}
impl Sources {
    pub fn cancel(&mut self) {
        if let Some(t) = self.ticket.take() {
            t.cancel();
        }
    }
}
pub(super) fn refresh() {
    let result = write(|a| {
        let s = &mut a.session;
        let previous = s.sources.ticket.take().map(|t| t.id);
        let ticket = s.ticket();
        let mut files = s.sources.settings.files.clone();
        files.extend(s.sources.settings.auto_load.clone());
        files.extend(s.sources.extra.clone());
        files.extend(s.inputs().iter().map(|s| s.path().to_path_buf()));
        let mut directories = s.sources.settings.directories.clone();
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            directories.push(dir.to_path_buf());
        }
        s.sources.ticket = Some(ticket.clone());
        let result = a
            .worker
            .replace(previous, ticket, Job::Discover { files, directories });
        if result.is_err() {
            s.sources.ticket = None;
        }
        result
    });
    if let Err(e) = result {
        catalog::set_status(e);
    }
}
pub(super) fn apply_discovery(c: Completion) {
    if !read(|a| {
        a.session
            .sources
            .ticket
            .as_ref()
            .is_some_and(|t| t.id == c.id)
    }) {
        return;
    }
    write(|a| a.session.sources.ticket = None);
    let files = match c.result {
        Ok(Output::Discovered(files)) => files,
        Err(e) => {
            catalog::set_status(e);
            return;
        }
        _ => unreachable!(),
    };
    let list = read(|a| a.sidebar.panel(super::panel::SOURCES_PANEL).list());
    let selected = [LVNI_SELECTED, LVNI_FOCUSED].map(|flag| {
        let row = send(list, LVM_GETNEXTITEM, usize::MAX, flag as isize);
        read(|a| {
            a.session
                .sources
                .entries
                .get(row as usize)
                .map(|e| e.path.clone())
        })
    });
    write(|a| {
        a.session.sources.entries = files;
        for path in &a.session.sources.settings.directories {
            if !a.session.sources.entries.iter().any(|e| &e.path == path) {
                a.session.sources.entries.push(Discovered {
                    missing: false,
                    path: path.clone(),
                    size: None,
                    error: None,
                });
            }
        }
    });
    refresh_views();
    let mut item = LVITEMW {
        stateMask: LVIS_SELECTED | LVIS_FOCUSED,
        ..unsafe { std::mem::zeroed() }
    };
    send(
        list,
        LVM_SETITEMSTATE,
        usize::MAX,
        &item as *const _ as isize,
    );
    for (path, flag) in selected.into_iter().zip([LVIS_SELECTED, LVIS_FOCUSED]) {
        if let Some(path) = path
            && let Some(row) = read(|a| {
                a.session
                    .sources
                    .entries
                    .iter()
                    .position(|e| e.path == path)
            })
        {
            item.stateMask = flag;
            item.state = flag;
            send(list, LVM_SETITEMSTATE, row, &item as *const _ as isize);
        }
    }
}
pub(super) fn cell(row: usize, detail: bool) -> String {
    read(|a| {
        let Some(e) = a.session.sources.entries.get(row) else {
            return String::new();
        };
        let loaded = a.session.inputs().iter().any(|s| s.path() == e.path);
        let failure = e.error.as_ref();
        let state = resources::text(if a.session.pending(&e.path) {
            IDS_LOADING
        } else if e.missing {
            IDS_MISSING
        } else if failure.is_some() {
            IDS_FAILED
        } else if a.session.sources.settings.directories.contains(&e.path) {
            IDS_DIRECTORY
        } else if loaded {
            IDS_LOADED
        } else {
            IDS_NOT_LOADED
        });
        if detail {
            format!(
                "{}\r\n{}\r\n{}",
                e.path.display(),
                state,
                failure.map(String::as_str).unwrap_or("")
            )
        } else {
            format!(
                "{}\n{} · {}",
                e.path.file_name().unwrap_or_default().to_string_lossy(),
                state,
                e.size.map(|s| format!("{s} B")).unwrap_or_default()
            )
        }
    })
}
pub(super) fn add(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    match write(|a| {
        a.session
            .sources
            .settings
            .remember_locations(&a.config, &paths)
    }) {
        Ok(()) => refresh(),
        Err(e) => catalog::set_status(e),
    }
}
pub(super) fn choose_add() {
    match input::choose_files(read(|a| a.frame), read(|a| a.system_version), true) {
        Ok(paths) => add(paths),
        Err(e) => catalog::set_status(e),
    }
}
pub(super) fn choose_directory() {
    match input::choose_directory(read(|a| a.frame), read(|a| a.system_version)) {
        Ok(Some(path)) => add(vec![path]),
        Ok(None) => {}
        Err(e) => catalog::set_status(e),
    }
}
pub(super) fn forget_selected() {
    let list = read(|a| a.sidebar.panel(super::panel::SOURCES_PANEL).list());
    let row = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    let path = read(|a| {
        a.session
            .sources
            .entries
            .get(row as usize)
            .map(|e| e.path.clone())
    });
    if let Some(path) = path {
        match write(|a| a.session.sources.settings.forget_location(&a.config, &path)) {
            Ok(()) => refresh(),
            Err(e) => catalog::set_status(e),
        }
    }
}
pub(super) fn open_selected() {
    let list = read(|a| a.sidebar.panel(super::panel::SOURCES_PANEL).list());
    let row = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    open_row(row);
}
pub(super) fn open_row(row: isize) {
    let path = read(|a| {
        a.session
            .sources
            .entries
            .get(row as usize)
            .map(|e| e.path.clone())
    });
    if let Some(path) = path {
        open(path);
    }
}
pub(super) fn open(path: PathBuf) {
    workspace::keep_focus(|| open_now(path));
}
fn open_now(path: PathBuf) {
    if read(|a| a.session.sources.settings.directories.contains(&path)) {
        catalog::set_status(format!(
            "{}: {}",
            resources::text(IDS_DIRECTORY),
            path.display()
        ));
        return;
    }
    let existing =
        metadata::find(|_, d| matches!(&d.binding, Binding::Source { path: p, .. } if p == &path));
    if let Some(h) = existing {
        workspace::show_document(h);
        return;
    }
    let title = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if let Err(e) = metadata::open(
        &title,
        Binding::Source {
            path,
            catalog: None,
        },
        false,
    ) {
        catalog::set_status(e);
    }
}
pub(super) fn inspected(hwnd: HWND, snapshot: Arc<Catalog>) {
    let source = &snapshot.sources()[0];
    write(|a| {
        if let Some(e) = a
            .session
            .sources
            .entries
            .iter_mut()
            .find(|e| e.path == source.path)
        {
            e.error = None;
            e.missing = false;
            e.size = Some(snapshot.inputs()[0].len() as u64);
        }
    });
    metadata::edit(hwnd, |d| {
        if let Binding::Source { catalog, .. } = &mut d.binding {
            *catalog = Some(snapshot);
        }
    });
    refresh_views();
}
pub(super) fn inspection_failed(path: &Path, error: &str) {
    write(|a| {
        if let Some(e) = a
            .session
            .sources
            .entries
            .iter_mut()
            .find(|e| e.path == path)
        {
            e.error = Some(error.into());
        }
    });
    refresh_views();
}
pub(super) fn details(path: &Path, snapshot: &Catalog) -> String {
    let failure = read(|a| {
        let entry = a.session.sources.entries.iter().find(|e| e.path == path);
        entry
            .and_then(|e| e.error.as_ref())
            .cloned()
            .unwrap_or_default()
    });
    let active = read(|a| a.session.inputs().iter().any(|s| s.path() == path));
    format!(
        "{}\r\n{} B\r\n{}: {}\r\n{}\r\n\r\n{}",
        path.display(),
        snapshot.inputs()[0].len(),
        resources::text(IDS_LOAD_STATE),
        resources::text(if active { IDS_LOADED } else { IDS_NOT_LOADED }),
        failure,
        format!("{:#}", winlex_format::summary(snapshot)).replace('\n', "\r\n")
    )
}
pub(super) fn refresh_views() {
    let (list, count) = read(|a| {
        (
            a.sidebar.panel(super::panel::SOURCES_PANEL).list(),
            a.session.sources.entries.len(),
        )
    });
    send(list, LVM_SETITEMCOUNT, count, LVSICF_NOSCROLL as isize);
    super::panel::fit(list);
    unsafe {
        InvalidateRect(list, std::ptr::null(), 0);
    }
    metadata::refresh_all();
}
pub(super) fn action(hwnd: HWND, command: u16) {
    let Some(Binding::Source { path, .. }) = metadata::get(hwnd, |_, d| d.binding.clone()) else {
        return;
    };
    if command == REFRESH_SOURCE {
        catalog::request_document(hwnd);
        return;
    }
    let mut paths = catalog::paths();
    let (remove, add) = match command {
        REMOVE_SOURCE => (vec![path.clone()], vec![]),
        REPLACE_SOURCE => {
            match input::choose_files(read(|a| a.frame), read(|a| a.system_version), false) {
                Ok(p) if !p.is_empty() => (vec![path.clone()], p),
                Ok(_) => return,
                Err(e) => {
                    catalog::set_status(e);
                    return;
                }
            }
        }
        _ => (vec![], vec![path.clone()]),
    };
    if let Err(e) = write(|a| {
        a.session
            .sources
            .settings
            .set_source_intent(&a.config, &remove, &add)
    }) {
        catalog::set_status(e);
        return;
    }
    paths.retain(|p| !remove.contains(p));
    for p in &add {
        if !paths.contains(p) {
            paths.push(p.clone());
        }
    }
    catalog::reload(
        paths,
        if command == RELOAD_SOURCE {
            vec![path]
        } else {
            vec![]
        },
    );
}

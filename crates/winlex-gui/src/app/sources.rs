//! Discovered locations are distinct from active content identities and pinned document versions.
use super::{
    Architecture,
    catalog::{self, Binding},
    document, edit_document, read, workspace, write,
};
use crate::{
    input,
    native::*,
    resources::{self, *},
    worker::{Completion, Discovered, Job, Output, Ticket},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use winlex_core::Catalog;

#[derive(Default)]
pub(super) struct Sources {
    pub entries: Vec<Discovered>,
    pub extra: Vec<PathBuf>,
    ticket: Option<Ticket>,
    pub pending: Vec<PathBuf>,
    pub errors: std::collections::HashMap<PathBuf, String>,
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
        s.sources.cancel();
        let ticket = s.ticket();
        let mut files = a.config.files.clone();
        files.extend(a.config.auto_load.clone());
        files.extend(s.sources.extra.clone());
        files.extend(s.inputs.iter().map(|s| s.path().to_path_buf()));
        let mut directories = a.config.directories.clone();
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            directories.push(dir.to_path_buf());
        }
        s.sources.ticket = Some(ticket.clone());
        let result = s
            .worker
            .submit(ticket, Job::Discover { files, directories });
        if result.is_err() {
            s.sources.ticket = None;
        }
        result
    });
    if let Err(e) = result {
        catalog::set_status(e);
    }
}
pub(super) fn apply_discovery(c: &Completion) -> bool {
    if !read(|a| {
        a.session
            .sources
            .ticket
            .as_ref()
            .is_some_and(|t| t.id == c.id)
    }) {
        return false;
    }
    write(|a| {
        a.session.sources.ticket = None;
        if let Ok(Output::Discovered(files)) = &c.result {
            a.session.sources.entries = files.clone();
            for path in &a.config.directories {
                if !a.session.sources.entries.iter().any(|e| &e.path == path) {
                    a.session.sources.entries.push(Discovered {
                        missing: false,
                        path: path.clone(),
                        size: None,
                        error: None,
                        id: None,
                    });
                }
            }
        }
    });
    if let Err(e) = &c.result {
        catalog::set_status(e.clone());
    }
    refresh_views();
    true
}
pub(super) fn cell(row: usize, detail: bool) -> String {
    read(|a| {
        let Some(e) = a.session.sources.entries.get(row) else {
            return String::new();
        };
        let loaded = a
            .session
            .inputs
            .iter()
            .any(|s| s.path() == e.path || e.id.as_deref() == Some(s.id()));
        let failure = a.session.sources.errors.get(&e.path).or(e.error.as_ref());
        let state = resources::text(if a.session.sources.pending.contains(&e.path) {
            IDS_LOADING
        } else if e.missing {
            IDS_MISSING
        } else if failure.is_some() {
            IDS_FAILED
        } else if a.config.directories.contains(&e.path) {
            IDS_DIRECTORY
        } else if loaded {
            IDS_LOADED
        } else {
            IDS_NOT_LOADED
        });
        if detail {
            format!(
                "{}\r\n{}\r\n{}\r\n{}",
                e.path.display(),
                state,
                e.id.as_deref().unwrap_or(""),
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
    match write(|a| a.config.remember_locations(&paths)) {
        Ok(()) => refresh(),
        Err(e) => catalog::set_status(e),
    }
}
pub(super) fn choose_add() {
    match input::choose_files(read(|a| a.frame), true) {
        Ok(paths) => add(paths),
        Err(e) => catalog::set_status(e),
    }
}
pub(super) fn choose_directory() {
    match input::choose_directory(read(|a| a.frame)) {
        Ok(Some(path)) => add(vec![path]),
        Ok(None) => {}
        Err(e) => catalog::set_status(e),
    }
}
pub(super) fn forget_selected() {
    let list = read(|a| a.source_list);
    let row = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    let path = read(|a| {
        a.session
            .sources
            .entries
            .get(row as usize)
            .map(|e| e.path.clone())
    });
    if let Some(path) = path {
        match write(|a| a.config.forget_location(&path)) {
            Ok(()) => refresh(),
            Err(e) => catalog::set_status(e),
        }
    }
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
    if read(|a| a.config.directories.contains(&path)) {
        catalog::set_status(format!(
            "{}: {}",
            resources::text(IDS_DIRECTORY),
            path.display()
        ));
        return;
    }
    let existing = read(|a| {
        a.documents
            .iter()
            .find(|d| matches!(&d.query.binding, Binding::Source { path: p, .. } if p == &path))
            .map(|d| d.hwnd)
    });
    if let Some(h) = existing {
        workspace::activate_document(h);
        return;
    }
    let title = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if let Err(e) = workspace::create_document(
        &title,
        Architecture::X64,
        Binding::Source {
            path,
            catalog: None,
        },
    ) {
        catalog::set_status(e);
    }
}
pub(super) fn inspected(hwnd: HWND, id: u64, snapshot: Arc<Catalog>) {
    if !document(hwnd).is_some_and(|d| d.id == id) {
        return;
    }
    let source = &snapshot.sources()[0];
    write(|a| {
        if let Some(e) = a
            .session
            .sources
            .entries
            .iter_mut()
            .find(|e| e.path == source.native_path)
        {
            e.id = Some(source.id.clone());
            e.error = None;
            e.missing = false;
            e.size = Some(snapshot.inputs()[0].len() as u64);
        }
    });
    edit_document(hwnd, |d| {
        if let Binding::Source { catalog, .. } = &mut d.query.binding {
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
            e.id = None;
        }
    });
    refresh_views();
}
pub(super) fn details(path: &Path, snapshot: &Catalog) -> String {
    let (disk, failure) = read(|a| {
        let entry = a.session.sources.entries.iter().find(|e| e.path == path);
        (
            entry.and_then(|e| e.id.clone()),
            a.session
                .sources
                .errors
                .get(path)
                .or_else(|| entry.and_then(|e| e.error.as_ref()))
                .cloned()
                .unwrap_or_default(),
        )
    });
    let active = read(|a| {
        a.session
            .inputs
            .iter()
            .find(|s| s.path() == path || s.id() == snapshot.sources()[0].id)
            .map(|s| s.id().to_owned())
    });
    format!(
        "{}\r\n{}: {} ({} B)\r\n{}: {}\r\n{}: {}\r\n{}\r\n\r\n{}",
        path.display(),
        resources::text(IDS_INSPECTED_VERSION),
        snapshot.sources()[0].id,
        snapshot.inputs()[0].len(),
        resources::text(IDS_DISK_VERSION),
        disk.as_deref().unwrap_or("—"),
        resources::text(IDS_LOAD_STATE),
        active.as_deref().unwrap_or("—"),
        failure,
        format!("{:#}", snapshot.summary()).replace('\n', "\r\n")
    )
}
pub(super) fn refresh_views() {
    let (list, count, docs) = read(|a| {
        (
            a.source_list,
            a.session.sources.entries.len(),
            a.documents.iter().map(|d| d.hwnd).collect::<Vec<_>>(),
        )
    });
    send(list, LVM_SETITEMCOUNT, count, LVSICF_NOSCROLL as isize);
    super::sidebar::fit(list);
    unsafe {
        InvalidateRect(list, std::ptr::null(), 0);
    }
    for h in docs {
        let Some(doc) = document(h) else { continue };
        if let Binding::Source {
            path,
            catalog: Some(snapshot),
        } = &doc.query.binding
        {
            let content = details(path, snapshot);
            if doc.query.content.as_ref() == content {
                continue;
            }
            edit_document(h, |d| d.query.content = content.into());
        } else if doc.old_version == catalog::old_version(&doc.query.binding) {
            continue;
        }
        super::document::refresh_content(h);
    }
}
pub(super) fn action(hwnd: HWND, command: u16) {
    let Some(doc) = document(hwnd) else { return };
    let Binding::Source {
        path,
        catalog: inspected,
    } = doc.query.binding
    else {
        return;
    };
    if command == REFRESH_SOURCE {
        catalog::request_document(hwnd);
        return;
    }
    let id = inspected.as_ref().map(|c| c.sources()[0].id.as_str());
    let mut paths = catalog::paths();
    let aliases = read(|a| {
        let mut paths = a
            .session
            .inputs
            .iter()
            .filter(|s| s.path() == path || id == Some(s.id()))
            .map(|s| s.path().to_path_buf())
            .collect::<Vec<_>>();
        for e in &a.session.sources.entries {
            if (e.path == path || id.is_some() && e.id.as_deref() == id) && !paths.contains(&e.path)
            {
                paths.push(e.path.clone());
            }
        }
        if !paths.contains(&path) {
            paths.push(path.clone());
        }
        paths
    });
    let (remove, add) = match command {
        REMOVE_SOURCE => (aliases, vec![]),
        REPLACE_SOURCE => match input::choose_files(read(|a| a.frame), false) {
            Ok(p) if !p.is_empty() => (aliases, p),
            Ok(_) => return,
            Err(e) => {
                catalog::set_status(e);
                return;
            }
        },
        _ => (vec![], vec![path.clone()]),
    };
    if let Err(e) = write(|a| a.config.set_source_intent(&remove, &add)) {
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

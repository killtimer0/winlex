//! The source panel projects the committed catalog; opening files is a load transaction.
use super::{catalog, read, write};
use crate::{
    icons::Icon,
    input,
    list::{Display, FoldKind, TextLine, TextSegment, tooltip_field},
    native::*,
    resources::{self, *},
};
use std::{os::windows::ffi::OsStrExt, path::PathBuf};

#[derive(Default)]
pub(super) struct Sources {
    rows: Vec<Row>,
}
struct Row {
    pub path: PathBuf,
    pub display: Display,
    pub text: Vec<u16>,
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    read(|a| {
        if let Some(row) = a.sources.rows.get(index) {
            visit(&row.display);
        }
    });
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    read(|a| {
        a.sources
            .rows
            .get(index)
            .map(|r| r.text.clone())
            .unwrap_or_default()
    })
}
fn rows() -> Vec<Row> {
    read(|a| {
        a.session
            .inputs()
            .iter()
            .map(|source| {
                let path = source.path();
                let failure = a.session.source_error(path);
                let status = resources::text(if a.session.pending(path) {
                    IDS_RELOADING
                } else if failure.is_some() {
                    IDS_RELOAD_FAILED
                } else {
                    IDS_LOADED
                });
                let mut detail = TextLine::default();
                detail.push(TextSegment::plain(&format!(
                    "{status} · {} · ",
                    size(source.len())
                )));
                if let Some(parent) = path.parent() {
                    detail.push(TextSegment::new(
                        parent.as_os_str().encode_wide(),
                        FoldKind::Path,
                    ));
                }
                let exact =
                    resources::text(IDS_SIZE_BYTES).replace("{size}", &source.len().to_string());
                let native_path: Vec<_> = path.as_os_str().encode_wide().collect();
                let title: Vec<_> = path.file_name().unwrap_or_default().encode_wide().collect();
                let mut info = native_path.clone();
                info.extend(format!("\r\n{status} · {exact}").encode_utf16());
                if let Some(error) = failure {
                    info.extend(format!("\r\n{error}").encode_utf16());
                }
                let mut tooltip = title.clone();
                tooltip.extend("\r\n".encode_utf16());
                tooltip_field(&mut tooltip, IDS_TIP_STATUS, status.encode_utf16());
                tooltip_field(&mut tooltip, IDS_TIP_SIZE, exact.encode_utf16());
                tooltip_field(&mut tooltip, IDS_TIP_PATH, native_path);
                if let Some(error) = failure {
                    tooltip_field(&mut tooltip, IDS_TIP_ERROR, error.encode_utf16());
                    tooltip.extend("\r\n".encode_utf16());
                    tooltip.extend(resources::text(IDS_TIP_PREVIOUS_DATA).encode_utf16());
                }
                Row {
                    path: path.to_path_buf(),
                    display: Display {
                        title,
                        detail,
                        icon: Some(Icon::File),
                        tooltip,
                        ..Default::default()
                    },
                    text: info,
                }
            })
            .collect()
    })
}

fn size(bytes: usize) -> String {
    let units = [
        IDS_SIZE_BYTES,
        IDS_SIZE_KB,
        IDS_SIZE_MB,
        IDS_SIZE_GB,
        IDS_SIZE_TB,
    ];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < units.len() {
        value /= 1024.0;
        unit += 1;
    }
    let value = if unit == 0 {
        bytes.to_string()
    } else {
        format!("{value:.1}")
    };
    resources::text(units[unit]).replace("{size}", &value)
}

fn list() -> HWND {
    unsafe { GetDlgItem(read(|a| a.frame), SOURCE_LIST as i32) }
}
fn selected() -> Option<PathBuf> {
    let index = crate::list::selected(list())?;
    read(|a| a.sources.rows.get(index).map(|row| row.path.clone()))
}
pub(super) fn update_actions() {
    let path = selected();
    let reload = path
        .as_ref()
        .is_some_and(|p| !read(|a| a.session.pending(p)));
    let frame = read(|a| a.frame);
    unsafe {
        EnableWindow(GetDlgItem(frame, RELOAD_SOURCE as i32), reload as i32);
        EnableWindow(
            GetDlgItem(frame, REMOVE_SOURCE as i32),
            path.is_some() as i32,
        );
    }
}

pub(super) fn add(files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    super::status::begin_status(super::status::StatusContext::Catalog);
    let mut paths = catalog::paths();
    for file in files {
        let path = match std::path::absolute(file) {
            Ok(path) => path,
            Err(error) => {
                super::status::set_status(error.to_string());
                return;
            }
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    catalog::reload(paths, vec![]);
}

fn choose_add() {
    if let Some(paths) = super::status::report(input::choose(
        read(|a| a.frame),
        read(|a| a.system_version),
        IDS_LOAD_METADATA,
        IDS_WINMD_FILES,
        "*.winmd",
        true,
    )) {
        add(paths);
    }
}

pub(super) fn command(id: u16) -> bool {
    if matches!(
        id,
        LOAD_METADATA | RELOAD_SOURCE | REMOVE_SOURCE | COPY_SOURCE_PATH
    ) {
        super::status::begin_status(super::status::StatusContext::Catalog);
    }
    match id {
        LOAD_METADATA => choose_add(),
        RELOAD_SOURCE | REMOVE_SOURCE | COPY_SOURCE_PATH => {
            if let Some(path) = selected() {
                action_path(id, path);
            }
        }
        _ => return false,
    }
    true
}
fn action_path(command: u16, path: PathBuf) {
    if command == COPY_SOURCE_PATH {
        super::status::report(copy_unicode(
            list(),
            &path.as_os_str().encode_wide().collect::<Vec<_>>(),
        ));
        return;
    }
    if !read(|a| a.session.inputs().iter().any(|s| s.path() == path)) {
        return;
    }
    let mut paths = catalog::paths();
    if command == REMOVE_SOURCE {
        paths.retain(|p| *p != path);
        catalog::reload(paths, vec![]);
    } else if command == RELOAD_SOURCE && !read(|a| a.session.pending(&path)) {
        catalog::reload(paths, vec![path]);
    }
}

pub(super) fn refresh() {
    let list = list();
    let mut state = crate::list::State::capture(list);
    let rows = rows();
    read(|a| {
        for index in &mut state.indices {
            if let Some(old) = a.sources.rows.get(*index as usize) {
                let occurrence = a.sources.rows[..*index as usize]
                    .iter()
                    .filter(|row| row.path == old.path)
                    .count();
                *index = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.path == old.path)
                    .nth(occurrence)
                    .map_or_else(
                        || (*index).min(rows.len() as isize - 1),
                        |(n, _)| n as isize,
                    );
            }
        }
    });
    let count = rows.len();
    write(|a| a.sources.rows = rows);
    state.apply(list, count);
    read(|a| a.sidebar.panel(super::panel::SOURCES_PANEL))
        .list
        .refresh_tooltip();
    update_actions();
}

pub(super) fn context_menu(hwnd: HWND, location: isize) -> anyhow::Result<()> {
    let mut point = point_from_lparam(location);
    if location == -1 {
        let Some(index) = crate::list::selected(hwnd) else {
            return Ok(());
        };
        send(hwnd, LVM_ENSUREVISIBLE, index, 0);
        let rect =
            crate::list::item_rect(hwnd, index).unwrap_or_else(|| crate::layout::rect(0, 0, 0, 0));
        point = POINT {
            x: rect.left,
            y: rect.bottom,
        };
        unsafe {
            ClientToScreen(hwnd, &mut point);
        }
    } else {
        let mut hit = LVHITTESTINFO {
            pt: point,
            ..unsafe { std::mem::zeroed() }
        };
        unsafe {
            ScreenToClient(hwnd, &mut hit.pt);
        }
        let index = send_mut(hwnd, LVM_HITTEST, 0, &mut hit);
        if index < 0 {
            return Ok(());
        }
        let state = LVIS_SELECTED | LVIS_FOCUSED;
        crate::list::set_item_state(hwnd, index, state, state);
        send(hwnd, LVM_SETSELECTIONMARK, 0, index);
    }
    let Some(path) = selected() else {
        return Ok(());
    };
    let pending = read(|a| a.session.pending(&path));
    unsafe {
        let menu = Popup::new()?;
        for (id, label) in [
            (RELOAD_SOURCE, IDS_RELOAD),
            (REMOVE_SOURCE, IDS_UNLOAD),
            (COPY_SOURCE_PATH, IDS_COPY_SOURCE_PATH),
        ] {
            menu.item(
                id,
                resources::wide_text(label),
                id != RELOAD_SOURCE || !pending,
            )?;
        }
        let command = menu.track(hwnd, point);
        if command != 0 && IsWindow(hwnd) != 0 {
            action_path(command, path);
        }
    }
    Ok(())
}

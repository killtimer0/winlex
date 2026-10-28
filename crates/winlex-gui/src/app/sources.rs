//! The source panel projects the committed catalog; opening files is a load transaction.
use super::{catalog, read};
use crate::{
    input,
    native::*,
    resources::{self, *},
};
use std::path::PathBuf;

pub(super) fn cell(row: usize, detail: bool) -> String {
    read(|a| {
        let Some(source) = a.session.inputs().get(row) else {
            return String::new();
        };
        let path = source.path();
        let state = resources::text(if a.session.pending(path) {
            IDS_LOADING
        } else {
            IDS_LOADED
        });
        if detail {
            format!("{}\r\n{} · {} B", path.display(), state, source.len())
        } else {
            format!(
                "{}\n{} · {} B",
                path.file_name().unwrap_or_default().to_string_lossy(),
                state,
                source.len()
            )
        }
    })
}

pub(super) fn add(files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    let mut paths = catalog::paths();
    for file in files {
        let path = match std::path::absolute(file) {
            Ok(path) => path,
            Err(error) => {
                catalog::set_status(error.to_string());
                return;
            }
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    catalog::reload(paths, vec![]);
}

pub(super) fn choose_add() {
    match input::choose_files(read(|a| a.frame), read(|a| a.system_version)) {
        Ok(paths) => add(paths),
        Err(error) => catalog::set_status(error),
    }
}

pub(super) fn action(command: u16) {
    let list = unsafe { GetDlgItem(read(|a| a.frame), SOURCE_LIST as i32) };
    let row = send(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    let path = read(|a| {
        a.session
            .inputs()
            .get(row as usize)
            .map(|s| s.path().to_path_buf())
    });
    let Some(path) = path else {
        return;
    };
    let mut paths = catalog::paths();
    if command == REMOVE_SOURCE {
        paths.retain(|p| *p != path);
        catalog::reload(paths, vec![]);
    } else {
        catalog::reload(paths, vec![path]);
    }
}

pub(super) fn refresh(committed: bool) {
    let list = unsafe { GetDlgItem(read(|a| a.frame), SOURCE_LIST as i32) };
    if committed {
        let item = LVITEMW {
            stateMask: LVIS_SELECTED | LVIS_FOCUSED,
            ..unsafe { std::mem::zeroed() }
        };
        send(
            list,
            LVM_SETITEMSTATE,
            usize::MAX,
            &item as *const _ as isize,
        );
    }
    send(
        list,
        LVM_SETITEMCOUNT,
        read(|a| a.session.inputs().len()),
        LVSICF_NOSCROLL as isize,
    );
    super::panel::fit(list);
    unsafe {
        InvalidateRect(list, std::ptr::null(), 0);
    }
}

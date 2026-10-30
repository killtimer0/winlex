//! The source panel projects the committed catalog; opening files is a load transaction.
use super::{catalog, read, write};
use crate::{
    input,
    list::Display,
    native::*,
    resources::{self, *},
};
use std::{os::windows::ffi::OsStrExt, path::PathBuf};

pub(super) struct Row {
    pub display: Display,
    pub text: Vec<u16>,
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    read(|a| {
        if let Some(row) = a.session.source_rows.get(index) {
            visit(&row.display);
        }
    });
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    read(|a| {
        a.session
            .source_rows
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
                let detail: Vec<u16> = format!(
                    "{} · {} B",
                    resources::text(if a.session.pending(path) {
                        IDS_LOADING
                    } else {
                        IDS_LOADED
                    }),
                    source.len()
                )
                .encode_utf16()
                .collect();
                let info = path
                    .as_os_str()
                    .encode_wide()
                    .chain("\r\n".encode_utf16())
                    .chain(detail.iter().copied())
                    .collect();
                Row {
                    display: Display {
                        title: path.file_name().unwrap_or_default().encode_wide().collect(),
                        detail,
                        ..Default::default()
                    },
                    text: info,
                }
            })
            .collect()
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
        Err(error) => catalog::set_status(format!("{error:#}")),
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
    let rows = rows();
    write(|a| a.session.source_rows = rows);
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
    crate::list::fit(list);
    unsafe {
        InvalidateRect(list, std::ptr::null(), 0);
    }
}

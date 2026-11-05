//! The source panel projects the committed catalog; opening files is a load transaction.
use super::catalog;
use crate::app::context::Context;
use crate::{
    icons::Icon,
    input,
    list::{Display, FoldKind, TextLine, TextSegment, tooltip_field},
    native::*,
    resources::{self, *},
};
use std::{os::windows::ffi::OsStrExt, path::PathBuf};

const METADATA_GUIDE: &str = "https://github.com/killtimer0/winlex/blob/main/docs/metadata.md";

pub(super) fn should_prompt(command_line: &[PathBuf], saved: Option<&[PathBuf]>) -> bool {
    command_line.is_empty() && saved.is_some_and(|paths| paths.is_empty())
}

pub(super) fn prompt_guide(ctx: &Context) {
    let answer = unsafe {
        MessageBoxW(
            ctx.frame(),
            resources::wide_text(IDS_METADATA_PROMPT).as_ptr(),
            resources::wide_text(IDS_APP_TITLE).as_ptr(),
            MB_YESNO | MB_ICONQUESTION,
        )
    };
    if answer == IDYES {
        open_guide(ctx);
    }
}

pub(super) fn open_guide(ctx: &Context) {
    if open_url(ctx.frame(), METADATA_GUIDE).is_err() {
        let result = resources::dialog(IDD_SEARCH_HELP).and_then(|template| {
            crate::dialog::modal::show(
                ctx.frame(),
                ctx.dpi(),
                template,
                &[
                    (
                        SEARCH_HELP_TEXT as i32,
                        crate::dialog::SIZE_X | crate::dialog::SIZE_Y,
                    ),
                    (IDOK, crate::dialog::MOVE_X | crate::dialog::MOVE_Y),
                ],
                &|hwnd, message, _, _| {
                    if message == WM_INITDIALOG {
                        set_text(hwnd, &resources::text(IDS_GET_METADATA));
                        set_text(
                            unsafe { GetDlgItem(hwnd, SEARCH_HELP_TEXT as i32) },
                            &format!(
                                "{}\r\n\r\n{}",
                                resources::text(IDS_METADATA_BROWSER_FAILED),
                                METADATA_GUIDE
                            ),
                        );
                    }
                    Ok(())
                },
            )
        });
        super::status::report(ctx, result);
    }
}

#[derive(Default)]
pub(super) struct Sources {
    rows: Vec<Row>,
}
struct Row {
    pub path: PathBuf,
    pub display: Display,
    pub text: Vec<u16>,
}
pub(super) fn row(ctx: &Context, index: usize, visit: &mut dyn FnMut(&Display)) {
    ctx.with_sources(|sources| {
        if let Some(row) = sources.rows.get(index) {
            visit(&row.display);
        }
    });
}
pub(super) fn row_text(ctx: &Context, index: usize) -> Vec<u16> {
    ctx.with_sources(|sources| {
        sources
            .rows
            .get(index)
            .map(|r| r.text.clone())
            .unwrap_or_default()
    })
}
fn rows(ctx: &Context) -> Vec<Row> {
    ctx.with_session(|session| {
        session
            .inputs()
            .iter()
            .map(|source| {
                let path = source.path();
                let len = source.len();
                let failure = session.source_error(path);
                let status = resources::text(if session.pending(path) {
                    IDS_RELOADING
                } else if failure.is_some() {
                    IDS_RELOAD_FAILED
                } else {
                    IDS_LOADED
                });
                let mut detail = TextLine::default();
                detail.push(TextSegment::plain(&format!("{status} · {} · ", size(len))));
                if let Some(parent) = path.parent() {
                    detail.push(TextSegment::new(
                        parent.as_os_str().encode_wide(),
                        FoldKind::Path,
                    ));
                }
                let exact = resources::text(IDS_SIZE_BYTES).replace("{size}", &len.to_string());
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

fn list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), SOURCE_LIST as i32) }
}
fn selected(ctx: &Context) -> Option<PathBuf> {
    let index = crate::list::selected(list(ctx))?;
    ctx.with_sources(|sources| sources.rows.get(index).map(|row| row.path.clone()))
}
pub(super) fn update_actions(ctx: &Context) {
    let path = selected(ctx);
    let reload = path.as_ref().is_some_and(|p| !catalog::pending(ctx, p));
    let frame = ctx.frame();
    unsafe {
        EnableWindow(GetDlgItem(frame, RELOAD_SOURCE as i32), reload as i32);
        EnableWindow(
            GetDlgItem(frame, REMOVE_SOURCE as i32),
            path.is_some() as i32,
        );
    }
}

pub(super) fn add(ctx: &Context, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    super::status::begin_status(
        ctx,
        super::status::StatusContext::Panel(super::panel::PanelId::Sources),
    );
    let mut paths = catalog::paths(ctx);
    for file in files {
        let path = match std::path::absolute(file) {
            Ok(path) => path,
            Err(error) => {
                super::status::show_notice(ctx, error.to_string());
                return;
            }
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    catalog::reload(ctx, paths, vec![]);
}

fn choose_add(ctx: &Context) {
    if let Some(paths) = super::status::report(
        ctx,
        input::choose(
            ctx.frame(),
            ctx.system_version(),
            IDS_LOAD_METADATA,
            IDS_WINMD_FILES,
            "*.winmd",
            true,
        ),
    ) {
        add(ctx, paths);
    }
}

pub(super) fn command(ctx: &Context, id: u16) -> bool {
    if matches!(
        id,
        LOAD_METADATA | RELOAD_SOURCE | REMOVE_SOURCE | COPY_SOURCE_PATH
    ) {
        super::status::begin_status(
            ctx,
            super::status::StatusContext::Panel(super::panel::PanelId::Sources),
        );
    }
    match id {
        LOAD_METADATA => choose_add(ctx),
        RELOAD_SOURCE | REMOVE_SOURCE | COPY_SOURCE_PATH => {
            if let Some(path) = selected(ctx) {
                action_path(ctx, id, path);
            }
        }
        _ => return false,
    }
    true
}

fn action_path(ctx: &Context, command: u16, path: PathBuf) {
    if command == COPY_SOURCE_PATH {
        super::status::report(
            ctx,
            copy_unicode(
                list(ctx),
                &path.as_os_str().encode_wide().collect::<Vec<_>>(),
            ),
        );
        return;
    }
    if !catalog::contains_path(ctx, &path) {
        return;
    }
    let mut paths = catalog::paths(ctx);
    if command == REMOVE_SOURCE {
        paths.retain(|p| *p != path);
        catalog::reload(ctx, paths, vec![]);
    } else if command == RELOAD_SOURCE && !catalog::pending(ctx, &path) {
        catalog::reload(ctx, paths, vec![path]);
    }
}

pub(super) fn refresh(ctx: &Context) {
    let list = list(ctx);
    let mut state = crate::list::State::capture(list);
    let rows = rows(ctx);
    ctx.with_sources(|sources| {
        for index in &mut state.indices {
            if let Some(old) = sources.rows.get(*index as usize) {
                let occurrence = sources.rows[..*index as usize]
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
    ctx.with_sources_mut(|sources| sources.rows = rows);
    state.apply(list, count);
    super::sidebar::panel(ctx, super::panel::SOURCES_PANEL)
        .list
        .refresh_tooltip();
    update_actions(ctx);
}

pub(super) fn context_menu(ctx: &Context, hwnd: HWND, location: isize) -> anyhow::Result<()> {
    let Some((_, point)) = crate::list::context_target(hwnd, location) else {
        return Ok(());
    };
    let Some(path) = selected(ctx) else {
        return Ok(());
    };
    let pending = catalog::pending(ctx, &path);
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
            action_path(ctx, command, path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_guide_requires_known_empty_configuration() {
        let file = PathBuf::from("missing.winmd");
        assert!(should_prompt(&[], Some(&[])));
        assert!(!should_prompt(&[], None));
        assert!(!should_prompt(&[], Some(std::slice::from_ref(&file))));
        assert!(!should_prompt(&[file], Some(&[])));
    }
}

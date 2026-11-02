use super::{dpi, read};
use crate::{
    dialog::{MOVE_X, MOVE_Y, SIZE_X, SIZE_Y, modal},
    icons::WindowIcons,
    native::*,
    resources::{self, *},
};
use std::cell::RefCell;

pub(super) fn search() -> anyhow::Result<()> {
    modal::show(
        read(|a| a.frame),
        dpi(),
        resources::dialog(IDD_SEARCH_HELP)?,
        &[
            (SEARCH_HELP_TEXT as i32, SIZE_X | SIZE_Y),
            (IDOK, MOVE_X | MOVE_Y),
        ],
        &|hwnd, message, _, _| {
            if message == WM_INITDIALOG {
                set_text(
                    unsafe { GetDlgItem(hwnd, SEARCH_HELP_TEXT as i32) },
                    &resources::text(IDS_SEARCH_HELP).replace('\n', "\r\n"),
                );
            }
            Ok(())
        },
    )
}

pub(super) fn about() -> anyhow::Result<()> {
    let icons = RefCell::new(None);
    modal::show(
        read(|a| a.frame),
        dpi(),
        resources::dialog(IDD_ABOUT)?,
        &[],
        &|hwnd, message, w, l| {
            if message == WM_INITDIALOG || message == WM_DPICHANGED {
                let value = if message == WM_DPICHANGED {
                    (w & 0xffff) as u32
                } else {
                    dpi().window(hwnd)
                };
                let new_icons = WindowIcons::load(value)?;
                new_icons.apply(hwnd);
                send(
                    unsafe { GetDlgItem(hwnd, ABOUT_ICON as i32) },
                    STM_SETICON,
                    send(hwnd, WM_GETICON, ICON_BIG as usize, 0) as usize,
                    0,
                );
                *icons.borrow_mut() = Some(new_icons);
            }
            if message == WM_NOTIFY {
                let header = unsafe { &*(l as *const NMHDR) };
                if matches!(header.idFrom as u16, ABOUT_HOMEPAGE | ABOUT_LICENSE)
                    && matches!(header.code, NM_CLICK | NM_RETURN)
                {
                    let link = unsafe { &*(l as *const NMLINK) };
                    let end = link
                        .item
                        .szUrl
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(link.item.szUrl.len());
                    if end != 0 {
                        open_url(hwnd, &String::from_utf16_lossy(&link.item.szUrl[..end]))?;
                    }
                }
            }
            Ok(())
        },
    )
}

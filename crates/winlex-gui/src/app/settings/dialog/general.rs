use super::{Page, Preferences};
use crate::{
    app::settings::THEMES,
    native::*,
    resources::{self, *},
};

pub(super) const PAGE: Page = Page {
    template: IDD_SETTINGS_GENERAL,
    title: IDS_GENERAL,
    initialize,
    read,
};

fn initialize(hwnd: HWND, preferences: Preferences) {
    let language = unsafe { GetDlgItem(hwnd, SETTINGS_LANGUAGE as i32) };
    for (index, option) in LANGUAGES.iter().enumerate() {
        send(
            language,
            CB_ADDSTRING,
            0,
            resources::wide_text(option.label).as_ptr() as isize,
        );
        if option.value == preferences.language {
            send(language, CB_SETCURSEL, index, 0);
        }
    }
    let theme = unsafe { GetDlgItem(hwnd, SETTINGS_THEME as i32) };
    let supported = crate::theme::supported();
    for (index, &(value, _, label)) in THEMES.iter().enumerate() {
        if !supported && index != 0 {
            continue;
        }
        send(
            theme,
            CB_ADDSTRING,
            0,
            resources::wide_text(label).as_ptr() as isize,
        );
        if value == preferences.theme || !supported {
            send(theme, CB_SETCURSEL, index, 0);
        }
    }
    unsafe { EnableWindow(theme, i32::from(supported)) };
}

fn read(hwnd: HWND, preferences: &mut Preferences) -> anyhow::Result<()> {
    let index = send(
        unsafe { GetDlgItem(hwnd, SETTINGS_LANGUAGE as i32) },
        CB_GETCURSEL,
        0,
        0,
    );
    preferences.language = LANGUAGES
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("Invalid language selection"))?
        .value;
    // Unsupported systems display System without discarding a preference saved
    // on a supported OS when the user commits unrelated settings.
    if crate::theme::supported() {
        let index = send(
            unsafe { GetDlgItem(hwnd, SETTINGS_THEME as i32) },
            CB_GETCURSEL,
            0,
            0,
        );
        preferences.theme = THEMES
            .get(index as usize)
            .ok_or_else(|| anyhow::anyhow!("Invalid theme selection"))?
            .0;
    }
    Ok(())
}

use super::{Page, Preferences};
use crate::app::settings::SEARCH_ENGINES;
use crate::{native::*, resources::*};
use winlex_core::EntityCategory;

pub(super) const PAGE: Page = Page {
    template: IDD_SETTINGS_SEARCH,
    title: IDS_PANEL_SEARCH,
    initialize,
    read,
};

const FILTERS: &[(u16, EntityCategory)] = &[
    (FILTER_TYPE, EntityCategory::Type),
    (FILTER_FUNCTION, EntityCategory::Function),
    (FILTER_METHOD, EntityCategory::Method),
    (FILTER_CONSTANT, EntityCategory::Constant),
    (FILTER_FIELD, EntityCategory::Field),
    (FILTER_PROPERTY, EntityCategory::Property),
    (FILTER_EVENT, EntityCategory::Event),
    (FILTER_REFERENCE, EntityCategory::TypeReference),
];

fn initialize(hwnd: HWND, preferences: Preferences) {
    let engine = unsafe { GetDlgItem(hwnd, SETTINGS_SEARCH_ENGINE as i32) };
    for (index, &(value, _, label, _)) in SEARCH_ENGINES.iter().enumerate() {
        send(
            engine,
            CB_ADDSTRING,
            0,
            crate::resources::wide_text(label).as_ptr() as isize,
        );
        if value == preferences.search_engine {
            send(engine, CB_SETCURSEL, index, 0);
        }
    }
    for &(id, category) in FILTERS {
        send(
            unsafe { GetDlgItem(hwnd, id as i32) },
            BM_SETCHECK,
            usize::from(preferences.categories & category as u32 != 0),
            0,
        );
    }
}

fn read(hwnd: HWND, preferences: &mut Preferences) -> anyhow::Result<()> {
    let index = send(
        unsafe { GetDlgItem(hwnd, SETTINGS_SEARCH_ENGINE as i32) },
        CB_GETCURSEL,
        0,
        0,
    );
    preferences.search_engine = SEARCH_ENGINES
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("Invalid search engine selection"))?
        .0;
    preferences.categories = 0;
    for &(id, category) in FILTERS {
        if send(unsafe { GetDlgItem(hwnd, id as i32) }, BM_GETCHECK, 0, 0) == BST_CHECKED as isize {
            preferences.categories |= category as u32;
        }
    }
    Ok(())
}

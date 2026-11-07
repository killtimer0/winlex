use super::{Page, Preferences};
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
    (FILTER_METHOD, EntityCategory::Method),
    (FILTER_CONSTANT, EntityCategory::Constant),
    (FILTER_FIELD, EntityCategory::Field),
    (FILTER_PROPERTY, EntityCategory::Property),
    (FILTER_EVENT, EntityCategory::Event),
    (FILTER_REFERENCE, EntityCategory::TypeReference),
];

fn initialize(hwnd: HWND, preferences: Preferences) {
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
    preferences.categories = 0;
    for &(id, category) in FILTERS {
        if send(unsafe { GetDlgItem(hwnd, id as i32) }, BM_GETCHECK, 0, 0) == BST_CHECKED as isize {
            preferences.categories |= category as u32;
        }
    }
    Ok(())
}

//! Keyboard routing: one table from the focused region and key to an action.
//!
//! Semantic keys are resolved before the dialog manager sees them. Controls
//! keep their default behavior for every key the table does not claim.
use super::{catalog, document, read, sources, workspace};
use crate::native::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Region {
    SearchEntry { composing: bool },
    Results,
    Sources,
    Tabs,
    Document,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    Search,
    Preview,
    Pin,
    OpenSource,
    FocusDocument,
}

/// Pure routing table; every key that is not listed keeps its native meaning.
pub(super) fn action(region: Region, key: usize) -> Option<Action> {
    match (region, key) {
        (Region::SearchEntry { composing: false }, k) if k == VK_RETURN as usize => {
            Some(Action::Search)
        }
        (Region::Results, k) if k == VK_RETURN as usize => Some(Action::Pin),
        (Region::Results, k) if k == VK_SPACE as usize => Some(Action::Preview),
        (Region::Sources, k) if k == VK_RETURN as usize => Some(Action::OpenSource),
        (Region::Tabs, k) if k == VK_RETURN as usize => Some(Action::FocusDocument),
        _ => None,
    }
}

/// Claim a key press for the region that currently owns the keyboard.
pub(super) fn translate(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let Some(action) = action(region(msg.hwnd), msg.wParam) else {
        return false;
    };
    dispatch(action);
    true
}

fn region(hwnd: HWND) -> Region {
    let (panel, tabs) = read(|a| (a.sidebar.owner(hwnd), a.workspace.tabs));
    if let Some(region) = panel.as_ref().and_then(|p| p.region(hwnd)) {
        return region;
    }
    if hwnd == tabs {
        return Region::Tabs;
    }
    let doc = workspace::active();
    if !doc.is_null() && unsafe { hwnd == doc || IsChild(doc, hwnd) != 0 } {
        return Region::Document;
    }
    Region::Other
}

fn dispatch(action: Action) {
    match action {
        Action::Search => catalog::search(),
        Action::Preview => catalog::open_selected(catalog::Open::Preview),
        Action::Pin => catalog::open_selected(catalog::Open::Pin),
        Action::OpenSource => sources::open_selected(),
        Action::FocusDocument => document::focus_document(workspace::active()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTER: usize = VK_RETURN as usize;
    const SPACE: usize = VK_SPACE as usize;

    fn route(region: Region, key: usize) -> Option<Action> {
        action(region, key)
    }

    #[test]
    fn enter_follows_the_focused_region() {
        assert_eq!(
            route(Region::SearchEntry { composing: false }, ENTER),
            Some(Action::Search)
        );
        assert_eq!(route(Region::Results, ENTER), Some(Action::Pin));
        assert_eq!(route(Region::Sources, ENTER), Some(Action::OpenSource));
        assert_eq!(route(Region::Tabs, ENTER), Some(Action::FocusDocument));
    }

    #[test]
    fn composing_text_keeps_enter() {
        assert_eq!(route(Region::SearchEntry { composing: true }, ENTER), None);
    }

    #[test]
    fn space_previews_only_in_the_result_list() {
        assert_eq!(route(Region::Results, SPACE), Some(Action::Preview));
        assert_eq!(route(Region::SearchEntry { composing: false }, SPACE), None);
        assert_eq!(route(Region::Sources, SPACE), None);
        assert_eq!(route(Region::Document, SPACE), None);
        assert_eq!(route(Region::Other, SPACE), None);
    }

    #[test]
    fn unclaimed_keys_keep_their_native_meaning() {
        assert_eq!(route(Region::Document, ENTER), None);
        assert_eq!(route(Region::Results, VK_F1 as usize), None);
    }
}

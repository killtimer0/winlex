//! Keyboard routing: one table from the focused region and key to an action.
//!
//! Semantic keys are resolved before the dialog manager sees them. Controls
//! keep their default behavior for every key the table does not claim.
use super::{catalog, panel, read, workspace};
use crate::native::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Region {
    SearchEntry { composing: bool },
    Results,
    Tabs,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Action {
    Search,
    Preview,
    Pin,
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
    let (search, results, tabs) = read(|a| {
        (
            a.sidebar.search.clone(),
            a.sidebar.panel(panel::SEARCH_PANEL).list.hwnd,
            a.workspace.tabs,
        )
    });
    if let Some(search) = search
        && hwnd == search.input
    {
        return Region::SearchEntry {
            composing: search.composing.get(),
        };
    }
    if hwnd == results {
        return Region::Results;
    }
    if hwnd == tabs {
        return Region::Tabs;
    }
    Region::Other
}

fn dispatch(action: Action) {
    match action {
        Action::Search => catalog::search(),
        Action::Preview => catalog::open_selected(catalog::Open::Preview),
        Action::Pin => catalog::open_selected(catalog::Open::Pin),
        Action::FocusDocument => {
            if let Some(document) = workspace::view(workspace::active()) {
                document.focus();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTER: usize = VK_RETURN as usize;
    const SPACE: usize = VK_SPACE as usize;

    #[test]
    fn semantic_keys_respect_region_and_composition() {
        for (region, enter, space) in [
            (
                Region::SearchEntry { composing: false },
                Some(Action::Search),
                None,
            ),
            (Region::SearchEntry { composing: true }, None, None),
            (Region::Results, Some(Action::Pin), Some(Action::Preview)),
            (Region::Tabs, Some(Action::FocusDocument), None),
            (Region::Other, None, None),
        ] {
            assert_eq!(action(region, ENTER), enter, "{region:?}: Enter");
            assert_eq!(action(region, SPACE), space, "{region:?}: Space");
        }
        assert_eq!(action(Region::Results, VK_F1 as usize), None);
    }
}

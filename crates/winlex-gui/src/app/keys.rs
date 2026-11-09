//! Keyboard routing for the focused control.
//!
//! Semantic keys are resolved before the dialog manager sees them. Controls
//! keep their default behavior for every key the router does not claim.
use super::{catalog, panel, sidebar, workspace};
use crate::app::context::Context;
use crate::native::*;

/// Claim a key press for the region that currently owns the keyboard.
pub(super) fn translate(ctx: &Context, msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    if msg.wParam == VK_RETURN as usize
        && let Some(panel) = sidebar::owner(ctx, msg.hwnd)
        && msg.hwnd == panel.list.hwnd
        && panel.list.activate_selected()
    {
        return true;
    }
    let search = sidebar::search(ctx);
    match msg.wParam {
        key if key == VK_RETURN as usize && msg.hwnd == search.input && !search.composing.get() => {
            catalog::search(ctx);
        }
        key if key == VK_SPACE as usize
            && msg.hwnd == sidebar::panel(ctx, panel::SEARCH_PANEL).list.hwnd =>
        {
            catalog::open_selected(ctx, catalog::Open::Preview);
        }
        key if key == VK_RETURN as usize && msg.hwnd == workspace::tabs(ctx) => {
            if let Some(document) = workspace::content(ctx, workspace::active(ctx)) {
                document.focus();
            }
        }
        _ => return false,
    }
    true
}

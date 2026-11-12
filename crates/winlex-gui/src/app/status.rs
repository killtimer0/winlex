//! Panel/document status ownership and command feedback.
use super::{
    panel::{self, PanelId},
    sidebar, workspace,
};
use crate::app::context::Context;
use crate::native::{HWND, IsChild, set_status_text};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusContext {
    Panel(PanelId),
    Document(HWND),
}
impl Default for StatusContext {
    fn default() -> Self {
        Self::Panel(PanelId::Search)
    }
}
#[derive(Clone, Default)]
pub(super) struct Status {
    context: StatusContext,
    notice: Option<String>,
}
impl Status {
    fn begin(&mut self, context: StatusContext) {
        self.context = context;
        self.notice = None;
    }
    fn message(
        &self,
        content: impl FnOnce(HWND) -> Option<String>,
        panel: impl FnOnce(Option<PanelId>) -> String,
        visible: Option<PanelId>,
    ) -> String {
        self.notice.clone().unwrap_or_else(|| {
            match self.context {
                StatusContext::Document(hwnd) => content(hwnd),
                StatusContext::Panel(_) => None,
            }
            .unwrap_or_else(|| panel(visible))
        })
    }
}
pub(super) fn begin_status(ctx: &Context, context: StatusContext) {
    ctx.with_status_mut(|status| status.begin(context));
    refresh_status(ctx);
}
pub(super) fn status_context(ctx: &Context, context: StatusContext) {
    if ctx.with_status(|status| status.context != context) {
        begin_status(ctx, context);
    }
}
pub(super) fn begin_operation(ctx: &Context) {
    ctx.with_status_mut(|status| status.notice = None);
    refresh_status(ctx);
}
pub(super) fn status_input(ctx: &Context, hwnd: HWND) {
    let doc = workspace::active(ctx);
    if !doc.is_null() && (hwnd == doc || unsafe { IsChild(doc, hwnd) } != 0) {
        begin_status(ctx, StatusContext::Document(doc));
    } else if let Some(panel) = sidebar::owner(ctx, hwnd) {
        begin_status(ctx, StatusContext::Panel(panel.id));
    }
}
pub(super) fn show_notice(ctx: &Context, message: String) {
    ctx.with_status_mut(|status| status.notice = Some(message));
    refresh_status(ctx);
}
pub(super) fn report<T>(ctx: &Context, result: anyhow::Result<T>) -> Option<T> {
    result
        .map_err(|error| show_notice(ctx, format!("{error:#}")))
        .ok()
}
pub(super) fn refresh_status(ctx: &Context) {
    let state = ctx.with_status(|status| status.clone());
    let hwnd = ctx.status_bar();
    let empty_status = ctx.with_session(|session| session.empty_status());
    let message = state.message(
        |hwnd| {
            workspace::content(ctx, hwnd)
                .map(|content| content.status())
                .filter(|message| !message.is_empty() || empty_status.is_none())
        },
        |id| {
            empty_status
                .clone()
                .unwrap_or_else(|| id.map(|id| panel::status(ctx, id)).unwrap_or_default())
        },
        sidebar::current(ctx).map(|panel| panel.id),
    );
    set_status_text(hwnd, &message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn host_notice_survives_background_updates_until_next_operation() {
        let mut status = Status::default();
        status.begin(StatusContext::Document(ptr::null_mut()));
        status.notice = Some("operation failed".into());
        for background in ["querying", "done"] {
            assert_eq!(
                status.message(
                    |_| Some(background.into()),
                    |_| background.into(),
                    Some(PanelId::Search)
                ),
                "operation failed"
            );
        }
        status.begin(StatusContext::Document(ptr::null_mut()));
        assert_eq!(
            status.message(
                |_| Some("tool".into()),
                |_| "search".into(),
                Some(PanelId::Search)
            ),
            "tool"
        );
        status.notice = Some("another failure".into());
        status.begin(StatusContext::Panel(PanelId::Search));
        assert_eq!(
            status.message(
                |_| panic!("inactive tool queried"),
                |_| "search error".into(),
                Some(PanelId::Search)
            ),
            "search error"
        );
    }
}

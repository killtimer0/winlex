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
        panel: impl Fn(PanelId) -> String,
        visible: Option<PanelId>,
    ) -> String {
        self.notice.clone().unwrap_or_else(|| {
            match self.context {
                StatusContext::Document(hwnd) => content(hwnd),
                StatusContext::Panel(id) if visible == Some(id) => Some(panel(id)),
                StatusContext::Panel(_) => None,
            }
            .unwrap_or_else(|| visible.map(panel).unwrap_or_default())
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
    let message = state.message(
        |hwnd| workspace::content(ctx, hwnd).map(|content| content.status()),
        |id| panel::status(ctx, id),
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

    #[test]
    fn missing_document_uses_visible_panel_but_empty_document_stays_empty() {
        let mut status = Status::default();
        status.begin(StatusContext::Document(ptr::null_mut()));
        assert_eq!(
            status.message(|_| None, |_| "sources".into(), Some(PanelId::Sources)),
            "sources"
        );
        assert_eq!(
            status.message(|_| None, |_| panic!("no visible panel"), None),
            ""
        );
        assert_eq!(
            status.message(
                |_| Some(String::new()),
                |_| panic!("document owns blank status"),
                Some(PanelId::Search)
            ),
            ""
        );
    }

    #[test]
    fn panel_contexts_do_not_borrow_other_panels_and_collapsed_sidebar_is_blank() {
        let mut status = Status::default();
        for id in PanelId::ALL {
            status.begin(StatusContext::Panel(id));
            assert_eq!(
                status.message(
                    |_| panic!("panel does not query document"),
                    |owner| {
                        assert_eq!(owner, id);
                        format!("{owner:?}")
                    },
                    Some(id)
                ),
                format!("{id:?}")
            );
            assert_eq!(
                status.message(|_| None, |_| panic!("collapsed panel"), None),
                ""
            );
        }
    }
}

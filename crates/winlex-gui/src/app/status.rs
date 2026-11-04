//! Catalog/document status ownership and command feedback.
use super::{catalog, sidebar, workspace};
use crate::app::context::Context;
use crate::native::{HWND, IsChild, set_status_text};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum StatusContext {
    #[default]
    Catalog,
    Document(HWND),
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
        catalog: impl FnOnce() -> String,
    ) -> String {
        self.notice.clone().unwrap_or_else(|| {
            match self.context {
                StatusContext::Document(hwnd) => content(hwnd),
                StatusContext::Catalog => None,
            }
            .unwrap_or_else(catalog)
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
    } else if sidebar::owner(ctx, hwnd).is_some() {
        begin_status(ctx, StatusContext::Catalog);
    }
}
pub(super) fn set_status(ctx: &Context, message: String) {
    ctx.with_status_mut(|status| status.notice = Some(message));
    refresh_status(ctx);
}
pub(super) fn report<T>(ctx: &Context, result: anyhow::Result<T>) -> Option<T> {
    result
        .map_err(|error| set_status(ctx, format!("{error:#}")))
        .ok()
}
pub(super) fn refresh_status(ctx: &Context) {
    let state = ctx.with_status(|status| status.clone());
    let hwnd = ctx.status_bar();
    let message = state.message(
        |hwnd| workspace::content(ctx, hwnd).and_then(|content| content.status()),
        || catalog::status(ctx),
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
                status.message(|_| Some(background.into()), || background.into()),
                "operation failed"
            );
        }
        status.begin(StatusContext::Document(ptr::null_mut()));
        assert_eq!(
            status.message(|_| Some("tool".into()), || "catalog".into()),
            "tool"
        );
        status.notice = Some("another failure".into());
        status.begin(StatusContext::Catalog);
        assert_eq!(
            status.message(
                |_| panic!("inactive tool queried"),
                || "search error".into()
            ),
            "search error"
        );
    }

    #[test]
    fn missing_or_unspecialized_document_falls_back_to_catalog() {
        let mut status = Status::default();
        status.begin(StatusContext::Document(ptr::null_mut()));
        assert_eq!(status.message(|_| None, || "catalog".into()), "catalog");
        assert_eq!(
            status.message(|_| Some(String::new()), || "catalog".into()),
            ""
        );
    }
}

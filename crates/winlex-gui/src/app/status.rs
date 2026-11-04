//! Catalog/document status ownership and command feedback.
use super::{read, workspace, write};
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
pub(super) fn begin_status(context: StatusContext) {
    write(|a| a.status_state.begin(context));
    refresh_status();
}
pub(super) fn status_context(context: StatusContext) {
    if read(|a| a.status_state.context != context) {
        begin_status(context);
    }
}
pub(super) fn begin_operation() {
    write(|a| a.status_state.notice = None);
    refresh_status();
}
pub(super) fn status_input(hwnd: HWND) {
    let doc = workspace::active();
    if !doc.is_null() && (hwnd == doc || unsafe { IsChild(doc, hwnd) } != 0) {
        begin_status(StatusContext::Document(doc));
    } else if read(|a| a.sidebar.owner(hwnd).is_some()) {
        begin_status(StatusContext::Catalog);
    }
}
pub(super) fn set_status(message: String) {
    write(|a| a.status_state.notice = Some(message));
    refresh_status();
}
pub(super) fn report<T>(result: anyhow::Result<T>) -> Option<T> {
    result
        .map_err(|error| set_status(format!("{error:#}")))
        .ok()
}
pub(super) fn refresh_status() {
    let (state, hwnd) = read(|a| (a.status_state.clone(), a.status));
    let message = state.message(
        |hwnd| workspace::content(hwnd).and_then(|content| content.status()),
        || read(|a| a.session.status()),
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

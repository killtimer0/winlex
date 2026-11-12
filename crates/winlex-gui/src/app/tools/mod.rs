//! Tool identity, list presentation and instance hosting.
use super::{history, workspace};
use crate::app::context::Context;
use crate::{
    icons::Icon,
    layout::{self, Placement},
    list::{Display, TextSegment},
    native::*,
    resources::{self, *},
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod com;
mod demangle;
mod error_code;

/// Shared tool subclass: Tab stays inside single-line inputs and IME composition
/// is flagged so Enter does not submit a half-composed value.
const TEXT_SUBCLASS: usize = 1810;

unsafe extern "system" fn text_proc(
    hwnd: HWND,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        match msg {
            WM_GETDLGCODE if w == VK_TAB as usize => {
                return (DLGC_WANTARROWS | DLGC_WANTCHARS) as isize;
            }
            WM_IME_STARTCOMPOSITION => {
                SetPropW(hwnd, w!("Winlex.Tool.Composing"), 1usize as HANDLE);
            }
            WM_IME_ENDCOMPOSITION => {
                RemovePropW(hwnd, w!("Winlex.Tool.Composing"));
            }
            WM_NCDESTROY => {
                RemovePropW(hwnd, w!("Winlex.Tool.Composing"));
                remove_subclass(hwnd, Some(text_proc), TEXT_SUBCLASS);
            }
            _ => {}
        }
        default_subclass(hwnd, msg, w, l)
    }
}

fn native_text(hwnd: HWND) -> Vec<u16> {
    let mut text = text_wide(hwnd);
    text.pop();
    text
}

/// State every tool view shares with the document host. Views dereference to it,
/// so tool code keeps using `self.hwnd`, `self.body` and the lifecycle helpers.
pub(super) struct Shell {
    // The document lives inside the context, so the pointer cannot outlive it.
    ctx: *const Context,
    pub id: u64,
    pub hwnd: HWND,
    pub placement: Cell<Option<Placement>>,
    pub(super) recent: Cell<u64>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    icon: Icon,
    pub labels: Vec<HWND>,
    result: HWND,
    copy: HWND,
    status: RefCell<String>,
    body: RefCell<crate::rich_text::Content>,
    plain: RefCell<String>,
    /// Set while the view updates its own controls, so their notifications are
    /// not mistaken for user edits.
    pub(super) setting: Cell<bool>,
}

impl Shell {
    fn new(ctx: &Context, status: String, icon: Icon) -> Self {
        Self {
            ctx: ctx as *const Context,
            id: super::workspace::next_serial(ctx),
            hwnd: std::ptr::null_mut(),
            placement: Cell::new(None),
            recent: Cell::new(0),
            icons: RefCell::new(None),
            icon,
            labels: Vec::new(),
            result: std::ptr::null_mut(),
            copy: std::ptr::null_mut(),
            status: RefCell::new(status),
            body: RefCell::new(crate::rich_text::Content::new(String::new())),
            plain: RefCell::new(String::new()),
            setting: Cell::new(true),
        }
    }
    fn ctx(&self) -> &Context {
        unsafe { &*self.ctx }
    }
    /// Dropdown lists need their collapsed height set from the font metrics; a
    /// later resize only sets the dropdown, not the closed control.
    fn size_combos(&self, controls: &[HWND]) {
        let ctx = self.ctx();
        let m = ctx.metrics(self.hwnd);
        for &combo in controls {
            ctx.checked(collapse_combo(combo, m.row(), 9));
        }
    }
    /// Font the labels, controls, copy button and result editor, then lay out.
    fn prepare_common(&self, controls: &[HWND], combos: &[HWND]) {
        self.update_icons();
        let m = self.ctx().metrics(self.hwnd);
        for h in self
            .labels
            .iter()
            .copied()
            .chain(controls.iter().copied())
            .chain(Some(self.copy))
        {
            super::bind(h, m.font);
        }
        self.size_combos(combos);
        let ctx = self.ctx();
        let (system_font, _) = ctx.font_for(ctx.dpi().system());
        ctx.checked(crate::rich_text::prepare(
            self.result,
            system_font,
            ctx.dpi(),
        ));
    }
    fn activated(&self) {
        self.recent.set(super::workspace::next_serial(self.ctx()));
    }
    /// Queue this tool's job, replacing its pending one and recording the new ticket.
    /// A submission failure belongs in the shared status line.
    fn submit(&self, pending: &Cell<Option<u64>>, job: crate::tasks::tools::Job) -> bool {
        let ctx = self.ctx();
        let ticket = crate::app::catalog::next_ticket(ctx);
        let id = ticket.id;
        let previous = pending.take();
        let result = ctx.replace_job(
            previous,
            ticket,
            crate::tasks::Job::Tool {
                document: self.id,
                job,
            },
        );
        match result {
            Ok(()) => {
                pending.set(Some(id));
                true
            }
            Err(error) => {
                self.set_status(&format!("{error:#}"));
                false
            }
        }
    }
    fn set_status(&self, message: &str) {
        self.status.replace(message.to_owned());
        super::document::status_changed(self.ctx(), self.hwnd);
    }
    fn status(&self) -> String {
        self.status.borrow().clone()
    }
    fn update_icons(&self) {
        let ctx = self.ctx();
        super::status::report(
            ctx,
            super::document::update_icons(ctx, self.hwnd, self.icon, &self.icons),
        );
    }
    fn recolor(&self) {
        self.update_icons();
        let _ = crate::rich_text::recolor(self.result, &self.body.borrow());
    }
    fn show(&self, content: crate::rich_text::Content, copy: String, status: &str) {
        if let Err(error) = crate::rich_text::set_content(self.result, &content) {
            self.set_status(&format!("{error:#}"));
            return;
        }
        self.body.replace(content);
        self.plain.replace(copy);
        self.set_status(status);
    }
    /// Shared tool layout: inset area and the row gaps above the result.
    fn layout_start(&self) -> Option<(layout::Metrics, RECT, i32, i32)> {
        if self.result.is_null() {
            return None;
        }
        let metrics = self.ctx().metrics(self.hwnd);
        let area = layout::inset(client(self.hwnd), metrics.px(8));
        let gap = metrics.px(4);
        let label_gap = metrics.px(8);
        Some((metrics, area, gap, label_gap))
    }
    /// Every tool ends with the result caption and the editor filling the rest.
    fn finish_layout(
        &self,
        metrics: &layout::Metrics,
        area: &mut RECT,
        label: HWND,
        positions: &mut Vec<(HWND, RECT)>,
    ) {
        layout::take_top(area, metrics.px(4));
        positions.push((
            label,
            layout::take_top(area, metrics.height + metrics.px(4)),
        ));
        positions.push((self.result, *area));
        if let Err(error) = place_all(positions) {
            self.set_status(&format!("{error:#}"));
        }
    }
    /// One shared label column, capped so controls always keep most of the width.
    fn label_column(&self, metrics: &layout::Metrics, labels: &[u16], available: i32) -> i32 {
        labels
            .iter()
            .map(|&id| metrics.text_width(resources::wide_text(id)))
            .max()
            .unwrap_or(0)
            .min(available / 3)
    }
    /// Ctrl+A selects the whole target edit, including an editable combo's child edit.
    fn ctrl_a(&self, msg: &MSG, targets: &[HWND]) -> bool {
        if msg.message == WM_KEYDOWN
            && msg.wParam == VK_A as usize
            && unsafe { GetKeyState(VK_CONTROL as i32) } < 0
            && targets
                .iter()
                .any(|&target| target == msg.hwnd || unsafe { IsChild(target, msg.hwnd) } != 0)
        {
            send(msg.hwnd, EM_SETSEL, 0, -1);
            return true;
        }
        false
    }
    /// Route the shared input shortcuts; `submit` runs when Enter reaches the input.
    fn key(&self, msg: &MSG, targets: &[HWND], input: HWND, submit: impl FnOnce()) -> bool {
        if self.ctrl_a(msg, targets) {
            return true;
        }
        if self.submits(msg, input) {
            submit();
            return true;
        }
        false
    }
    /// Enter submits a tool input only when IME composition is not active.
    fn submits(&self, msg: &MSG, input: HWND) -> bool {
        msg.message == WM_KEYDOWN
            && msg.hwnd == input
            && msg.wParam == VK_RETURN as usize
            && unsafe { GetPropW(input, w!("Winlex.Tool.Composing")) }.is_null()
    }
    /// Append the static caption for a labelled row or the result section.
    fn create_label(&mut self, hwnd: HWND, id: u16) -> anyhow::Result<()> {
        let caption = unsafe { child(hwnd, w!("STATIC"), resources::wide_text(id), 0, 0) }?;
        self.labels.push(caption);
        Ok(())
    }
    /// Create a labelled tab-stop edit or combo row.
    fn create_field(
        &mut self,
        hwnd: HWND,
        label: u16,
        id: u16,
        class: *const u16,
        style: u32,
        edge: u32,
    ) -> anyhow::Result<HWND> {
        self.create_label(hwnd, label)?;
        unsafe { child_ex(hwnd, class, &[0], WS_TABSTOP | style, edge, id) }
    }
    /// Create a tab-stop push button.
    fn create_button(&mut self, hwnd: HWND, label: u16, id: u16, style: u32) -> anyhow::Result<HWND> {
        unsafe { child(hwnd, w!("BUTTON"), resources::wide_text(label), WS_TABSTOP | style, id) }
    }
    /// Create the result copy button; the id identifies the tool's text.
    fn create_copy_button(&mut self, hwnd: HWND, id: u16) -> anyhow::Result<()> {
        self.copy = self.create_button(hwnd, IDS_COPY_ALL, id, 0)?;
        Ok(())
    }
    /// Fill a combo from fixed resource labels and select the first row.
    fn populate_combo(&self, combo: HWND, labels: impl IntoIterator<Item = u16>) {
        for label in labels {
            send(
                combo,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(combo, CB_SETCURSEL, 0, 0);
    }
    /// Create the result section's caption and rich-text editor.
    fn create_result(&mut self, hwnd: HWND, id: u16) -> anyhow::Result<()> {
        self.create_label(hwnd, IDS_RESULT_LABEL)?;
        self.result = crate::rich_text::create(hwnd, id)?;
        Ok(())
    }
    /// Subclass a single-line tool input for Tab and IME composition handling.
    fn subclass_text(&self, control: HWND) -> anyhow::Result<()> {
        anyhow::ensure!(
            unsafe { set_subclass(control, Some(text_proc), TEXT_SUBCLASS, 0) } != 0,
            "Subclass tool text control"
        );
        Ok(())
    }
    /// Copy the tool's plain result text; every tool owns one copy control.
    fn copy_plain(&self) {
        let copy: Vec<u16> = self.plain.borrow().encode_utf16().collect();
        if let Err(error) = copy_unicode(self.hwnd, &copy) {
            self.set_status(&format!("{error:#}"));
        }
    }
    /// The result editor's context menu belongs to the shared rich-text control.
    fn result_context_menu(&self, w: usize, l: isize) -> Option<isize> {
        if w as HWND != self.result {
            return None;
        }
        let _ = crate::rich_text::context_menu(self.result, l, &[]);
        Some(0)
    }
    /// Decode the tool message protocol; `command` handles the command id and code.
    fn message(
        &self,
        msg: u32,
        w: usize,
        l: isize,
        command: impl FnOnce(u16, u32) -> bool,
    ) -> Option<isize> {
        match msg {
            WM_COMMAND if command(w as u16, (w >> 16) as u32) => Some(0),
            WM_CONTEXTMENU => self.result_context_menu(w, l),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Kind {
    ErrorCode,
    Demangle,
    Com,
}

pub(super) struct Descriptor {
    pub kind: Kind,
    pub title: u16,
    pub description: u16,
    pub icon: Icon,
}

pub(super) const TOOLS: &[Descriptor] = &[
    Descriptor {
        kind: Kind::ErrorCode,
        title: IDS_ERROR_CODE_TOOL,
        description: IDS_ERROR_CODE_DESCRIPTION,
        icon: Icon::Constant,
    },
    Descriptor {
        kind: Kind::Demangle,
        title: IDS_DEMANGLE_TOOL,
        description: IDS_DEMANGLE_DESCRIPTION,
        icon: Icon::Method,
    },
    Descriptor {
        kind: Kind::Com,
        title: IDS_COM_TOOL,
        description: IDS_COM_DESCRIPTION,
        icon: Icon::Interface,
    },
];

impl Kind {
    pub fn descriptor(self) -> &'static Descriptor {
        TOOLS
            .iter()
            .find(|descriptor| descriptor.kind == self)
            .expect("tool descriptor")
    }
}

/// Initial work handed to a tool when it is opened from outside.
#[derive(Clone)]
pub(super) enum Request {
    ErrorCode(u32),
    Com { text: String },
}

impl Request {
    fn kind(&self) -> Kind {
        match self {
            Self::ErrorCode(_) => Kind::ErrorCode,
            Self::Com { .. } => Kind::Com,
        }
    }
}

/// A search-result command owned by one tool.
pub(super) struct Action {
    pub label: u16,
    pub request: Request,
}

/// Search-result actions in tool descriptor order.
pub(super) fn actions(value: Option<&crate::app::result_value::ResultValue>) -> Vec<Action> {
    TOOLS
        .iter()
        .filter_map(|descriptor| match descriptor.kind {
            Kind::ErrorCode => error_code::action(value),
            Kind::Demangle => None,
            Kind::Com => com::action(value),
        })
        .collect()
}

pub(super) enum ToolView {
    ErrorCode(Rc<error_code::View>),
    Demangle(Rc<demangle::View>),
    Com(Rc<com::View>),
}

macro_rules! forward {
    ($this:ident, $view:ident, $value:expr) => {
        match $this {
            Self::ErrorCode($view) => $value,
            Self::Demangle($view) => $value,
            Self::Com($view) => $value,
        }
    };
}

impl ToolView {
    pub fn kind(&self) -> Kind {
        match self {
            Self::ErrorCode(_) => Kind::ErrorCode,
            Self::Demangle(_) => Kind::Demangle,
            Self::Com(_) => Kind::Com,
        }
    }
    pub fn id(&self) -> u64 {
        forward!(self, view, view.id)
    }
    pub fn hwnd(&self) -> HWND {
        forward!(self, view, view.hwnd)
    }
    fn recent(&self) -> u64 {
        forward!(self, view, view.recent.get())
    }
    pub fn placement(&self) -> Option<Placement> {
        forward!(self, view, view.placement.get())
    }
    pub fn set_placement(&self, placement: Placement) {
        forward!(self, view, view.placement.set(Some(placement)))
    }
    pub fn focus(&self) {
        forward!(self, view, view.focus())
    }
    pub fn layout(&self) {
        forward!(self, view, view.layout())
    }
    pub fn prepare(&self) {
        forward!(self, view, view.prepare())
    }
    pub fn recolor(&self) {
        forward!(self, view, view.recolor())
    }
    pub fn status(&self) -> String {
        forward!(self, view, view.status())
    }
    pub fn key(&self, msg: &MSG) -> bool {
        forward!(self, view, view.key(msg))
    }
    pub fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        forward!(self, view, view.message(msg, w, l))
    }
    pub fn destroy(&self) {
        forward!(self, view, view.destroy())
    }
    pub fn activated(&self) {
        forward!(self, view, view.activated())
    }
    pub fn complete(&self, id: u64, completion: crate::tasks::tools::Completion) {
        match self {
            Self::ErrorCode(view) => view.complete(id, completion),
            Self::Com(view) => view.complete(id, completion),
            Self::Demangle(_) => {}
        }
    }
    pub fn worker_stopped(&self) {
        match self {
            Self::ErrorCode(view) => view.worker_stopped(),
            Self::Com(view) => view.worker_stopped(),
            Self::Demangle(_) => {}
        }
    }
    pub fn description(&self) -> String {
        resources::text(self.kind().descriptor().description)
    }
    pub fn apply(&self, request: &Request) {
        match (self, request) {
            (Self::ErrorCode(view), Request::ErrorCode(code)) => view.search_code(*code),
            (Self::Com(view), Request::Com { text: target }) => view.search(target),
            _ => debug_assert!(false, "tool request kind mismatch"),
        }
    }
}

pub(super) fn instantiate(ctx: &Context, kind: Kind, hwnd: HWND) -> anyhow::Result<ToolView> {
    let icon = kind.descriptor().icon;
    let view = match kind {
        Kind::ErrorCode => {
            let mut view = error_code::View::new(ctx, icon);
            view.create(hwnd)?;
            ToolView::ErrorCode(Rc::new(view))
        }
        Kind::Demangle => {
            let mut view = demangle::View::new(ctx, icon);
            view.create(hwnd)?;
            ToolView::Demangle(Rc::new(view))
        }
        Kind::Com => {
            let mut view = com::View::new(ctx, icon);
            view.create(hwnd)?;
            ToolView::Com(Rc::new(view))
        }
    };
    Ok(view)
}

pub(super) fn open(ctx: &Context, kind: Kind, new: bool) -> Option<Rc<ToolView>> {
    history::visit(ctx, || {
        let existing = if new {
            None
        } else {
            workspace::contents(ctx)
                .into_iter()
                .filter_map(|content| content.tool())
                .filter(|view| view.kind() == kind)
                .max_by_key(|view| view.recent())
        };
        let view = if let Some(view) = existing {
            workspace::show_document(ctx, view.hwnd());
            Some(view)
        } else {
            super::status::report(ctx, workspace::create_tool(ctx, kind))
        }?;
        view.focus();
        Some(view)
    })
}

pub(super) fn open_with(ctx: &Context, request: Request) -> Option<Rc<ToolView>> {
    let view = open(ctx, request.kind(), false)?;
    view.apply(&request);
    Some(view)
}

pub(super) fn open_row(ctx: &Context, row: usize, new: bool) {
    if let Some(descriptor) = TOOLS.get(row) {
        open(ctx, descriptor.kind, new);
    }
}

pub(super) fn list(ctx: &Context) -> HWND {
    unsafe { GetDlgItem(ctx.frame(), TOOL_LIST as i32) }
}
pub(super) fn refresh(ctx: &Context) {
    let list = list(ctx);
    crate::list::State::capture(list).apply(list, TOOLS.len());
    super::panel::update_actions(ctx, super::panel::PanelId::Tools);
}
pub(super) fn update_actions(ctx: &Context) {
    unsafe {
        EnableWindow(
            GetDlgItem(ctx.frame(), TOOL_NEW as i32),
            i32::from(crate::list::selected(list(ctx)).is_some_and(|row| row < TOOLS.len())),
        );
    }
}
pub(super) fn selected(ctx: &Context, new: bool) {
    if let Some(row) = crate::list::selected(list(ctx)) {
        open_row(ctx, row, new);
    }
}

/// The model value of the selected combo row, or `fallback` when none is.
fn combo_value<T: Copy>(combo: HWND, values: &[(u16, T)], fallback: T) -> T {
    let selected = send(combo, CB_GETCURSEL, 0, 0);
    values
        .get(selected.max(0) as usize)
        .map(|&(_, value)| value)
        .unwrap_or(fallback)
}
pub(super) fn row(index: usize, visit: &mut dyn FnMut(&Display)) {
    let Some(descriptor) = TOOLS.get(index) else {
        return;
    };
    let description = resources::text(descriptor.description);
    let mut display = Display {
        title: resources::text(descriptor.title).encode_utf16().collect(),
        icon: Some(descriptor.icon),
        tooltip: description.encode_utf16().collect(),
        ..Default::default()
    };
    display.detail.push(TextSegment::plain(&description));
    visit(&display);
}
pub(super) fn row_text(index: usize) -> Vec<u16> {
    let Some(descriptor) = TOOLS.get(index) else {
        return Vec::new();
    };
    format!(
        "{}\r\n{}",
        resources::text(descriptor.title),
        resources::text(descriptor.description)
    )
    .encode_utf16()
    .collect()
}
pub(super) fn context_menu(ctx: &Context, hwnd: HWND, l: isize) -> anyhow::Result<()> {
    let Some((row, point)) = crate::list::context_target(hwnd, l) else {
        return Ok(());
    };
    let Some(descriptor) = TOOLS.get(row) else {
        return Ok(());
    };
    let menu = Popup::new()?;
    menu.item(1, resources::wide_text(IDS_OPEN), true)?;
    menu.item(2, resources::wide_text(IDS_TOOL_NEW), true)?;
    match menu.track(ctx.frame(), point) {
        1 => {
            open(ctx, descriptor.kind, false);
        }
        2 => {
            open(ctx, descriptor.kind, true);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_descriptors_are_unique_and_addressable_by_kind() {
        let mut kinds = std::collections::HashSet::new();
        for descriptor in TOOLS {
            assert!(kinds.insert(descriptor.kind));
            let resolved = descriptor.kind.descriptor();
            assert_eq!(resolved.kind, descriptor.kind);
            assert_eq!(resolved.title, descriptor.title);
            assert_eq!(resolved.description, descriptor.description);
            assert_ne!(descriptor.title, descriptor.description);
        }
    }

    #[test]
    fn rows_beyond_the_descriptor_table_are_empty() {
        let mut visits = 0;
        row(TOOLS.len(), &mut |_| visits += 1);
        assert_eq!(visits, 0);
        assert!(row_text(TOOLS.len()).is_empty());
    }

    #[test]
    fn every_request_names_its_tool_descriptor() {
        assert_eq!(
            Request::ErrorCode(5).kind().descriptor().kind,
            Kind::ErrorCode
        );
    }

    #[test]
    fn search_result_actions_follow_the_descriptor_order() {
        use crate::app::result_value::ResultValue;
        assert!(actions(None).is_empty());
        let integer = ResultValue {
            text: "5".into(),
            hexadecimal: Some("0x00000005".into()),
            integer: Some(5),
        };
        let acts = actions(Some(&integer));
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].label, IDS_ERROR_CODE_SEARCH);
        match &acts[0].request {
            Request::ErrorCode(code) => assert_eq!(*code, 5),
            Request::Com { .. } => panic!("unexpected COM action"),
        }
        let string_value = ResultValue {
            text: "Windows".into(),
            hexadecimal: None,
            integer: None,
        };
        assert!(actions(Some(&string_value)).is_empty());
        let guid = ResultValue {
            text: "{00000000-0000-0000-0000-000000000001}".into(),
            hexadecimal: None,
            integer: None,
        };
        let acts = actions(Some(&guid));
        assert_eq!(acts.len(), 1);
        assert_eq!(acts[0].label, IDS_COM_SEARCH);
        match &acts[0].request {
            Request::Com { text: target } => assert_eq!(target, &guid.text),
            Request::ErrorCode(_) => panic!("unexpected error-code action"),
        }
    }
}

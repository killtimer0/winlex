//! Error-code controls and instance-local asynchronous message queries.
use super::Descriptor;
use crate::app::context::Context;
use crate::native::text;
use crate::{
    layout,
    native::*,
    resources::{self, *},
    tasks::{Job, tools::Job as ToolJob},
};
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    rc::Rc,
};
use winlex_core::{
    number::IntegerLiteral,
    tools::error_code::{
        self as core, Message, MessageStatus, QueryResult, Request, Scheme, Scope, Source,
    },
};

const TEXT_SUBCLASS: usize = 1810;
fn item_code(value: Option<&crate::app::result_value::ResultValue>) -> Option<u32> {
    let (number, _) = value?.integer?;
    if number < 0 {
        i32::try_from(number).ok().map(|number| number as u32)
    } else {
        u32::try_from(number).ok()
    }
}
pub(super) fn item_actions(
    descriptor: &'static Descriptor,
    _binding: &crate::app::metadata::Binding,
    value: Option<&crate::app::result_value::ResultValue>,
) -> Vec<super::ItemAction> {
    let Some(code) = item_code(value) else {
        return Vec::new();
    };
    vec![super::ItemAction {
        label: IDS_ERROR_CODE_SEARCH,
        run: Box::new(move |ctx| {
            if let Some(view) = super::open_descriptor(ctx, descriptor, false) {
                view.search_code(code);
            }
        }),
    }]
}
const SCOPES: &[(u16, Scope)] = &[
    (IDS_ERROR_CODE_AUTO, Scope::Auto),
    (IDS_ERROR_CODE_WIN32, Scope::Win32),
    (IDS_ERROR_CODE_HRESULT, Scope::HResult),
    (IDS_ERROR_CODE_NTSTATUS, Scope::NtStatus),
    (IDS_ERROR_CODE_ERRNO, Scope::Errno),
];

pub(in crate::app) struct View {
    // The document lives inside the context, so the pointer cannot outlive it.
    ctx: *const Context,
    pub id: u64,
    pub hwnd: HWND,
    pub descriptor: &'static Descriptor,
    pub lifetime: Rc<()>,
    pub placement: Cell<Option<layout::Placement>>,
    pub live: Cell<bool>,
    pub(super) recent: Cell<u64>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    input: HWND,
    scope: HWND,
    language: HWND,
    module: HWND,
    browse: HWND,
    query: HWND,
    result: HWND,
    status: RefCell<String>,
    tooltip: HWND,
    copy: HWND,
    labels: Vec<HWND>,
    setting: Cell<bool>,
    revision: Cell<u64>,
    query_ticket: Cell<Option<u64>>,
    languages_ticket: Cell<Option<u64>>,
    languages: RefCell<Vec<u16>>,
    body: RefCell<crate::rich_text::Content>,
}
impl View {
    fn ctx(&self) -> &Context {
        unsafe { &*self.ctx }
    }
    pub fn new(ctx: &Context, descriptor: &'static Descriptor) -> Self {
        Self {
            ctx: ctx as *const Context,
            id: super::super::workspace::next_id(ctx),
            hwnd: std::ptr::null_mut(),
            descriptor,
            lifetime: Rc::new(()),
            placement: Cell::new(None),
            live: Cell::new(true),
            recent: Cell::new(0),
            icons: RefCell::new(None),
            input: std::ptr::null_mut(),
            scope: std::ptr::null_mut(),
            language: std::ptr::null_mut(),
            module: std::ptr::null_mut(),
            browse: std::ptr::null_mut(),
            query: std::ptr::null_mut(),
            result: std::ptr::null_mut(),
            status: RefCell::new(resources::text(IDS_ERROR_CODE_READY)),
            tooltip: std::ptr::null_mut(),
            copy: std::ptr::null_mut(),
            labels: vec![],
            setting: Cell::new(true),
            revision: Cell::new(0),
            query_ticket: Cell::new(None),
            languages_ticket: Cell::new(None),
            languages: RefCell::new(vec![]),
            body: RefCell::new(crate::rich_text::Content::new(String::new())),
        }
    }
}
fn native_text(hwnd: HWND) -> Vec<u16> {
    let mut text = text_wide(hwnd);
    text.pop();
    text
}
fn language_identity(ids: &[u16], selected: isize, value: &str) -> Option<u16> {
    ids.get(selected as usize)
        .copied()
        .filter(|id| language_label(*id) == value)
}
impl View {
    pub fn title(&self) -> String {
        if self.hwnd.is_null() {
            resources::text(self.descriptor.title)
        } else {
            text(self.hwnd)
        }
    }
    fn search_code(&self, code: u32) {
        self.cancel_languages();
        if let Some(id) = self.query_ticket.take() {
            self.ctx().cancel_job(id);
        }
        self.setting.set(true);
        send(self.scope, CB_SETCURSEL, 0, 0);
        set_text(self.input, &format!("0x{code:08X}"));
        unsafe {
            EnableWindow(self.language, 1);
            EnableWindow(self.query, 1);
        }
        self.system_languages(false);
        self.setting.set(false);
        self.changed();
        self.layout();
        self.execute();
        self.focus();
    }
    fn update_icons(&self) {
        let ctx = self.ctx();
        super::super::status::report(
            ctx,
            super::super::document::update_icons(ctx, self.hwnd, self.descriptor.icon, &self.icons),
        );
    }
    pub fn activated(&self) {
        self.recent
            .set(super::super::workspace::next_tool_clock(self.ctx()));
    }
    fn command(&self, id: u16, code: u32) -> bool {
        if self.setting.get() {
            return false;
        }
        match id {
            ERROR_CODE_INPUT | ERROR_CODE_MODULE if code == EN_CHANGE => {
                self.changed();
                if id == ERROR_CODE_MODULE && self.custom() {
                    self.cancel_languages();
                    self.set_languages(vec![0], true);
                }
            }
            ERROR_CODE_MODULE if code == EN_KILLFOCUS => self.refresh_module_languages(),
            ERROR_CODE_SCOPE if code == CBN_SELENDOK => {
                self.cancel_languages();
                self.changed();
                let errno = self.scope() == Scope::Errno;
                unsafe { EnableWindow(self.language, i32::from(!errno)) };
                if self.custom() {
                    self.set_languages(vec![0], true);
                    self.refresh_module_languages();
                } else if errno {
                    self.set_languages(vec![0x409], false);
                    send(self.language, CB_SETCURSEL, 1, 0);
                } else {
                    self.system_languages(true);
                }
                self.layout();
            }
            ERROR_CODE_LANGUAGE if code == CBN_EDITCHANGE || code == CBN_SELENDOK => self.changed(),
            ERROR_CODE_QUERY if code == BN_CLICKED => {
                self.refresh_module_languages();
                self.execute();
            }
            ERROR_CODE_BROWSE if code == BN_CLICKED => self.browse(),
            ERROR_CODE_COPY_ALL if code == BN_CLICKED => {
                if let Err(error) = copy_unicode(self.hwnd, &native_text(self.result)) {
                    self.set_status(&format!("{error:#}"));
                }
            }
            _ => return false,
        };
        true
    }
    fn set_status(&self, message: &str) {
        self.status.replace(message.to_owned());
        super::super::document::status_changed(self.ctx(), self.hwnd);
    }
    fn custom(&self) -> bool {
        send(self.scope, CB_GETCURSEL, 0, 0) == SCOPES.len() as isize
    }
    fn path(&self) -> PathBuf {
        PathBuf::from(OsString::from_wide(&native_text(self.module)))
    }
    fn scope(&self) -> Scope {
        SCOPES
            .get(send(self.scope, CB_GETCURSEL, 0, 0) as usize)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| Scope::Module(self.path()))
    }
    fn request(&self) -> anyhow::Result<Request> {
        let code = IntegerLiteral::parse(text(self.input).trim())?.to_code_u32()?;
        let (value, language) = self.selected_language();
        let language = match language {
            Some(id) => id,
            None => IntegerLiteral::parse(value.trim())?.to_u16()?,
        };
        let request = Request {
            code,
            scope: self.scope(),
            language,
        };
        request.validate()?;
        Ok(request)
    }
    fn changed(&self) {
        if self.setting.get() {
            return;
        }
        self.revision.set(self.revision.get() + 1);
        self.set_status(&resources::text(IDS_ERROR_CODE_STALE));
    }
    fn selected_language(&self) -> (String, Option<u16>) {
        let selected = send(self.language, CB_GETCURSEL, 0, 0);
        let value = text(self.language);
        let id = language_identity(&self.languages.borrow(), selected, &value);
        (value, id)
    }
    fn set_languages(&self, ids: Vec<u16>, preserve: bool) {
        let previous = preserve.then(|| self.selected_language());
        let was_setting = self.setting.replace(true);
        let ids = order_languages(ids, resources::interface_langid(), unsafe {
            windows_sys::Win32::Globalization::GetUserDefaultUILanguage()
        });
        send(self.language, CB_RESETCONTENT, 0, 0);
        for &id in &ids {
            send(
                self.language,
                CB_ADDSTRING,
                0,
                wide(&language_label(id)).as_ptr() as isize,
            );
        }
        if let Some((previous, id)) = previous {
            if let Some(index) = id.and_then(|id| ids.iter().position(|value| *value == id)) {
                send(self.language, CB_SETCURSEL, index, 0);
            } else {
                set_text(
                    self.language,
                    &id.map(|id| format!("0x{id:04X}")).unwrap_or(previous),
                );
            }
        } else {
            send(self.language, CB_SETCURSEL, 0, 0);
        }
        self.languages.replace(ids);
        self.setting.set(was_setting);
    }
    fn system_languages(&self, preserve: bool) {
        let mut ids = core::system_languages();
        ids.push(resources::interface_langid());
        self.set_languages(ids, preserve);
    }
    fn submit(&self, pending: &Cell<Option<u64>>, job: ToolJob) -> anyhow::Result<()> {
        let ctx = self.ctx();
        let ticket = crate::app::catalog::next_ticket(ctx);
        let id = ticket.id;
        let previous = pending.take();
        ctx.replace_job(
            previous,
            ticket,
            Job::Tool {
                document: self.id,
                job,
            },
        )?;
        pending.set(Some(id));
        Ok(())
    }
    fn cancel_languages(&self) {
        if let Some(id) = self.languages_ticket.take() {
            self.ctx().cancel_job(id);
        }
    }
    fn refresh_module_languages(&self) {
        if !self.custom() {
            return;
        }
        let path = self.path();
        if path.as_os_str().is_empty() {
            return;
        }
        if let Err(error) = self.submit(&self.languages_ticket, ToolJob::Languages { path }) {
            self.set_status(&format!("{error:#}"));
        }
    }
    fn execute(&self) {
        if unsafe { IsWindowEnabled(self.query) } == 0 {
            return;
        }
        let request = match self.request() {
            Ok(request) => request,
            Err(error) => {
                self.set_status(&format!(
                    "{}: {error:#}",
                    resources::text(IDS_ERROR_CODE_INVALID)
                ));
                return;
            }
        };
        let revision = self.revision.get();
        match self.submit(&self.query_ticket, ToolJob::Query { revision, request }) {
            Ok(()) => {
                unsafe { EnableWindow(self.query, 0) };
                self.set_status(&resources::text(IDS_ERROR_CODE_QUERYING));
            }
            Err(error) => self.set_status(&format!("{error:#}")),
        }
    }
    pub(super) fn query_complete(
        &self,
        id: u64,
        revision: u64,
        result: anyhow::Result<QueryResult>,
    ) {
        if self.query_ticket.get() != Some(id) {
            return;
        }
        self.query_ticket.set(None);
        unsafe { EnableWindow(self.query, 1) };
        let body = match result {
            Ok(result) => {
                super::super::document::update_title(
                    self.ctx(),
                    self.hwnd,
                    &format!(
                        "{} — 0x{:08X}",
                        resources::text(IDS_ERROR_CODE_TOOL),
                        result.request.code
                    ),
                );
                render(&result)
            }
            Err(error) => format!("{}: {error:#}", resources::text(IDS_ERROR_CODE_FAILED)),
        };
        let body = crate::rich_text::Content::new(body);
        if let Err(error) = crate::rich_text::set_content(self.result, &body) {
            self.set_status(&format!("{error:#}"));
        } else {
            self.body.replace(body);
            self.set_status(&if revision != self.revision.get() {
                resources::text(IDS_ERROR_CODE_STALE)
            } else {
                String::new()
            });
        }
    }
    pub(super) fn languages_complete(
        &self,
        id: u64,
        path: PathBuf,
        result: Result<Vec<u16>, core::Failure>,
    ) {
        if self.languages_ticket.get() != Some(id) {
            return;
        }
        self.languages_ticket.set(None);
        if self.custom() && self.path() == path {
            match result {
                Ok(ids) => self.set_languages(ids, true),
                Err(error) => self.set_status(&format!(
                    "{}: {}",
                    resources::text(IDS_ERROR_CODE_LANGUAGES_FAILED),
                    error.detail
                )),
            }
        }
    }
    fn browse(&self) {
        match crate::input::choose(
            self.hwnd,
            self.ctx().system_version(),
            IDS_ERROR_CODE_TOOL,
            IDS_ERROR_CODE_MODULE_FILES,
            "*.exe;*.dll;*.mui",
            false,
        )
        .map(|paths| paths.into_iter().next())
        {
            Ok(Some(path)) => {
                let value: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                unsafe { SetWindowTextW(self.module, value.as_ptr()) };
                self.refresh_module_languages();
            }
            Ok(None) => {}
            Err(error) => self.set_status(&format!("{error:#}")),
        }
    }
}
impl View {
    pub fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        for (label, id, control, class, style, edge) in [
            (
                IDS_ERROR_CODE_LABEL,
                ERROR_CODE_INPUT,
                &mut self.input,
                w!("EDIT"),
                ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
            ),
            (
                IDS_ERROR_CODE_SCOPE,
                ERROR_CODE_SCOPE,
                &mut self.scope,
                w!("COMBOBOX"),
                WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
            ),
            (
                IDS_ERROR_CODE_LANGUAGE,
                ERROR_CODE_LANGUAGE,
                &mut self.language,
                w!("COMBOBOX"),
                WS_VSCROLL | CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32,
                0,
            ),
            (
                IDS_ERROR_CODE_MODULE,
                ERROR_CODE_MODULE,
                &mut self.module,
                w!("EDIT"),
                ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
            ),
        ] {
            self.labels
                .push(unsafe { child(hwnd, w!("STATIC"), resources::wide_text(label), 0, 0) }?);
            *control = unsafe { child_ex(hwnd, class, &[0], WS_TABSTOP | style, edge, id) }?;
        }
        for &(label, _) in SCOPES {
            send(
                self.scope,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(
            self.scope,
            CB_ADDSTRING,
            0,
            resources::wide_text(IDS_ERROR_CODE_CUSTOM).as_ptr() as isize,
        );
        send(self.scope, CB_SETCURSEL, 0, 0);
        self.system_languages(false);
        for (control, id, label, style) in [
            (
                &mut self.browse,
                ERROR_CODE_BROWSE,
                IDS_ERROR_CODE_BROWSE,
                0,
            ),
            (
                &mut self.query,
                ERROR_CODE_QUERY,
                IDS_ERROR_CODE_QUERY,
                BS_DEFPUSHBUTTON as u32,
            ),
            (
                &mut self.copy,
                ERROR_CODE_COPY_ALL,
                IDS_ERROR_CODE_COPY_ALL,
                0,
            ),
        ] {
            *control = unsafe {
                child(
                    hwnd,
                    w!("BUTTON"),
                    resources::wide_text(label),
                    WS_TABSTOP | style,
                    id,
                )
            }?;
        }
        let tool = TTTOOLINFOW {
            cbSize: TOOLINFO_SIZE,
            uFlags: TTF_IDISHWND | TTF_SUBCLASS,
            hwnd: self.input,
            uId: self.input as usize,
            lpszText: resources::wide_text(IDS_ERROR_CODE_HINT)
                .as_ptr()
                .cast_mut(),
            ..unsafe { std::mem::zeroed() }
        };
        self.labels.push(unsafe {
            child(
                hwnd,
                w!("STATIC"),
                resources::wide_text(IDS_ERROR_CODE_RESULT_LABEL),
                0,
                0,
            )
        }?);
        self.result = crate::rich_text::create(hwnd, ERROR_CODE_RESULT)?;
        send(self.input, EM_SETLIMITTEXT, 4096, 0);
        send(self.module, EM_SETLIMITTEXT, 32767, 0);
        for control in [self.input, self.module] {
            if unsafe { set_subclass(control, Some(text_proc), TEXT_SUBCLASS, 0) } == 0 {
                return Err(error("Subclass tool text control"));
            }
        }
        self.tooltip = tooltip(hwnd, &tool)?;
        self.setting.set(false);
        Ok(())
    }
    pub fn layout(&self) {
        if self.result.is_null() {
            return;
        }
        let m = self.ctx().metrics(self.hwnd);
        let mut area = layout::inset(client(self.hwnd), m.px(8));
        let mut positions = vec![];
        let gap = m.px(4);
        let label_gap = m.px(8);
        let label_width = [
            IDS_ERROR_CODE_LABEL,
            IDS_ERROR_CODE_SCOPE,
            IDS_ERROR_CODE_LANGUAGE,
            IDS_ERROR_CODE_MODULE,
        ]
        .into_iter()
        .map(|id| m.text_width(resources::wide_text(id)))
        .max()
        .unwrap_or(0)
        .min((area.right - area.left) / 3);
        let label_column = label_width + label_gap;
        let browse = m.button_size(self.browse);
        let dpi = self.ctx().dpi();
        let heights = [
            m.edit_height(self.input, dpi),
            {
                let r = bounds(self.scope, self.hwnd);
                r.bottom - r.top
            },
            {
                let r = bounds(self.language, self.hwnd);
                r.bottom - r.top
            },
            m.edit_height(self.module, dpi),
        ];
        let combo_bounds = |r: RECT, height: i32| {
            let mut r = layout::center_vertical(r, height);
            r.bottom = r.top + height.max(m.row() * 9);
            r
        };
        let paired = area.right - area.left >= label_column * 2 + m.px(360);
        for (index, control) in [self.input, self.scope, self.language, self.module]
            .into_iter()
            .enumerate()
        {
            let visible = index != 3 || self.custom();
            show(self.labels[index], visible);
            show(control, visible);
            if !visible || index == 2 && paired {
                continue;
            }
            let wrap_browse =
                index == 3 && area.right - area.left < label_column + m.px(80) + gap + browse.cx;
            let mut row = heights[index].max(m.height);
            if index == 1 && paired {
                row = row.max(heights[2]);
            }
            if index == 3 && !wrap_browse {
                row = row.max(browse.cy);
            }
            let mut line = layout::take_top(&mut area, row);
            let label = layout::take_left(&mut line, label_width);
            positions.push((self.labels[index], layout::center_vertical(label, m.height)));
            layout::take_left(&mut line, label_gap);
            if index == 3 && !wrap_browse {
                let right = line.right;
                line.right = (right - browse.cx - gap).max(line.left);
                positions.push((
                    self.browse,
                    layout::center_vertical(
                        layout::rect(
                            right - browse.cx,
                            line.top,
                            browse.cx,
                            line.bottom - line.top,
                        ),
                        browse.cy,
                    ),
                ));
            }
            if index == 1 && paired {
                let mut language = line;
                let width = (line.right - line.left - label_column - label_gap) / 2;
                line.right = line.left + width;
                language.left = line.right + label_gap;
                let label = layout::take_left(&mut language, label_width);
                positions.push((self.labels[2], layout::center_vertical(label, m.height)));
                layout::take_left(&mut language, label_gap);
                positions.push((self.language, combo_bounds(language, heights[2])));
            }
            positions.push((
                control,
                if index == 1 || index == 2 {
                    combo_bounds(line, heights[index])
                } else {
                    layout::center_vertical(line, heights[index])
                },
            ));
            layout::take_top(&mut area, gap);
            if wrap_browse {
                let mut line = layout::take_top(&mut area, browse.cy);
                let indent = label_column.min((line.right - line.left - browse.cx).max(0));
                layout::take_left(&mut line, indent);
                let button = layout::take_left(&mut line, browse.cx);
                positions.push((self.browse, button));
                layout::take_top(&mut area, gap);
            }
        }
        show(self.browse, self.custom());
        m.buttons(&mut area, [self.query, self.copy], &mut positions);
        layout::take_top(&mut area, m.px(4));
        positions.push((
            self.labels[4],
            layout::take_top(&mut area, m.height + m.px(4)),
        ));
        positions.push((self.result, area));
        if let Err(error) = place_all(&positions) {
            self.set_status(&format!("{error:#}"));
        }
    }
    pub fn focus(&self) {
        focus(self.input)
    }
    pub fn prepare(&self) {
        self.update_icons();
        let m = self.ctx().metrics(self.hwnd);
        for h in self.labels.iter().copied().chain([
            self.input,
            self.scope,
            self.language,
            self.module,
            self.browse,
            self.query,
            self.tooltip,
            self.result,
            self.copy,
        ]) {
            super::super::bind(h, m.font);
        }
        crate::rich_text::font(self.result);
        send(self.tooltip, TTM_SETMAXTIPWIDTH, 0, m.px(480) as isize);
        self.layout();
    }
    pub fn recolor(&self) {
        self.update_icons();
        let _ = crate::rich_text::recolor(self.result, &self.body.borrow());
    }
    pub fn status(&self) -> String {
        self.status.borrow().clone()
    }
    pub fn key(&self, msg: &MSG) -> bool {
        if msg.message == WM_KEYDOWN
            && msg.wParam == VK_A as usize
            && unsafe { GetKeyState(VK_CONTROL as i32) } < 0
            && ([self.input, self.module, self.result].contains(&msg.hwnd)
                || unsafe { IsChild(self.language, msg.hwnd) } != 0)
        {
            send(msg.hwnd, EM_SETSEL, 0, -1);
            return true;
        }
        if msg.message == WM_KEYDOWN
            && msg.hwnd == self.input
            && msg.wParam == VK_RETURN as usize
            && unsafe { GetPropW(self.input, w!("Winlex.Tool.Composing")) }.is_null()
        {
            self.execute();
            return true;
        }
        false
    }
    pub fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        match msg {
            WM_COMMAND if self.command(w as u16, (w >> 16) as u32) => Some(0),
            WM_CONTEXTMENU if w as HWND == self.result => {
                let _ = crate::rich_text::context_menu(self.result, l, &[], None);
                Some(0)
            }
            _ => None,
        }
    }
    pub fn destroy(&self) {
        if let Some(id) = self.query_ticket.take() {
            self.ctx().cancel_job(id);
        }
        self.cancel_languages();
        unsafe {
            DestroyWindow(self.tooltip);
        }
    }
    pub(super) fn worker_stopped(&self) {
        let querying = self.query_ticket.take().is_some();
        let languages = self.languages_ticket.take().is_some();
        if querying || languages {
            unsafe { EnableWindow(self.query, 1) };
            self.set_status(&resources::text(IDS_WORKER_STOPPED));
        }
    }
}

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

fn locale_info(id: u16, kind: u32) -> String {
    let mut b = vec![0; 256];
    let n = unsafe {
        windows_sys::Win32::Globalization::GetLocaleInfoW(
            id as u32,
            kind,
            b.as_mut_ptr(),
            b.len() as i32,
        )
    };
    if n > 0 {
        String::from_utf16_lossy(&b[..n as usize - 1])
    } else {
        String::new()
    }
}
fn language_tag(id: u16) -> String {
    use windows_sys::Win32::Globalization::*;
    let language = locale_info(id, LOCALE_SISO639LANGNAME);
    let country = locale_info(id, LOCALE_SISO3166CTRYNAME);
    if country.is_empty() {
        language
    } else {
        format!("{language}-{country}")
    }
}
fn language_label(id: u16) -> String {
    if id == 0 {
        return resources::text(IDS_ERROR_CODE_AUTO_LANGUAGE);
    }
    let name = locale_info(id, windows_sys::Win32::Globalization::LOCALE_SLANGUAGE);
    let tag = language_tag(id);
    let mut fields = vec![];
    if !name.is_empty() {
        fields.push(name);
    }
    if !tag.is_empty() {
        fields.push(tag);
    }
    fields.push(format!("0x{id:04X}"));
    fields.join(" · ")
}
fn order_languages(mut ids: Vec<u16>, interface: u16, user: u16) -> Vec<u16> {
    ids.push(0);
    ids.sort_unstable();
    ids.dedup();
    ids.sort_by_cached_key(|id| {
        let priority = if *id == 0 {
            0
        } else if *id == interface {
            1
        } else if *id == user {
            2
        } else if *id == 0x409 {
            3
        } else {
            4
        };
        (priority, language_tag(*id))
    });
    ids
}
fn scope_label(scope: &Scope) -> u16 {
    SCOPES
        .iter()
        .find(|(_, value)| value == scope)
        .map_or(IDS_ERROR_CODE_CUSTOM, |(label, _)| *label)
}
fn render(result: &QueryResult) -> String {
    let mut lines = vec![
        resources::text(IDS_ERROR_CODE_REQUEST),
        format!(
            "{} 0x{:08X}",
            resources::text(IDS_ERROR_CODE_LABEL),
            result.request.code
        ),
        resources::text(IDS_ERROR_CODE_DECIMAL)
            .replace("{unsigned}", &result.request.code.to_string())
            .replace("{signed}", &(result.request.code as i32).to_string()),
        format!(
            "{} {}",
            resources::text(IDS_ERROR_CODE_SCOPE),
            resources::text(scope_label(&result.request.scope))
        ),
        format!(
            "{} {}",
            resources::text(IDS_ERROR_CODE_LANGUAGE),
            language_label(result.request.language)
        ),
    ];
    if let Scope::Module(path) = &result.request.scope {
        lines.push(format!(
            "{} {}",
            resources::text(IDS_ERROR_CODE_MODULE),
            path.display()
        ));
    }
    if result.candidates.is_empty() {
        lines.push(String::new());
        lines.push(resources::text(IDS_ERROR_CODE_NO_MESSAGE));
    }
    for candidate in &result.candidates {
        lines.push(String::new());
        lines.push(resources::text(match candidate.scheme {
            Scheme::Win32 => IDS_ERROR_CODE_WIN32,
            Scheme::HResult => IDS_ERROR_CODE_HRESULT,
            Scheme::NtStatus => IDS_ERROR_CODE_NTSTATUS,
            Scheme::Errno => IDS_ERROR_CODE_ERRNO,
            Scheme::Module => IDS_ERROR_CODE_CUSTOM,
        }));
        if let Some(e) = &candidate.encoding {
            lines.push(
                resources::text(IDS_ERROR_CODE_ENCODING)
                    .replace("{severity}", &e.severity.to_string())
                    .replace("{customer}", &u8::from(e.customer).to_string())
                    .replace("{facility}", &e.facility.to_string())
                    .replace("{code}", &e.code.to_string())
                    .replace("{nt}", &u8::from(e.nt_mapping).to_string()),
            );
        }
        render_message(&mut lines, &candidate.message);
        for related in &candidate.related {
            lines.push(String::new());
            lines.push(format!(
                "{}: 0x{:08X} ({})",
                resources::text(IDS_ERROR_CODE_RELATED),
                related.message.code,
                related.message.code
            ));
            render_message(&mut lines, &related.message);
        }
    }
    for attempt in &result.attempts {
        if attempt.status != MessageStatus::NotFound {
            lines.push(String::new());
            render_message(&mut lines, attempt);
        }
    }
    lines.join("\r\n")
}
fn render_message(lines: &mut Vec<String>, message: &Message) {
    let source = match &message.source {
        Source::System => resources::text(IDS_ERROR_CODE_SYSTEM),
        Source::NtDll => resources::text(IDS_ERROR_CODE_NTDLL),
        Source::Msvcrt => resources::text(IDS_ERROR_CODE_MSVCRT),
        Source::Module(path) => path.display().to_string(),
    };
    lines.push(format!(
        "{}: {source} · 0x{:08X}",
        resources::text(IDS_ERROR_CODE_SOURCE),
        message.code
    ));
    lines.push(if let Some(id) = message.actual_language {
        if id == 0 {
            resources::text(IDS_ERROR_CODE_NEUTRAL_LANGUAGE)
        } else {
            language_label(id)
        }
    } else if message.requested_language == 0 {
        resources::text(IDS_ERROR_CODE_SYSTEM_LANGUAGE)
    } else {
        language_label(message.requested_language)
    });
    lines.push(match message.status {
        MessageStatus::Found => message.text.clone().unwrap_or_default(),
        MessageStatus::NotFound => resources::text(IDS_ERROR_CODE_NO_MESSAGE),
        MessageStatus::LanguageNotFound => resources::text(IDS_ERROR_CODE_NO_LANGUAGE),
        MessageStatus::NoMessageTable => resources::text(IDS_ERROR_CODE_NO_TABLE),
        MessageStatus::Failed => resources::text(IDS_ERROR_CODE_FAILED),
    });
    if let Some(error) = &message.failure {
        lines.push(format!(
            "{}: {}{}",
            error.operation,
            error.detail,
            error
                .os_code
                .map(|c| format!(" (0x{c:08X})"))
                .unwrap_or_default()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_actions_accept_only_checked_32_bit_integer_codes() {
        use crate::app::result_value::ResultValue;
        assert_eq!(item_code(None), None);
        for (number, bits, expected) in [
            (0, 8, Some(0)),
            (-1, 8, Some(u32::MAX)),
            (i32::MIN as i128, 32, Some(0x80000000)),
            (u32::MAX as i128, 64, Some(u32::MAX)),
            (i32::MIN as i128 - 1, 64, None),
            (u32::MAX as i128 + 1, 64, None),
        ] {
            let value = ResultValue {
                text: number.to_string(),
                hexadecimal: None,
                integer: Some((number, bits)),
            };
            assert_eq!(item_code(Some(&value)), expected);
        }
        let value = ResultValue {
            text: "5".into(),
            hexadecimal: None,
            integer: None,
        };
        assert_eq!(item_code(Some(&value)), None);
    }
    #[test]
    fn language_identity_requires_the_selected_label_to_match_editable_text() {
        let ids = [0x409, 0x804];
        for (index, id) in ids.into_iter().enumerate() {
            assert_eq!(
                language_identity(&ids, index as isize, &language_label(id)),
                Some(id)
            );
            for value in ["0x0409", "409", "unknown", ""] {
                assert_eq!(language_identity(&ids, index as isize, value), None);
            }
        }
        assert_eq!(language_identity(&ids, -1, &language_label(ids[0])), None);
        assert_eq!(language_identity(&[], 0, &language_label(ids[0])), None);
    }
    #[test]
    fn native_text_keeps_unpaired_utf16_units_without_a_terminator() {
        unsafe {
            let value = [67, 58, 92, 0xd800, 0];
            let control = test_window(
                w!("EDIT"),
                value.as_ptr(),
                WS_POPUP,
                layout::rect(0, 0, 300, 200),
            );
            assert_eq!(native_text(control), value[..value.len() - 1]);
            DestroyWindow(control);
        }
    }
    #[test]
    fn scopes_have_distinct_resource_and_model_identities() {
        assert_eq!(
            scope_label(&Scope::Module(PathBuf::from("messages.dll"))),
            IDS_ERROR_CODE_CUSTOM
        );
        for (i, (label, scope)) in SCOPES.iter().enumerate() {
            assert_eq!(scope_label(scope), *label);
            assert_ne!(*label, IDS_ERROR_CODE_CUSTOM);
            for (other_label, other_scope) in &SCOPES[i + 1..] {
                assert_ne!(label, other_label);
                assert_ne!(scope, other_scope);
            }
        }
    }
}

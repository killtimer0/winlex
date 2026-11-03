//! Error-code controls and instance-local asynchronous resource queries.
use super::ToolContent;
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    sync::mpsc,
};
use winlex_core::{
    number::IntegerLiteral,
    tools::error_code::{
        self as core, Message, MessageStatus, QueryResult, Request, Scheme, Scope, Source,
    },
};

const TIMER: usize = 1810;
const SCOPES: &[(u16, Scope)] = &[
    (IDS_ERROR_CODE_AUTO, Scope::Auto),
    (IDS_ERROR_CODE_WIN32, Scope::Win32),
    (IDS_ERROR_CODE_HRESULT, Scope::HResult),
    (IDS_ERROR_CODE_NTSTATUS, Scope::NtStatus),
];

pub(super) fn create() -> Box<dyn ToolContent> {
    Box::new(View::default())
}
enum Completion {
    Query {
        revision: u64,
        result: anyhow::Result<QueryResult>,
    },
    Languages {
        path: PathBuf,
        result: Result<Vec<u16>, core::Failure>,
    },
}
struct View {
    hwnd: HWND,
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
    pending: Cell<usize>,
    querying: Cell<bool>,
    languages: RefCell<Vec<u16>>,
    body: RefCell<crate::rich_text::Content>,
    tx: mpsc::Sender<Completion>,
    rx: RefCell<mpsc::Receiver<Completion>>,
}
impl Default for View {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            hwnd: std::ptr::null_mut(),
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
            pending: Cell::new(0),
            querying: Cell::new(false),
            languages: RefCell::new(vec![]),
            body: RefCell::new(crate::rich_text::Content::new(String::new())),
            tx,
            rx: RefCell::new(rx),
        }
    }
}
fn native_text(hwnd: HWND) -> Vec<u16> {
    let length = unsafe { GetWindowTextLengthW(hwnd) }.max(0) as usize;
    let mut text = vec![0; length + 1];
    let count =
        unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) }.max(0) as usize;
    text.truncate(count);
    text
}
fn text(hwnd: HWND) -> String {
    String::from_utf16_lossy(&native_text(hwnd))
}
impl View {
    fn command(&self, id: u16, code: u32) -> bool {
        if self.setting.get() {
            return false;
        }
        match id {
            ERROR_CODE_INPUT | ERROR_CODE_MODULE if code == EN_CHANGE => {
                self.changed();
                if id == ERROR_CODE_MODULE && self.custom() {
                    self.set_languages(vec![0], true);
                }
            }
            ERROR_CODE_MODULE if code == EN_KILLFOCUS => self.refresh_module_languages(),
            ERROR_CODE_SCOPE if code == CBN_SELENDOK => {
                self.changed();
                if self.custom() {
                    self.set_languages(vec![0], true);
                    self.refresh_module_languages();
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
        super::super::document::status_changed(self.hwnd);
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
        let selected = send(self.language, CB_GETCURSEL, 0, 0);
        let value = text(self.language);
        let language = self
            .languages
            .borrow()
            .get(selected as usize)
            .copied()
            .filter(|id| language_label(*id) == value);
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
    fn set_languages(&self, ids: Vec<u16>, preserve: bool) {
        let previous = preserve.then(|| {
            let selected = send(self.language, CB_GETCURSEL, 0, 0);
            let value = text(self.language);
            let id = self
                .languages
                .borrow()
                .get(selected as usize)
                .copied()
                .filter(|id| language_label(*id) == value);
            (value, id)
        });
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
    fn queue(
        &self,
        job: impl FnOnce(mpsc::Sender<Completion>) + Send + 'static,
    ) -> anyhow::Result<()> {
        if unsafe { SetTimer(self.hwnd, TIMER, 75, None) } == 0 {
            return Err(error("Start tool completion timer"));
        }
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("winlex-error-code".into())
            .spawn(move || job(tx))?;
        self.pending.set(self.pending.get() + 1);
        Ok(())
    }
    fn refresh_module_languages(&self) {
        if !self.custom() {
            return;
        }
        let path = self.path();
        if path.as_os_str().is_empty() {
            return;
        }
        if let Err(error) = self.queue(move |tx| {
            let result = core::module_languages(&path);
            let _ = tx.send(Completion::Languages { path, result });
        }) {
            self.set_status(&format!("{error:#}"));
        }
    }
    fn execute(&self) {
        if self.querying.get() {
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
        match self.queue(move |tx| {
            let result = core::query(&request);
            let _ = tx.send(Completion::Query { revision, result });
        }) {
            Ok(()) => {
                self.querying.set(true);
                unsafe { EnableWindow(self.query, 0) };
                self.set_status(&resources::text(IDS_ERROR_CODE_QUERYING));
            }
            Err(error) => self.set_status(&format!("{error:#}")),
        }
    }
    fn poll(&self) {
        loop {
            let message = self.rx.borrow_mut().try_recv();
            let Ok(message) = message else {
                break;
            };
            self.pending.set(self.pending.get().saturating_sub(1));
            match message {
                Completion::Query { revision, result } => {
                    self.querying.set(false);
                    unsafe { EnableWindow(self.query, 1) };
                    let body = match result {
                        Ok(result) => {
                            super::super::document::update_title(
                                self.hwnd,
                                &format!(
                                    "{} — 0x{:08X}",
                                    resources::text(IDS_ERROR_CODE_TOOL),
                                    result.request.code
                                ),
                            );
                            render(&result)
                        }
                        Err(error) => {
                            format!("{}: {error:#}", resources::text(IDS_ERROR_CODE_FAILED))
                        }
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
                Completion::Languages { path, result } => {
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
            }
        }
        if self.pending.get() == 0 {
            unsafe { KillTimer(self.hwnd, TIMER) };
        }
    }
    fn browse(&self) {
        match crate::input::choose(
            self.hwnd,
            super::super::read(|a| a.system_version),
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
impl ToolContent for View {
    fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        for label in [
            IDS_ERROR_CODE_LABEL,
            IDS_ERROR_CODE_SCOPE,
            IDS_ERROR_CODE_LANGUAGE,
            IDS_ERROR_CODE_MODULE,
        ] {
            self.labels
                .push(unsafe { child(hwnd, w!("STATIC"), resources::wide_text(label), 0, 0) }?);
            let control = match label {
                IDS_ERROR_CODE_LABEL => unsafe {
                    child_ex(
                        hwnd,
                        w!("EDIT"),
                        &[0],
                        WS_TABSTOP | ES_AUTOHSCROLL as u32,
                        WS_EX_CLIENTEDGE,
                        ERROR_CODE_INPUT,
                    )
                }?,
                IDS_ERROR_CODE_SCOPE => unsafe {
                    child(
                        hwnd,
                        w!("COMBOBOX"),
                        &[0],
                        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                        ERROR_CODE_SCOPE,
                    )
                }?,
                IDS_ERROR_CODE_LANGUAGE => unsafe {
                    child(
                        hwnd,
                        w!("COMBOBOX"),
                        &[0],
                        WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32,
                        ERROR_CODE_LANGUAGE,
                    )
                }?,
                _ => unsafe {
                    child_ex(
                        hwnd,
                        w!("EDIT"),
                        &[0],
                        WS_TABSTOP | ES_AUTOHSCROLL as u32,
                        WS_EX_CLIENTEDGE,
                        ERROR_CODE_MODULE,
                    )
                }?,
            };
            match label {
                IDS_ERROR_CODE_LABEL => self.input = control,
                IDS_ERROR_CODE_SCOPE => self.scope = control,
                IDS_ERROR_CODE_LANGUAGE => self.language = control,
                _ => self.module = control,
            }
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
        self.browse = unsafe {
            child(
                hwnd,
                w!("BUTTON"),
                resources::wide_text(IDS_ERROR_CODE_BROWSE),
                WS_TABSTOP,
                ERROR_CODE_BROWSE,
            )
        }?;
        self.query = unsafe {
            child(
                hwnd,
                w!("BUTTON"),
                resources::wide_text(IDS_ERROR_CODE_QUERY),
                WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
                ERROR_CODE_QUERY,
            )
        }?;
        self.copy = unsafe {
            child(
                hwnd,
                w!("BUTTON"),
                resources::wide_text(IDS_ERROR_CODE_COPY_ALL),
                WS_TABSTOP,
                ERROR_CODE_COPY_ALL,
            )
        }?;
        let tool = TTTOOLINFOW {
            cbSize: std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
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
        for control in [self.input, self.module, self.result] {
            if unsafe { set_subclass(control, Some(text_proc), 1810, 0) } == 0 {
                return Err(error("Subclass tool text control"));
            }
        }
        self.tooltip = tooltip(hwnd, &tool)?;
        self.setting.set(false);
        Ok(())
    }
    fn layout(&self) {
        if self.result.is_null() {
            return;
        }
        let m = super::super::metrics(self.hwnd);
        let mut area = layout::inset(client(self.hwnd), m.px(8));
        let mut positions = vec![];
        let row = m.row().max(m.px(24));
        let label_width = m
            .label_width(
                [
                    IDS_ERROR_CODE_LABEL,
                    IDS_ERROR_CODE_SCOPE,
                    IDS_ERROR_CODE_LANGUAGE,
                    IDS_ERROR_CODE_MODULE,
                ]
                .into_iter()
                .map(resources::wide_text),
                64,
            )
            .min((area.right - area.left) / 3);
        let button_width = m.px(124);
        let paired = area.right - area.left >= label_width * 2 + m.px(360);
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
            let mut line = layout::take_top(&mut area, row);
            positions.push((
                self.labels[index],
                layout::take_left(&mut line, label_width),
            ));
            if index == 3 {
                let right = line.right;
                line.right = (right - button_width - m.px(4)).max(line.left);
                positions.push((
                    self.browse,
                    layout::rect(right - button_width, line.top, button_width, row),
                ));
            }
            if index == 1 || index == 2 {
                line.bottom = line.top + row * 9;
            }
            if index == 1 && paired {
                let mut language = line;
                let width = (line.right - line.left - label_width - m.px(8)) / 2;
                line.right = line.left + width;
                language.left = line.right + m.px(8);
                let mut label = layout::take_left(&mut language, label_width);
                label.bottom = label.top + row;
                positions.push((self.labels[2], label));
                positions.push((self.language, language));
            }
            positions.push((control, line));
            layout::take_top(&mut area, m.px(4));
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
    fn focus(&self) {
        focus(self.input)
    }
    fn prepare(&self) {
        let m = super::super::metrics(self.hwnd);
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
    fn recolor(&self) {
        let _ = crate::rich_text::recolor(self.result, &self.body.borrow());
    }
    fn status(&self) -> String {
        self.status.borrow().clone()
    }
    fn key(&self, msg: &MSG) -> bool {
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
    fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        match msg {
            WM_COMMAND if self.command(w as u16, (w >> 16) as u32) => Some(0),
            WM_TIMER if w == TIMER => {
                self.poll();
                Some(0)
            }
            WM_CONTEXTMENU if w as HWND == self.result => {
                let _ = crate::rich_text::context_menu(self.result, l, &[], None);
                Some(0)
            }
            _ => None,
        }
    }
    fn destroy(&self) {
        unsafe {
            KillTimer(self.hwnd, TIMER);
            DestroyWindow(self.tooltip);
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
            WM_RBUTTONDOWN if GetDlgCtrlID(hwnd) == ERROR_CODE_RESULT as i32 => return 0,
            WM_RBUTTONUP if GetDlgCtrlID(hwnd) == ERROR_CODE_RESULT as i32 => {
                let view = crate::rich_text::ViewState::capture(hwnd);
                focus(hwnd);
                view.restore(hwnd);
            }
            WM_GETDLGCODE if w == VK_TAB as usize => {
                return (DLGC_WANTARROWS | DLGC_WANTCHARS) as isize;
            }
            WM_IME_STARTCOMPOSITION => {
                SetPropW(hwnd, w!("Winlex.Tool.Composing"), 1usize as HANDLE);
            }
            WM_IME_ENDCOMPOSITION => {
                RemovePropW(hwnd, w!("Winlex.Tool.Composing"));
            }
            WM_CONTEXTMENU if GetDlgCtrlID(hwnd) == ERROR_CODE_RESULT as i32 => {
                return send(GetParent(hwnd), msg, hwnd as usize, l);
            }
            WM_NCDESTROY => {
                RemovePropW(hwnd, w!("Winlex.Tool.Composing"));
                remove_subclass(hwnd, Some(text_proc), 1810);
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
fn scope_label(scope: &Scope) -> String {
    resources::text(match scope {
        Scope::Auto => IDS_ERROR_CODE_AUTO,
        Scope::Win32 => IDS_ERROR_CODE_WIN32,
        Scope::HResult => IDS_ERROR_CODE_HRESULT,
        Scope::NtStatus => IDS_ERROR_CODE_NTSTATUS,
        Scope::Module(_) => IDS_ERROR_CODE_CUSTOM,
    })
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
            scope_label(&result.request.scope)
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
    fn scopes_have_distinct_resource_and_model_identities() {
        for (i, (label, scope)) in SCOPES.iter().enumerate() {
            for (other_label, other_scope) in &SCOPES[i + 1..] {
                assert_ne!(label, other_label);
                assert_ne!(scope, other_scope);
            }
        }
    }
}

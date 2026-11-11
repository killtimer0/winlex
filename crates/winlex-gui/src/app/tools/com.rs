//! COM/WinRT registration tool: one target, registry views and grouped results.
use crate::app::context::Context;
use crate::{
    layout,
    native::*,
    resources::{self, *},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use winlex_core::tools::com::{
    self, Field, Group, GroupKind, Report, Request, ResolvedKind, TargetKind, View as RegistryView,
    Views, Warning,
    activation::Instance,
    typelib::{self, Convention, Direction, MemberKind, TypeInfo, TypeKind, TypeLibrary},
};
use winlex_format::declaration::{Region, Rendered, SymbolRole};

use crate::tasks::tools::{ActivationError, Completion as ToolCompletion, Job as ToolJob};

const KINDS: &[(u16, TargetKind)] = &[
    (IDS_COM_KIND_AUTO, TargetKind::Auto),
    (IDS_COM_KIND_PROGID, TargetKind::ProgId),
    (IDS_COM_KIND_GUID, TargetKind::Guid),
];
const VIEWS: &[(u16, Views)] = &[
    (IDS_COM_VIEW_BOTH, Views::Both),
    (IDS_COM_VIEW_X86, Views::X86),
    (IDS_COM_VIEW_X64, Views::X64),
    (IDS_COM_VIEW_AUTO, Views::Auto),
];

pub(in crate::app) struct View {
    // The document lives inside the context, so the pointer cannot outlive it.
    ctx: *const Context,
    pub id: u64,
    pub hwnd: HWND,
    pub placement: Cell<Option<layout::Placement>>,
    pub live: Cell<bool>,
    pub(super) recent: Cell<u64>,
    icons: RefCell<Option<Rc<crate::icons::WindowIcons>>>,
    input: HWND,
    kind: HWND,
    registry_view: HWND,
    query: HWND,
    type_lib: HWND,
    activate: HWND,
    copy: HWND,
    result: HWND,
    labels: Vec<HWND>,
    status: RefCell<String>,
    setting: Cell<bool>,
    body: RefCell<crate::rich_text::Content>,
    plain: RefCell<String>,
    report: RefCell<Option<Report>>,
    type_libraries: RefCell<Vec<TypeLibrary>>,
    type_lib_errors: RefCell<Vec<String>>,
    type_lib_ticket: Cell<Option<u64>>,
    activated: RefCell<Option<Result<Instance, String>>>,
    activate_ticket: Cell<Option<u64>>,
}

impl View {
    fn ctx(&self) -> &Context {
        unsafe { &*self.ctx }
    }
    pub fn new(ctx: &Context) -> Self {
        Self {
            ctx: ctx as *const Context,
            id: super::super::workspace::next_serial(ctx),
            hwnd: std::ptr::null_mut(),
            placement: Cell::new(None),
            live: Cell::new(true),
            recent: Cell::new(0),
            icons: RefCell::new(None),
            input: std::ptr::null_mut(),
            kind: std::ptr::null_mut(),
            registry_view: std::ptr::null_mut(),
            query: std::ptr::null_mut(),
            type_lib: std::ptr::null_mut(),
            activate: std::ptr::null_mut(),
            copy: std::ptr::null_mut(),
            result: std::ptr::null_mut(),
            labels: vec![],
            status: RefCell::new(resources::text(IDS_COM_READY)),
            setting: Cell::new(true),
            body: RefCell::new(crate::rich_text::Content::new(String::new())),
            plain: RefCell::new(String::new()),
            report: RefCell::new(None),
            type_libraries: RefCell::new(Vec::new()),
            type_lib_errors: RefCell::new(Vec::new()),
            type_lib_ticket: Cell::new(None),
            activated: RefCell::new(None),
            activate_ticket: Cell::new(None),
        }
    }
    fn update_icons(&self) {
        let ctx = self.ctx();
        super::super::status::report(
            ctx,
            super::super::document::update_icons(
                ctx,
                self.hwnd,
                crate::icons::Icon::Interface,
                &self.icons,
            ),
        );
    }
    pub fn activated(&self) {
        self.recent
            .set(super::super::workspace::next_serial(self.ctx()));
    }
    pub fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        for (label, id, control, class, style, edge) in [
            (
                IDS_COM_LABEL,
                COM_INPUT,
                &mut self.input,
                w!("EDIT"),
                ES_AUTOHSCROLL as u32,
                WS_EX_CLIENTEDGE,
            ),
            (
                IDS_COM_KIND,
                COM_KIND,
                &mut self.kind,
                w!("COMBOBOX"),
                WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
            ),
            (
                IDS_COM_VIEW,
                COM_VIEW,
                &mut self.registry_view,
                w!("COMBOBOX"),
                WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
            ),
        ] {
            self.labels
                .push(unsafe { child(hwnd, w!("STATIC"), resources::wide_text(label), 0, 0) }?);
            *control = unsafe { child_ex(hwnd, class, &[0], WS_TABSTOP | style, edge, id) }?;
        }
        for &(label, _) in KINDS {
            send(
                self.kind,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(self.kind, CB_SETCURSEL, 0, 0);
        for &(label, _) in VIEWS {
            send(
                self.registry_view,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(self.registry_view, CB_SETCURSEL, 0, 0);
        for (control, id, label, style) in [
            (
                &mut self.query,
                COM_QUERY,
                IDS_COM_QUERY,
                BS_DEFPUSHBUTTON as u32,
            ),
            (&mut self.type_lib, COM_TYPELIB, IDS_COM_TYPELIB, 0),
            (&mut self.activate, COM_ACTIVATE, IDS_COM_ACTIVATE, 0),
            (&mut self.copy, COM_COPY_ALL, IDS_COM_COPY_ALL, 0),
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
        self.labels.push(unsafe {
            child(
                hwnd,
                w!("STATIC"),
                resources::wide_text(IDS_COM_RESULT_LABEL),
                0,
                0,
            )
        }?);
        self.result = crate::rich_text::create(hwnd, COM_RESULT)?;
        unsafe {
            EnableWindow(self.type_lib, 0);
            EnableWindow(self.activate, 0);
        }
        send(self.input, EM_SETLIMITTEXT, 4096, 0);
        self.setting.set(false);
        Ok(())
    }
    fn execute(&self) {
        let raw = native_text(self.input);
        let target = String::from_utf16_lossy(&raw);
        let target = target.trim();
        if let Some(id) = self.type_lib_ticket.take() {
            self.ctx().cancel_job(id);
        }
        if let Some(id) = self.activate_ticket.take() {
            self.ctx().cancel_job(id);
        }
        self.type_libraries.borrow_mut().clear();
        self.type_lib_errors.borrow_mut().clear();
        self.activated.replace(None);
        if target.is_empty() {
            self.report.replace(None);
            let empty = resources::text(IDS_COM_EMPTY);
            self.show(
                crate::rich_text::Content::new(empty.clone()),
                String::new(),
                &empty,
            );
            self.update_actions();
            return;
        }
        let kind = selected(self.kind, KINDS, TargetKind::Auto);
        let views = selected(self.registry_view, VIEWS, Views::Both);
        let report = com::query(
            &com::registry::Live,
            &Request {
                text: target.to_owned(),
                kind,
                views,
            },
        );
        self.report.replace(Some(report));
        self.present();
    }
    fn present(&self) {
        let Some(report) = self.report.borrow().clone() else {
            return;
        };
        let labels = Labels::from_resources();
        let mut warnings: Vec<String> = report
            .warnings
            .iter()
            .map(|warning| warning_text(warning, &labels))
            .collect();
        warnings.extend(self.type_lib_errors.borrow().iter().cloned());
        let mut output = render(&report, &labels, &warnings);
        for library in self.type_libraries.borrow().iter() {
            render_type_library(&mut output.body, library, &labels);
        }
        if let Some(activated) = self.activated.borrow().as_ref() {
            render_activation(&mut output.body, activated, &labels);
        }
        output.copy = output.body.text.trim_end().to_owned();
        let status = resources::format(
            IDS_COM_STATUS,
            &[
                &report.groups.len().to_string(),
                &self.type_libraries.borrow().len().to_string(),
                &warnings.len().to_string(),
            ],
        );
        self.show(
            crate::rich_text::Content::from(output.body),
            output.copy,
            &status,
        );
        self.update_actions();
    }
    fn update_actions(&self) {
        let report = self.report.borrow();
        let has_library = report
            .as_ref()
            .is_some_and(|report| !type_library_references(report).is_empty());
        let has_class = report.as_ref().and_then(first_class).is_some();
        drop(report);
        let libraries = has_library && self.type_lib_ticket.get().is_none();
        let activate = has_class && self.activate_ticket.get().is_none();
        unsafe {
            EnableWindow(self.type_lib, if libraries { 1 } else { 0 });
            EnableWindow(self.activate, if activate { 1 } else { 0 });
        }
    }
    fn read_type_libraries(&self) {
        let report = self.report.borrow();
        let Some(report) = report.as_ref() else {
            return;
        };
        let libraries = type_library_references(report);
        if libraries.is_empty() {
            self.set_status(&resources::text(IDS_COM_TYPELIB_NONE));
            return;
        }
        let ctx = self.ctx();
        let ticket = crate::app::catalog::next_ticket(ctx);
        let id = ticket.id;
        let previous = self.type_lib_ticket.take();
        match ctx.replace_job(
            previous,
            ticket,
            crate::tasks::Job::Tool {
                document: self.id,
                job: ToolJob::TypeLibraries { libraries },
            },
        ) {
            Ok(()) => {
                self.type_lib_ticket.set(Some(id));
                self.set_status(&resources::text(IDS_COM_TYPELIB_LOADING));
                self.update_actions();
            }
            Err(error) => self.set_status(&format!("{error:#}")),
        }
    }
    fn activate(&self) {
        let report = self.report.borrow();
        let Some(guid) = report.as_ref().and_then(first_class) else {
            drop(report);
            self.set_status(&resources::text(IDS_COM_ACTIVATE_EMPTY));
            return;
        };
        drop(report);
        let question = resources::format(IDS_COM_ACTIVATE_CONFIRM, &[&guid.to_string()]);
        match crate::app::status::report(
            self.ctx(),
            crate::message::confirm(self.hwnd, &resources::text(IDS_APP_TITLE), &question),
        ) {
            Some(true) => {}
            Some(false) => return,
            None => return,
        }
        let ctx = self.ctx();
        let ticket = crate::app::catalog::next_ticket(ctx);
        let id = ticket.id;
        let previous = self.activate_ticket.take();
        match ctx.replace_job(
            previous,
            ticket,
            crate::tasks::Job::Tool {
                document: self.id,
                job: ToolJob::Activate { guid },
            },
        ) {
            Ok(()) => {
                self.activate_ticket.set(Some(id));
                self.activated.replace(None);
                self.set_status(&resources::text(IDS_COM_ACTIVATE_RUNNING));
                self.present();
            }
            Err(error) => self.set_status(&format!("{error:#}")),
        }
    }
    pub(in crate::app) fn complete(&self, id: u64, completion: ToolCompletion) {
        match completion {
            ToolCompletion::TypeLibraries { results } => self.type_libraries_complete(id, results),
            ToolCompletion::Activate { result } => self.activate_complete(id, result),
            _ => {}
        }
    }
    fn type_libraries_complete(
        &self,
        id: u64,
        results: Vec<crate::tasks::tools::TypeLibraryResult>,
    ) {
        if self.type_lib_ticket.get() != Some(id) {
            return;
        }
        self.type_lib_ticket.set(None);
        let mut libraries = self.type_libraries.borrow_mut();
        let mut errors = self.type_lib_errors.borrow_mut();
        libraries.clear();
        errors.clear();
        for outcome in results {
            match outcome.result {
                Ok(library) => libraries.push(library),
                Err(error) => {
                    let reason = match error {
                        typelib::Error::NotRegistered => {
                            resources::text(IDS_COM_TYPELIB_NOT_REGISTERED)
                        }
                        typelib::Error::Failure(message) => message,
                    };
                    errors.push(resources::format(
                        IDS_COM_TYPELIB_FAILED,
                        &[&outcome.guid.to_string(), &reason],
                    ));
                }
            }
        }
        drop(errors);
        drop(libraries);
        self.present();
    }
    fn activate_complete(&self, id: u64, result: Result<Instance, ActivationError>) {
        if self.activate_ticket.get() != Some(id) {
            return;
        }
        self.activate_ticket.set(None);
        let result = result.map_err(|error| match error {
            ActivationError::TimedOut(seconds) => {
                resources::format(IDS_COM_ACTIVATE_TIMEOUT, &[&seconds.to_string()])
            }
            ActivationError::Failed(reason) => {
                resources::format(IDS_COM_ACTIVATE_FAILED, &[&reason])
            }
        });
        self.activated.replace(Some(result));
        self.present();
    }
    pub(in crate::app) fn worker_stopped(&self) {
        let libraries = self.type_lib_ticket.take().is_some();
        let activate = self.activate_ticket.take().is_some();
        if libraries || activate {
            self.set_status(&resources::text(IDS_WORKER_STOPPED));
            self.update_actions();
        }
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
    fn set_status(&self, message: &str) {
        self.status.replace(message.to_owned());
        super::super::document::status_changed(self.ctx(), self.hwnd);
    }
    pub(in crate::app) fn search(&self, target: &str) {
        let value: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
        unsafe { SetWindowTextW(self.input, value.as_ptr()) };
        send(self.kind, CB_SETCURSEL, 0, 0);
        send(self.registry_view, CB_SETCURSEL, 0, 0);
        self.execute();
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
        let label_width = [IDS_COM_LABEL, IDS_COM_KIND, IDS_COM_VIEW]
            .into_iter()
            .map(|id| m.text_width(resources::wide_text(id)))
            .max()
            .unwrap_or(0)
            .min((area.right - area.left) / 3);
        let dpi = self.ctx().dpi();
        let heights = [
            m.edit_height(self.input, dpi),
            bounds(self.kind, self.hwnd).bottom - bounds(self.kind, self.hwnd).top,
            bounds(self.registry_view, self.hwnd).bottom
                - bounds(self.registry_view, self.hwnd).top,
        ];
        for (index, (label, control)) in [
            (self.labels[0], self.input),
            (self.labels[1], self.kind),
            (self.labels[2], self.registry_view),
        ]
        .into_iter()
        .enumerate()
        {
            let mut line = layout::take_top(&mut area, heights[index].max(m.height));
            let label_area = layout::take_left(&mut line, label_width);
            positions.push((label, layout::center_vertical(label_area, m.height)));
            layout::take_left(&mut line, label_gap);
            positions.push((control, layout::center_vertical(line, heights[index])));
            layout::take_top(&mut area, gap);
        }
        m.buttons(
            &mut area,
            [self.query, self.type_lib, self.activate, self.copy],
            &mut positions,
        );
        layout::take_top(&mut area, m.px(4));
        positions.push((
            self.labels[3],
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
            self.kind,
            self.registry_view,
            self.query,
            self.type_lib,
            self.activate,
            self.copy,
        ]) {
            super::super::bind(h, m.font);
        }
        let ctx = self.ctx();
        for combo in [self.kind, self.registry_view] {
            let mut rect = bounds(combo, self.hwnd);
            rect.bottom = rect.top + m.row() * 9;
            ctx.checked(set_bounds(combo, rect));
        }
        let (system_font, _) = ctx.font_for(ctx.dpi().system());
        ctx.checked(crate::rich_text::prepare(
            self.result,
            system_font,
            ctx.dpi(),
        ));
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
            && [self.input, self.result].contains(&msg.hwnd)
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
            WM_COMMAND => {
                let id = w as u16;
                let code = (w >> 16) as u32;
                match id {
                    COM_QUERY if code == BN_CLICKED => self.execute(),
                    COM_TYPELIB if code == BN_CLICKED => self.read_type_libraries(),
                    COM_ACTIVATE if code == BN_CLICKED => self.activate(),
                    COM_COPY_ALL if code == BN_CLICKED => {
                        let copy_text: Vec<u16> = self.plain.borrow().encode_utf16().collect();
                        if let Err(error) = copy_unicode(self.hwnd, &copy_text) {
                            self.set_status(&format!("{error:#}"));
                        }
                    }
                    COM_KIND | COM_VIEW if code == CBN_SELENDOK => {
                        if !self.setting.get() && !native_text(self.input).is_empty() {
                            self.execute();
                        }
                    }
                    _ => return None,
                }
                Some(0)
            }
            WM_CONTEXTMENU if w as HWND == self.result => {
                let _ = crate::rich_text::context_menu(self.result, l, &[]);
                Some(0)
            }
            _ => None,
        }
    }
    pub fn destroy(&self) {
        if let Some(id) = self.type_lib_ticket.take() {
            self.ctx().cancel_job(id);
        }
        if let Some(id) = self.activate_ticket.take() {
            self.ctx().cancel_job(id);
        }
    }
}

fn native_text(hwnd: HWND) -> Vec<u16> {
    let mut value = text_wide(hwnd);
    value.pop();
    value
}

/// GUID search results can be looked up in the local registrations.
pub(super) fn action(
    value: Option<&crate::app::result_value::ResultValue>,
) -> Option<super::Action> {
    let target = &value?.text;
    com::Guid::parse(target)?;
    Some(super::Action {
        label: IDS_COM_SEARCH,
        request: super::Request::Com {
            text: target.clone(),
        },
    })
}

fn selected<T: Copy>(combo: HWND, values: &[(u16, T)], fallback: T) -> T {
    let selected = send(combo, CB_GETCURSEL, 0, 0);
    values
        .get(selected.max(0) as usize)
        .map(|&(_, value)| value)
        .unwrap_or(fallback)
}

struct Labels {
    guid: String,
    prog_id: String,
    group: String,
    interface: String,
    type_lib: String,
    type_lib_version: String,
    type_lib_locale: String,
    app_id: String,
    view_x86: String,
    view_x64: String,
    view_native: String,
    origin_machine: String,
    origin_user: String,
    empty_value: String,
    warnings: String,
    warn_empty: String,
    type_library: String,
    type_enum: String,
    type_record: String,
    type_module: String,
    type_interface: String,
    type_dispatch: String,
    type_coclass: String,
    type_alias: String,
    type_union: String,
    type_unknown: String,
    member_function: String,
    member_get: String,
    member_put: String,
    member_put_ref: String,
    member_unknown: String,
    direction_in: String,
    direction_out: String,
    direction_inout: String,
    implements: String,
    optional: String,
    retval: String,
    instance: String,
    dispatch: String,
    dispatch_none: String,
    inspectable: String,
    runtime_class: String,
    interfaces: String,
    activate_none: String,
}

impl Labels {
    fn from_resources() -> Self {
        Self {
            guid: resources::text(IDS_COM_KIND_GUID),
            prog_id: resources::text(IDS_COM_KIND_PROGID),
            group: resources::text(IDS_COM_GROUP_CLASS),
            interface: resources::text(IDS_COM_GROUP_INTERFACE),
            type_lib: resources::text(IDS_COM_GROUP_TYPELIB),
            type_lib_version: resources::text(IDS_COM_GROUP_TYPELIB_VERSION),
            type_lib_locale: resources::text(IDS_COM_GROUP_TYPELIB_LOCALE),
            app_id: resources::text(IDS_COM_GROUP_APPID),
            view_x86: resources::text(IDS_COM_VIEW_X86),
            view_x64: resources::text(IDS_COM_VIEW_X64),
            view_native: resources::text(IDS_COM_VIEW_NATIVE),
            origin_machine: resources::text(IDS_COM_ORIGIN_MACHINE),
            origin_user: resources::text(IDS_COM_ORIGIN_USER),
            empty_value: resources::text(IDS_COM_EMPTY_VALUE),
            warnings: resources::text(IDS_COM_WARNINGS),
            warn_empty: resources::text(IDS_COM_WARN_EMPTY),
            type_library: resources::text(IDS_COM_TYPELIB_LABEL),
            type_enum: resources::text(IDS_COM_TYPE_ENUM),
            type_record: resources::text(IDS_COM_TYPE_RECORD),
            type_module: resources::text(IDS_COM_TYPE_MODULE),
            type_interface: resources::text(IDS_COM_TYPE_INTERFACE),
            type_dispatch: resources::text(IDS_COM_TYPE_DISPATCH),
            type_coclass: resources::text(IDS_COM_TYPE_COCLASS),
            type_alias: resources::text(IDS_COM_TYPE_ALIAS),
            type_union: resources::text(IDS_COM_TYPE_UNION),
            type_unknown: resources::text(IDS_COM_TYPE_UNKNOWN),
            member_function: resources::text(IDS_COM_MEMBER_FUNCTION),
            member_get: resources::text(IDS_COM_MEMBER_GET),
            member_put: resources::text(IDS_COM_MEMBER_PUT),
            member_put_ref: resources::text(IDS_COM_MEMBER_PUTREF),
            member_unknown: resources::text(IDS_COM_MEMBER_UNKNOWN),
            direction_in: resources::text(IDS_COM_DIRECTION_IN),
            direction_out: resources::text(IDS_COM_DIRECTION_OUT),
            direction_inout: resources::text(IDS_COM_DIRECTION_INOUT),
            implements: resources::text(IDS_COM_IMPLEMENTS),
            optional: resources::text(IDS_COM_OPTIONAL),
            retval: resources::text(IDS_COM_RETVAL),
            instance: resources::text(IDS_COM_INSTANCE),
            dispatch: resources::text(IDS_COM_DISPATCH),
            dispatch_none: resources::text(IDS_COM_DISPATCH_NONE),
            inspectable: resources::text(IDS_COM_INSPECTABLE),
            runtime_class: resources::text(IDS_COM_RUNTIME_CLASS),
            interfaces: resources::text(IDS_COM_INTERFACES),
            activate_none: resources::text(IDS_COM_ACTIVATE_NONE),
        }
    }
    fn group(&self, kind: GroupKind) -> &str {
        match kind {
            GroupKind::ProgId => &self.prog_id,
            GroupKind::Class => &self.group,
            GroupKind::Interface => &self.interface,
            GroupKind::TypeLib => &self.type_lib,
            GroupKind::TypeLibVersion => &self.type_lib_version,
            GroupKind::TypeLibLocale => &self.type_lib_locale,
            GroupKind::AppId => &self.app_id,
        }
    }
    fn view(&self, view: RegistryView) -> &str {
        match view {
            RegistryView::X86 => &self.view_x86,
            RegistryView::X64 => &self.view_x64,
            RegistryView::Native => &self.view_native,
        }
    }
    fn origin(&self, origin: com::Origin) -> &str {
        match origin {
            com::Origin::Machine => &self.origin_machine,
            com::Origin::User => &self.origin_user,
        }
    }
    fn type_kind(&self, kind: TypeKind) -> &str {
        match kind {
            TypeKind::Enum => &self.type_enum,
            TypeKind::Record => &self.type_record,
            TypeKind::Module => &self.type_module,
            TypeKind::Interface => &self.type_interface,
            TypeKind::Dispatch => &self.type_dispatch,
            TypeKind::Coclass => &self.type_coclass,
            TypeKind::Alias => &self.type_alias,
            TypeKind::Union => &self.type_union,
            TypeKind::Unknown => &self.type_unknown,
        }
    }
    fn member(&self, kind: MemberKind) -> &str {
        match kind {
            MemberKind::Function => &self.member_function,
            MemberKind::PropertyGet => &self.member_get,
            MemberKind::PropertyPut => &self.member_put,
            MemberKind::PropertyPutRef => &self.member_put_ref,
            MemberKind::Unknown => &self.member_unknown,
        }
    }
    fn direction(&self, direction: Direction) -> &str {
        match direction {
            Direction::In => &self.direction_in,
            Direction::Out => &self.direction_out,
            Direction::InOut => &self.direction_inout,
            Direction::Unknown => "",
        }
    }
}

fn warning_text(warning: &Warning, labels: &Labels) -> String {
    match warning {
        Warning::EmptyInput => labels.warn_empty.clone(),
        Warning::InvalidGuid(value) => resources::format(IDS_COM_WARN_INVALID_GUID, &[value]),
        Warning::InvalidProgId(value) => resources::format(IDS_COM_WARN_INVALID_PROGID, &[value]),
        Warning::Unrecognized(value) => resources::format(IDS_COM_WARN_UNRECOGNIZED, &[value]),
        Warning::NotFound(value) => resources::format(IDS_COM_WARN_NOT_FOUND, &[value]),
        Warning::UnsupportedView(view) => {
            resources::format(IDS_COM_WARN_UNSUPPORTED_VIEW, &[labels.view(*view)])
        }
        Warning::Registry(error) => resources::format(IDS_COM_WARN_REGISTRY, &[error]),
    }
}

struct Output {
    body: Rendered,
    copy: String,
}

fn render(report: &Report, labels: &Labels, warnings: &[String]) -> Output {
    let mut body = Rendered::default();
    body.text.push_str(&report.input);
    body.text.push('\n');
    if let Some(kind) = report.kind {
        let label = match kind {
            ResolvedKind::Guid => &labels.guid,
            ResolvedKind::ProgId => &labels.prog_id,
        };
        body.text.push_str(label);
        body.text.push(' ');
        body.text
            .push_str(report.normalized.as_deref().unwrap_or_default());
        body.text.push('\n');
    }
    if !report.groups.is_empty() {
        body.text.push('\n');
        for (index, group) in report.groups.iter().enumerate() {
            if index != 0 {
                body.text.push('\n');
            }
            render_group(&mut body, group, 0, labels);
        }
    }
    if !warnings.is_empty() {
        body.text.push('\n');
        body.text.push_str(&labels.warnings);
        for warning in warnings {
            body.text.push('\n');
            body.text.push_str("  ");
            body.text.push_str(warning);
        }
        body.text.push('\n');
    }
    Output {
        copy: body.text.trim_end().to_owned(),
        body,
    }
}

fn render_type_library(body: &mut Rendered, library: &TypeLibrary, labels: &Labels) {
    body.text.push('\n');
    body.text.push_str(&labels.type_library);
    body.text.push(' ');
    body.text
        .push_str(&format!("{}.{}", library.major, library.minor));
    body.text.push(' ');
    push_symbol(body, &library.guid, SymbolRole::Type);
    if !library.name.is_empty() {
        body.text.push(' ');
        body.text.push_str(&library.name);
    }
    body.text.push('\n');
    for info in &library.types {
        render_type_info(body, info, 1, labels);
    }
}

fn render_type_info(body: &mut Rendered, info: &TypeInfo, depth: usize, labels: &Labels) {
    let indent = "  ".repeat(depth);
    let member = "  ".repeat(depth + 1);
    let detail = "  ".repeat(depth + 2);
    body.text.push_str(&indent);
    body.text.push_str(labels.type_kind(info.kind));
    body.text.push(' ');
    push_symbol(body, &info.name, SymbolRole::Type);
    if let Some(guid) = &info.guid {
        body.text.push(' ');
        body.text.push_str(guid);
    }
    body.text.push('\n');
    for implemented in &info.implemented {
        body.text.push_str(&member);
        body.text.push_str(&labels.implements);
        body.text.push(' ');
        push_symbol(body, &implemented.name, SymbolRole::Type);
        if let Some(guid) = &implemented.guid {
            body.text.push(' ');
            body.text.push_str(guid);
        }
        let mut flags = Vec::new();
        if implemented.source {
            flags.push("source");
        }
        if implemented.default {
            flags.push("default");
        }
        if implemented.restricted {
            flags.push("restricted");
        }
        if implemented.default_vtable {
            flags.push("defaultvtable");
        }
        if !flags.is_empty() {
            body.text.push_str(" [");
            body.text.push_str(&flags.join(", "));
            body.text.push(']');
        }
        body.text.push('\n');
    }
    for function in &info.functions {
        body.text.push_str(&member);
        push_symbol(body, &function.name, SymbolRole::Method);
        body.text.push_str(" (");
        body.text.push_str(labels.member(function.member));
        if function.convention != Convention::Unknown {
            body.text.push_str(", ");
            body.text.push_str(convention_name(function.convention));
        }
        body.text.push_str(") -> ");
        body.text.push_str(&function.returns);
        body.text.push('\n');
        for parameter in &function.parameters {
            body.text.push_str(&detail);
            body.text.push('[');
            body.text.push_str(labels.direction(parameter.direction));
            if parameter.optional {
                body.text.push_str(", ");
                body.text.push_str(&labels.optional);
            }
            if parameter.return_value {
                body.text.push_str(", ");
                body.text.push_str(&labels.retval);
            }
            body.text.push_str("] ");
            body.text.push_str(&parameter.ty);
            if !parameter.name.is_empty() {
                body.text.push(' ');
                body.text.push_str(&parameter.name);
            }
            body.text.push('\n');
        }
    }
    for variable in &info.variables {
        body.text.push_str(&member);
        push_symbol(body, &variable.name, SymbolRole::Member);
        body.text.push_str(" : ");
        body.text.push_str(&variable.ty);
        body.text.push('\n');
    }
}

fn render_activation(body: &mut Rendered, result: &Result<Instance, String>, labels: &Labels) {
    body.text.push('\n');
    let instance = match result {
        Ok(instance) => instance,
        Err(message) => {
            body.text.push_str(message);
            body.text.push('\n');
            return;
        }
    };
    body.text.push_str(&labels.instance);
    body.text.push(' ');
    push_symbol(body, &instance.clsid, SymbolRole::Type);
    body.text.push('\n');
    let mut shown = false;
    if let Some(dispatch) = &instance.dispatch {
        shown = true;
        body.text.push_str("  ");
        body.text.push_str(&labels.dispatch);
        body.text.push('\n');
        match &dispatch.type_info {
            Some(info) => render_type_info(body, info, 2, labels),
            None => {
                body.text.push_str("    ");
                body.text.push_str(&labels.dispatch_none);
                body.text.push('\n');
            }
        }
    }
    if let Some(inspectable) = &instance.inspectable {
        shown = true;
        body.text.push_str("  ");
        body.text.push_str(&labels.inspectable);
        body.text.push('\n');
        if let Some(class) = &inspectable.runtime_class {
            body.text.push_str("    ");
            body.text.push_str(&labels.runtime_class);
            body.text.push(' ');
            push_symbol(body, class, SymbolRole::Type);
            body.text.push('\n');
        }
        if !inspectable.interfaces.is_empty() {
            body.text.push_str("    ");
            body.text.push_str(&labels.interfaces);
            body.text.push('\n');
            for iid in &inspectable.interfaces {
                body.text.push_str("      ");
                push_symbol(body, iid, SymbolRole::Type);
                body.text.push('\n');
            }
        }
    }
    if !shown {
        body.text.push_str("  ");
        body.text.push_str(&labels.activate_none);
        body.text.push('\n');
    }
}

fn first_class(report: &Report) -> Option<com::Guid> {
    report
        .groups
        .iter()
        .find(|group| group.kind == GroupKind::Class)
        .and_then(|group| group.identity.as_deref())
        .and_then(com::Guid::parse)
}

fn push_symbol(body: &mut Rendered, value: &str, role: SymbolRole) {
    if value.is_empty() {
        return;
    }
    let start = body.text.encode_utf16().count();
    body.text.push_str(value);
    let end = start + value.encode_utf16().count();
    body.regions.push(Region::symbol(start..end, role));
}

fn convention_name(convention: Convention) -> &'static str {
    match convention {
        Convention::Cdecl => "cdecl",
        Convention::Stdcall => "stdcall",
        Convention::Fastcall => "fastcall",
        Convention::Pascal => "pascal",
        Convention::Syscall => "syscall",
        Convention::MacPascal => "macpascal",
        Convention::FpFastcall => "fpfastcall",
        Convention::Unknown => "",
    }
}

fn parse_version(value: &str) -> Option<(u16, u16)> {
    let (major, minor) = value.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn type_library_references(report: &Report) -> Vec<(com::Guid, u16, u16)> {
    let mut libraries = Vec::new();
    for group in &report.groups {
        if group.kind != GroupKind::TypeLib {
            continue;
        }
        let Some(guid) = group.identity.as_deref().and_then(com::Guid::parse) else {
            continue;
        };
        for child in &group.children {
            if child.kind != GroupKind::TypeLibVersion {
                continue;
            }
            let Some((major, minor)) = child.identity.as_deref().and_then(parse_version) else {
                continue;
            };
            if !libraries.contains(&(guid, major, minor)) {
                libraries.push((guid, major, minor));
            }
        }
    }
    libraries
}

fn render_group(body: &mut Rendered, group: &Group, depth: usize, labels: &Labels) {
    let indent = "  ".repeat(depth);
    let name = labels.group(group.kind);
    let identity = group.identity.as_deref().unwrap_or_default();
    body.text.push_str(&indent);
    body.text.push_str(name);
    if !identity.is_empty() {
        body.text.push(' ');
        let start = body.text.encode_utf16().count();
        body.text.push_str(identity);
        let end = start + identity.encode_utf16().count();
        if matches!(
            group.kind,
            GroupKind::ProgId
                | GroupKind::Class
                | GroupKind::Interface
                | GroupKind::TypeLib
                | GroupKind::AppId
        ) {
            body.regions
                .push(Region::symbol(start..end, SymbolRole::Type));
        }
    }
    body.text.push('\n');
    for field in &group.fields {
        render_field(body, field, depth + 1, labels);
    }
    for child in &group.children {
        render_group(body, child, depth + 1, labels);
    }
}

fn render_field(body: &mut Rendered, field: &Field, depth: usize, labels: &Labels) {
    let indent = "  ".repeat(depth);
    let name = field.attribute.name();
    let pad = " ".repeat(name.len());
    let annotate = field.values.len() > 1;
    for (index, value) in field.values.iter().enumerate() {
        body.text.push_str(&indent);
        body.text.push_str(if index == 0 { name } else { &pad });
        body.text.push_str(": ");
        if value.value.is_empty() {
            body.text.push_str(&labels.empty_value);
        } else {
            body.text.push_str(&value.value);
        }
        if annotate {
            body.text.push_str(" [");
            for (index, source) in value.sources.iter().enumerate() {
                if index != 0 {
                    body.text.push_str(", ");
                }
                body.text.push_str(labels.view(source.view));
                body.text.push(' ');
                body.text.push_str(labels.origin(source.origin));
            }
            body.text.push(']');
        }
        body.text.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winlex_core::tools::com::Attribute;
    use winlex_core::tools::com::activation;

    fn labels() -> Labels {
        Labels {
            guid: "GUID".into(),
            prog_id: "ProgID".into(),
            group: "CLSID".into(),
            interface: "IID".into(),
            type_lib: "LIBID".into(),
            type_lib_version: "Version".into(),
            type_lib_locale: "Locale".into(),
            app_id: "AppID".into(),
            view_x86: "32-bit".into(),
            view_x64: "64-bit".into(),
            view_native: "native".into(),
            origin_machine: "machine".into(),
            origin_user: "user".into(),
            empty_value: "(empty)".into(),
            warnings: "Warnings:".into(),
            warn_empty: "Enter a target.".into(),
            type_library: "Type library".into(),
            type_enum: "Enumeration".into(),
            type_record: "Structure".into(),
            type_module: "Module".into(),
            type_interface: "Interface".into(),
            type_dispatch: "Dispatch interface".into(),
            type_coclass: "Class".into(),
            type_alias: "Alias".into(),
            type_union: "Union".into(),
            type_unknown: "Other".into(),
            member_function: "function".into(),
            member_get: "property get".into(),
            member_put: "property put".into(),
            member_put_ref: "property put by reference".into(),
            member_unknown: "member".into(),
            direction_in: "in".into(),
            direction_out: "out".into(),
            direction_inout: "in, out".into(),
            implements: "Implements".into(),
            optional: "optional".into(),
            retval: "retval".into(),
            instance: "Instance".into(),
            dispatch: "IDispatch".into(),
            dispatch_none: "No type information is exposed.".into(),
            inspectable: "IInspectable".into(),
            runtime_class: "Runtime class".into(),
            interfaces: "Interfaces".into(),
            activate_none: "The object exposes neither IDispatch nor IInspectable.".into(),
        }
    }

    fn provenance(view: RegistryView, origin: com::Origin) -> com::Provenance {
        com::Provenance { view, origin }
    }

    #[test]
    fn differing_view_values_are_annotated_and_identities_are_colored() {
        let report = Report {
            input: "{00000000-0000-0000-0000-000000000001}".into(),
            kind: Some(ResolvedKind::Guid),
            normalized: Some("{00000000-0000-0000-0000-000000000001}".into()),
            groups: vec![Group {
                kind: GroupKind::Class,
                identity: Some("{00000000-0000-0000-0000-000000000001}".into()),
                fields: vec![Field {
                    attribute: Attribute::InprocServer32,
                    values: vec![
                        com::Sourced {
                            value: "C:\\x86\\a.dll".into(),
                            sources: vec![provenance(RegistryView::X86, com::Origin::Machine)],
                        },
                        com::Sourced {
                            value: "C:\\x64\\a.dll".into(),
                            sources: vec![provenance(RegistryView::X64, com::Origin::Machine)],
                        },
                    ],
                }],
                children: vec![],
            }],
            warnings: vec![],
        };
        let output = render(&report, &labels(), &[]);
        assert!(
            output
                .body
                .text
                .contains("CLSID {00000000-0000-0000-0000-000000000001}")
        );
        assert!(output.body.text.contains("C:\\x86\\a.dll [32-bit machine]"));
        assert!(output.body.text.contains("C:\\x64\\a.dll [64-bit machine]"));
        assert!(output.copy.contains("C:\\x86\\a.dll [32-bit machine]"));
    }

    #[test]
    fn equal_values_appear_once_without_annotations() {
        let report = Report {
            input: "Example.Object".into(),
            kind: Some(ResolvedKind::ProgId),
            normalized: Some("Example.Object".into()),
            groups: vec![Group {
                kind: GroupKind::ProgId,
                identity: Some("Example.Object".into()),
                fields: vec![Field {
                    attribute: Attribute::Description,
                    values: vec![com::Sourced {
                        value: "Example".into(),
                        sources: vec![
                            provenance(RegistryView::X86, com::Origin::Machine),
                            provenance(RegistryView::X86, com::Origin::User),
                        ],
                    }],
                }],
                children: vec![],
            }],
            warnings: vec![],
        };
        let output = render(&report, &labels(), &[]);
        assert_eq!(output.body.text.matches("Description: Example").count(), 1);
        assert!(!output.body.text.contains("[32-bit"));
        assert!(
            output
                .copy
                .starts_with("Example.Object\nProgID Example.Object")
        );
    }

    #[test]
    fn warnings_follow_the_groups() {
        let report = Report {
            input: "not a target".into(),
            kind: None,
            normalized: None,
            groups: vec![],
            warnings: vec![Warning::Unrecognized("not a target".into())],
        };
        let output = render(&report, &labels(), &["cannot tell".into()]);
        assert_eq!(output.copy, "not a target\n\nWarnings:\n  cannot tell");
    }

    #[test]
    fn type_libraries_are_appended_with_members_and_parameters() {
        let library = TypeLibrary {
            guid: "{00020430-0000-0000-C000-000000000046}".into(),
            major: 2,
            minor: 0,
            lcid: 0,
            name: "OLE Automation".into(),
            documentation: None,
            types: vec![typelib::TypeInfo {
                name: "IFont".into(),
                kind: TypeKind::Interface,
                guid: Some("{BEF6E003-A874-101A-8BBA-00AA00300CAB}".into()),
                documentation: None,
                implemented: vec![typelib::Implemented {
                    name: "IUnknown".into(),
                    guid: Some("{00000000-0000-0000-C000-000000000046}".into()),
                    source: false,
                    default: true,
                    restricted: false,
                    default_vtable: false,
                }],
                functions: vec![typelib::Function {
                    name: "Name".into(),
                    memid: 0,
                    member: MemberKind::PropertyGet,
                    convention: Convention::Stdcall,
                    returns: "BSTR".into(),
                    parameters: vec![typelib::Parameter {
                        name: "pName".into(),
                        ty: "BSTR".into(),
                        direction: Direction::Out,
                        optional: false,
                        return_value: true,
                    }],
                }],
                variables: vec![typelib::Variable {
                    name: "hFont".into(),
                    memid: 1,
                    ty: "OLE_HANDLE".into(),
                    kind: typelib::VariableKind::PerInstance,
                }],
            }],
        };
        let mut body = Rendered::default();
        render_type_library(&mut body, &library, &labels());
        assert!(body.text.starts_with("\nType library 2.0 "));
        assert!(body.text.contains("\n  Interface IFont "));
        assert!(body.text.contains("\n    Implements IUnknown "));
        assert!(
            body.text
                .contains("\n    Name (property get, stdcall) -> BSTR")
        );
        assert!(body.text.contains("\n      [out, retval] BSTR pName"));
        assert!(body.text.contains("\n    hFont : OLE_HANDLE"));
        assert!(
            body.regions
                .iter()
                .any(|region| region.role == Some(SymbolRole::Method))
        );
        assert!(
            body.regions
                .iter()
                .any(|region| region.role == Some(SymbolRole::Member))
        );
    }

    #[test]
    fn type_library_references_follow_registered_versions() {
        let report = Report {
            input: "Example.Object".into(),
            kind: Some(ResolvedKind::ProgId),
            normalized: Some("Example.Object".into()),
            groups: vec![Group {
                kind: GroupKind::TypeLib,
                identity: Some("{00020430-0000-0000-C000-000000000046}".into()),
                fields: vec![],
                children: vec![
                    Group {
                        kind: GroupKind::TypeLibVersion,
                        identity: Some("2.0".into()),
                        fields: vec![],
                        children: vec![],
                    },
                    Group {
                        kind: GroupKind::TypeLibVersion,
                        identity: Some("1.0".into()),
                        fields: vec![],
                        children: vec![],
                    },
                ],
            }],
            warnings: vec![],
        };
        let references = type_library_references(&report);
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].1, 2);
        assert_eq!(references[1].1, 1);
    }

    #[test]
    fn activation_is_appended_with_dispatch_and_inspectable() {
        let instance = Instance {
            clsid: "{00000000-0000-0000-0000-000000000001}".into(),
            dispatch: Some(activation::DispatchInfo {
                type_info: Some(typelib::TypeInfo {
                    name: "IFoo".into(),
                    kind: TypeKind::Dispatch,
                    ..typelib::TypeInfo::default()
                }),
            }),
            inspectable: Some(activation::InspectableInfo {
                runtime_class: Some("Windows.Foo".into()),
                interfaces: vec!["{AAAAAAAA-0000-0000-0000-000000000000}".into()],
            }),
        };
        let mut body = Rendered::default();
        render_activation(&mut body, &Ok(instance), &labels());
        assert!(body.text.starts_with("\nInstance {"));
        assert!(body.text.contains("\n  IDispatch\n"));
        assert!(body.text.contains("\n    Dispatch interface IFoo"));
        assert!(body.text.contains("\n  IInspectable\n"));
        assert!(body.text.contains("\n    Runtime class Windows.Foo"));
        assert!(body.text.contains("\n    Interfaces\n"));
        assert!(
            body.text
                .contains("\n      {AAAAAAAA-0000-0000-0000-000000000000}")
        );
    }

    #[test]
    fn activation_failure_is_reported_inline() {
        let mut body = Rendered::default();
        render_activation(&mut body, &Err("Activation failed: boom".into()), &labels());
        assert_eq!(body.text, "\nActivation failed: boom\n");
    }

    #[test]
    fn only_class_groups_can_be_activated() {
        let mut report = Report {
            input: "Example.Object".into(),
            kind: None,
            normalized: None,
            groups: vec![Group {
                kind: GroupKind::Interface,
                identity: Some("{00000000-0000-0000-0000-000000000001}".into()),
                fields: vec![],
                children: vec![],
            }],
            warnings: vec![],
        };
        assert!(first_class(&report).is_none());
        report.groups[0].kind = GroupKind::Class;
        assert_eq!(
            first_class(&report).map(|guid| guid.to_string()),
            Some("{00000000-0000-0000-0000-000000000001}".into())
        );
    }
}

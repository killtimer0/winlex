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
};
use winlex_format::declaration::{Region, Rendered, SymbolRole};

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
    copy: HWND,
    result: HWND,
    labels: Vec<HWND>,
    status: RefCell<String>,
    setting: Cell<bool>,
    body: RefCell<crate::rich_text::Content>,
    plain: RefCell<String>,
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
            copy: std::ptr::null_mut(),
            result: std::ptr::null_mut(),
            labels: vec![],
            status: RefCell::new(resources::text(IDS_COM_READY)),
            setting: Cell::new(true),
            body: RefCell::new(crate::rich_text::Content::new(String::new())),
            plain: RefCell::new(String::new()),
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
        send(self.input, EM_SETLIMITTEXT, 4096, 0);
        self.setting.set(false);
        Ok(())
    }
    fn execute(&self) {
        let raw = native_text(self.input);
        let target = String::from_utf16_lossy(&raw);
        let target = target.trim();
        if target.is_empty() {
            let empty = resources::text(IDS_COM_EMPTY);
            self.show(
                crate::rich_text::Content::new(empty.clone()),
                String::new(),
                &empty,
            );
            return;
        }
        let kind = selected(self.kind, KINDS, TargetKind::Auto);
        let views = selected(self.registry_view, VIEWS, Views::Both);
        let labels = Labels::from_resources();
        let report = com::query(
            &com::registry::Live,
            &Request {
                text: target.to_owned(),
                kind,
                views,
            },
        );
        let warnings: Vec<String> = report
            .warnings
            .iter()
            .map(|warning| warning_text(warning, &labels))
            .collect();
        let output = render(&report, &labels, &warnings);
        let status = resources::format(
            IDS_COM_STATUS,
            &[
                &report.groups.len().to_string(),
                &warnings.len().to_string(),
            ],
        );
        self.show(
            crate::rich_text::Content::from(output.body),
            output.copy,
            &status,
        );
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
        m.buttons(&mut area, [self.query, self.copy], &mut positions);
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
    pub fn destroy(&self) {}
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
}

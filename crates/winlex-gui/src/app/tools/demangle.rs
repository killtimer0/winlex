//! Symbol demangle tool: multi-line input, ABI selection and semantic results.
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
use winlex_core::tools::demangle::{self, Abi, Convention, Role, SymbolKind};
use winlex_format::declaration::{Region, Rendered, SymbolRole};

const MAX_LINES: usize = 1000;
const MAX_UNITS: usize = 256 * 1024;

const ABIS: &[(u16, Abi)] = &[
    (IDS_DEMANGLE_AUTO, Abi::Auto),
    (IDS_DEMANGLE_MSVC, Abi::Msvc),
    (IDS_DEMANGLE_ITANIUM, Abi::Itanium),
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
    abi: HWND,
    copy: HWND,
    result: HWND,
    status: RefCell<String>,
    labels: Vec<HWND>,
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
            abi: std::ptr::null_mut(),
            copy: std::ptr::null_mut(),
            result: std::ptr::null_mut(),
            status: RefCell::new(resources::text(IDS_DEMANGLE_EMPTY)),
            labels: vec![],
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
                crate::icons::Icon::Method,
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
                IDS_DEMANGLE_LABEL,
                DEMANGLE_INPUT,
                &mut self.input,
                w!("EDIT"),
                WS_TABSTOP
                    | WS_VSCROLL
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32
                    | ES_NOHIDESEL as u32,
                WS_EX_CLIENTEDGE,
            ),
            (
                IDS_DEMANGLE_ABI,
                DEMANGLE_ABI,
                &mut self.abi,
                w!("COMBOBOX"),
                WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
            ),
        ] {
            self.labels
                .push(unsafe { child(hwnd, w!("STATIC"), resources::wide_text(label), 0, 0) }?);
            *control = unsafe {
                child_ex(
                    hwnd,
                    class,
                    &[0],
                    style,
                    edge,
                    if id == DEMANGLE_INPUT {
                        DEMANGLE_INPUT
                    } else {
                        DEMANGLE_ABI
                    },
                )
            }?;
        }
        for &(label, _) in ABIS {
            send(
                self.abi,
                CB_ADDSTRING,
                0,
                resources::wide_text(label).as_ptr() as isize,
            );
        }
        send(self.abi, CB_SETCURSEL, 0, 0);
        self.copy = unsafe {
            child(
                hwnd,
                w!("BUTTON"),
                resources::wide_text(IDS_DEMANGLE_COPY_ALL),
                WS_TABSTOP,
                DEMANGLE_COPY_ALL,
            )
        }?;
        self.labels.push(unsafe {
            child(
                hwnd,
                w!("STATIC"),
                resources::wide_text(IDS_DEMANGLE_RESULT_LABEL),
                0,
                0,
            )
        }?);
        self.result = crate::rich_text::create(hwnd, DEMANGLE_RESULT)?;
        send(self.input, EM_SETLIMITTEXT, MAX_UNITS, 0);
        self.setting.set(false);
        Ok(())
    }
    fn refresh(&self) {
        if self.setting.get() || self.result.is_null() {
            return;
        }
        let selected = send(self.abi, CB_GETCURSEL, 0, 0);
        let abi = ABIS
            .get(selected.max(0) as usize)
            .map(|&(_, abi)| abi)
            .unwrap_or(Abi::Auto);
        let raw = native_text(self.input);
        let raw = String::from_utf16_lossy(&raw);
        let (symbols, truncated) = split_input(&raw);
        let (content, copy, status) = if symbols.is_empty() {
            let empty = resources::text(IDS_DEMANGLE_EMPTY);
            (
                crate::rich_text::Content::new(empty.clone()),
                String::new(),
                empty,
            )
        } else {
            let labels = Labels::from_resources();
            let output = render(&symbols, abi, &labels, truncated);
            let status = self.status_text(&output);
            (
                crate::rich_text::Content::from(output.body),
                output.copy,
                status,
            )
        };
        if let Err(error) = crate::rich_text::set_content(self.result, &content) {
            self.set_status(&format!("{error:#}"));
            return;
        }
        self.body.replace(content);
        self.plain.replace(copy);
        self.set_status(&status);
    }
    fn status_text(&self, output: &Output) -> String {
        let total = (output.restored + output.failed).to_string();
        let restored = output.restored.to_string();
        let failed = output.failed.to_string();
        let mut status = resources::format(IDS_DEMANGLE_STATUS, &[&total, &restored, &failed]);
        if let Some(error) = &output.first_error {
            status.push(' ');
            status.push_str(error);
        }
        if output.truncated {
            status.push(' ');
            status.push_str(&resources::format(
                IDS_DEMANGLE_TRUNCATED,
                &[&MAX_LINES.to_string(), &MAX_UNITS.to_string()],
            ));
        }
        status
    }
    fn set_status(&self, message: &str) {
        self.status.replace(message.to_owned());
        super::super::document::status_changed(self.ctx(), self.hwnd);
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
            IDS_DEMANGLE_LABEL,
            IDS_DEMANGLE_ABI,
            IDS_DEMANGLE_RESULT_LABEL,
        ]
        .into_iter()
        .map(|id| m.text_width(resources::wide_text(id)))
        .max()
        .unwrap_or(0)
        .min((area.right - area.left) / 3);
        let input_height = (m.row() * 5).max(m.height * 3);
        let mut line = layout::take_top(&mut area, input_height);
        let label = layout::take_left(&mut line, label_width);
        positions.push((
            self.labels[0],
            layout::rect(label.left, line.top + m.px(2), label_width, m.height),
        ));
        layout::take_left(&mut line, label_gap);
        positions.push((self.input, line));
        layout::take_top(&mut area, gap);

        let copy = m.button_size(self.copy);
        let combo = bounds(self.abi, self.hwnd);
        let row_height = (combo.bottom - combo.top).max(copy.cy);
        let mut line = layout::take_top(&mut area, row_height);
        let label = layout::take_left(&mut line, label_width);
        positions.push((self.labels[1], layout::center_vertical(label, m.height)));
        layout::take_left(&mut line, label_gap);
        let right = line.right;
        line.right = (right - copy.cx - gap).max(line.left);
        positions.push((
            self.abi,
            layout::center_vertical(line, combo.bottom - combo.top),
        ));
        positions.push((
            self.copy,
            layout::center_vertical(
                layout::rect(right - copy.cx, line.top, copy.cx, row_height),
                copy.cy,
            ),
        ));
        layout::take_top(&mut area, gap);
        positions.push((
            self.labels[2],
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
        for h in self
            .labels
            .iter()
            .copied()
            .chain([self.input, self.abi, self.copy])
        {
            super::super::bind(h, m.font);
        }
        let ctx = self.ctx();
        let mut rect = bounds(self.abi, self.hwnd);
        rect.bottom = rect.top + m.row() * 9;
        ctx.checked(set_bounds(self.abi, rect));
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
        false
    }
    pub fn message(&self, msg: u32, w: usize, l: isize) -> Option<isize> {
        match msg {
            WM_COMMAND => {
                let id = w as u16;
                let code = (w >> 16) as u32;
                match id {
                    DEMANGLE_INPUT if code == EN_CHANGE => self.refresh(),
                    DEMANGLE_ABI if code == CBN_SELENDOK => self.refresh(),
                    DEMANGLE_COPY_ALL if code == BN_CLICKED => {
                        let copy_text: Vec<u16> = self.plain.borrow().encode_utf16().collect();
                        if let Err(error) = copy_unicode(self.hwnd, &copy_text) {
                            self.set_status(&format!("{error:#}"));
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
    let mut text = text_wide(hwnd);
    text.pop();
    text
}

struct Labels {
    msvc: String,
    itanium: String,
    function: String,
    data: String,
    special: String,
    unknown: String,
    no_return: String,
    error: String,
}

impl Labels {
    fn from_resources() -> Self {
        Self {
            msvc: resources::text(IDS_DEMANGLE_MSVC),
            itanium: resources::text(IDS_DEMANGLE_ITANIUM),
            function: resources::text(IDS_DEMANGLE_FUNCTION),
            data: resources::text(IDS_DEMANGLE_DATA),
            special: resources::text(IDS_DEMANGLE_SPECIAL),
            unknown: resources::text(IDS_DEMANGLE_UNKNOWN),
            no_return: resources::text(IDS_DEMANGLE_NO_RETURN),
            error: resources::text(IDS_DEMANGLE_ERROR),
        }
    }
    fn abi(&self, abi: Abi) -> &str {
        match abi {
            Abi::Msvc => &self.msvc,
            Abi::Itanium => &self.itanium,
            Abi::Auto => "",
        }
    }
    fn kind(&self, kind: SymbolKind) -> &str {
        match kind {
            SymbolKind::Function => &self.function,
            SymbolKind::Data => &self.data,
            SymbolKind::Special => &self.special,
            SymbolKind::Unknown => &self.unknown,
        }
    }
}

struct Output {
    body: Rendered,
    copy: String,
    restored: usize,
    failed: usize,
    first_error: Option<String>,
    truncated: bool,
}

fn render(symbols: &[&str], abi: Abi, labels: &Labels, truncated: bool) -> Output {
    let mut body = Rendered::default();
    let mut copy = String::new();
    let mut restored = 0;
    let mut failed = 0;
    let mut first_error = None;
    for (index, symbol) in symbols.iter().enumerate() {
        if index != 0 {
            body.text.push('\n');
        }
        body.text.push_str(symbol);
        body.text.push('\n');
        match demangle::demangle(symbol, abi) {
            Ok(result) => {
                restored += 1;
                let start = body.text.encode_utf16().count();
                body.text.push_str(&result.text);
                body.text.push('\n');
                for span in &result.spans {
                    body.regions.push(Region::symbol(
                        start + span.start..start + span.end,
                        role(span.role),
                    ));
                }
                body.text.push_str("  ");
                body.text.push_str(labels.abi(result.abi));
                body.text.push_str(" · ");
                body.text.push_str(labels.kind(result.kind));
                if let Some(convention) = result.convention {
                    body.text.push_str(" · ");
                    body.text.push_str(convention_name(convention));
                } else if result.abi == Abi::Itanium && result.kind == SymbolKind::Function {
                    body.text.push_str(" · ");
                    body.text.push_str(&labels.no_return);
                }
                body.text.push('\n');
                if !copy.is_empty() {
                    copy.push('\n');
                }
                copy.push_str(&result.text);
            }
            Err(error) => {
                failed += 1;
                let message = format!("{}: {}", labels.error, error);
                first_error.get_or_insert_with(|| format!("{symbol}: {message}"));
                body.text.push_str("  ");
                body.text.push_str(&message);
                body.text.push('\n');
            }
        }
    }
    Output {
        body,
        copy,
        restored,
        failed,
        first_error,
        truncated,
    }
}

fn role(role: Role) -> SymbolRole {
    match role {
        Role::Method => SymbolRole::Method,
    }
}

fn convention_name(convention: Convention) -> &'static str {
    match convention {
        Convention::Cdecl => "cdecl",
        Convention::Stdcall => "stdcall",
        Convention::Fastcall => "fastcall",
        Convention::Thiscall => "thiscall",
        Convention::Pascal => "pascal",
        Convention::Regcall => "regcall",
    }
}

fn split_input(value: &str) -> (Vec<&str>, bool) {
    let mut truncated = value.encode_utf16().count() > MAX_UNITS;
    let value = if truncated {
        let mut units = 0;
        let mut end = 0;
        for (index, character) in value.char_indices() {
            let width = character.len_utf16();
            if units + width > MAX_UNITS {
                break;
            }
            units += width;
            end = index + character.len_utf8();
        }
        &value[..end]
    } else {
        value
    };
    let mut lines: Vec<&str> = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() > MAX_LINES {
        truncated = true;
        lines.truncate(MAX_LINES);
    }
    (lines, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> Labels {
        Labels {
            msvc: "Visual C++".into(),
            itanium: "Itanium".into(),
            function: "function".into(),
            data: "data".into(),
            special: "special".into(),
            unknown: "symbol".into(),
            no_return: "return type not encoded".into(),
            error: "cannot restore".into(),
        }
    }

    fn text_of(result: &Output, span: &Region) -> String {
        let units: Vec<u16> = result.body.text.encode_utf16().collect();
        String::from_utf16(&units[span.range.clone()]).unwrap()
    }

    #[test]
    fn spans_point_at_demangled_names_on_their_own_line() {
        let output = render(
            &["?foo@@YAHH@Z", "_ZN5space3fooEibc"],
            Abi::Auto,
            &labels(),
            false,
        );
        assert_eq!(output.restored, 2);
        assert_eq!(output.failed, 0);
        let names: Vec<_> = output
            .body
            .regions
            .iter()
            .map(|region| text_of(&output, region))
            .collect();
        assert_eq!(names, ["foo", "space::foo"]);
        assert!(output.body.text.contains("Visual C++ · function · cdecl"));
        assert!(
            output
                .body
                .text
                .contains("Itanium · function · return type not encoded")
        );
        assert_eq!(output.copy.lines().count(), 2);
        assert!(!output.copy.contains("Visual C++"));
    }

    #[test]
    fn failures_stay_inline_and_keep_other_results() {
        let output = render(
            &["?foo@@YAHH@Z", "not-a-symbol"],
            Abi::Auto,
            &labels(),
            false,
        );
        assert_eq!(output.restored, 1);
        assert_eq!(output.failed, 1);
        assert!(
            output
                .first_error
                .as_deref()
                .unwrap()
                .starts_with("not-a-symbol: cannot restore")
        );
        assert_eq!(output.copy.lines().count(), 1);
        assert!(output.body.text.contains("cannot restore"));
    }

    #[test]
    fn input_is_trimmed_and_bounded_by_lines() {
        let input = "\r\n  _Z3fooi  \r\n\r\n_ZN3Foo3barEi\r\n";
        let (lines, truncated) = split_input(input);
        assert_eq!(lines, ["_Z3fooi", "_ZN3Foo3barEi"]);
        assert!(!truncated);

        let many = "_Z3fooi\n".repeat(MAX_LINES + 10);
        let (lines, truncated) = split_input(&many);
        assert_eq!(lines.len(), MAX_LINES);
        assert!(truncated);

        let long = "x".repeat(MAX_UNITS + 10);
        let (lines, truncated) = split_input(&long);
        assert!(truncated);
        assert_eq!(lines[0].encode_utf16().count(), MAX_UNITS);
    }

    #[test]
    fn utf16_spans_account_for_supplementary_characters() {
        let output = render(&["_Z3fooi"], Abi::Auto, &labels(), false);
        let units: Vec<u16> = output.body.text.encode_utf16().collect();
        let region = &output.body.regions[0];
        assert_eq!(
            String::from_utf16(&units[region.range.clone()]).unwrap(),
            "foo"
        );
    }
}

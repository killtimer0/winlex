//! Symbol demangle tool: multi-line input, ABI selection and semantic results.
use super::Shell;
use super::combo_value;
use crate::app::context::Context;
use crate::native::text;
use crate::{
    icons::Icon,
    layout,
    native::*,
    resources::{self, *},
};
use std::ops::{Deref, DerefMut};
use winlex_core::tools::demangle::{self, Abi, SymbolKind};
use winlex_format::declaration::{Region, Rendered, SymbolRole};

const MAX_LINES: usize = 1000;
const MAX_UNITS: usize = 256 * 1024;

const ABIS: &[(u16, Abi)] = &[
    (IDS_AUTO, Abi::Auto),
    (IDS_DEMANGLE_MSVC, Abi::Msvc),
    (IDS_DEMANGLE_ITANIUM, Abi::Itanium),
];

pub(in crate::app) struct View {
    shell: Shell,
    input: HWND,
    abi: HWND,
}

impl Deref for View {
    type Target = Shell;
    fn deref(&self) -> &Shell {
        &self.shell
    }
}
impl DerefMut for View {
    fn deref_mut(&mut self) -> &mut Shell {
        &mut self.shell
    }
}

impl View {
    pub fn new(ctx: &Context, icon: Icon) -> Self {
        Self {
            shell: Shell::new(ctx, resources::text(IDS_DEMANGLE_EMPTY), icon),
            input: std::ptr::null_mut(),
            abi: std::ptr::null_mut(),
        }
    }
    pub fn create(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.hwnd = hwnd;
        for (label, id, control, class, style, edge) in [
            (
                IDS_DEMANGLE_LABEL,
                DEMANGLE_INPUT,
                &mut self.input,
                w!("EDIT"),
                WS_VSCROLL
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
                WS_VSCROLL | CBS_DROPDOWNLIST as u32,
                0,
            ),
        ] {
            *control = self
                .shell
                .create_field(hwnd, label, id, class, style, edge)?;
        }
        self.shell
            .populate_combo(self.abi, ABIS.iter().map(|&(label, _)| label));
        self.shell.create_copy_button(hwnd, DEMANGLE_COPY_ALL)?;
        self.shell.create_result(hwnd, DEMANGLE_RESULT)?;
        send(self.input, EM_SETLIMITTEXT, MAX_UNITS, 0);
        Ok(())
    }
    fn refresh(&self) {
        if self.result.is_null() {
            return;
        }
        let abi = combo_value(self.abi, ABIS, Abi::Auto);
        let raw = text(self.input);
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
        self.shell.show(content, copy, &status);
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
    pub fn layout(&self) {
        let Some((m, mut area, gap, label_gap)) = self.layout_start() else {
            return;
        };
        let mut positions = vec![];
        let label_width = self.label_column(
            &m,
            &[IDS_DEMANGLE_LABEL, IDS_DEMANGLE_ABI, IDS_RESULT_LABEL],
            area.right - area.left,
        );
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
        self.finish_layout(&m, &mut area, self.labels[2], &mut positions);
    }
    pub fn focus(&self) {
        focus(self.input)
    }
    pub fn prepare(&self) {
        self.prepare_common(&[self.input, self.abi], &[self.abi]);
        self.layout();
    }
    pub fn key(&self, msg: &MSG) -> bool {
        self.ctrl_a(msg, &[self.input, self.result])
    }
    pub(super) fn command(&self, id: u16, code: u32) -> bool {
        match id {
            DEMANGLE_INPUT if code == EN_CHANGE => {
                self.refresh();
                true
            }
            DEMANGLE_ABI if code == CBN_SELENDOK => {
                self.refresh();
                true
            }
            DEMANGLE_COPY_ALL if code == BN_CLICKED => {
                self.copy_plain();
                true
            }
            _ => false,
        }
    }
    pub fn destroy(&self) {}
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
            function: resources::text(IDS_FUNCTION),
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
                        SymbolRole::Method,
                    ));
                }
                body.text.push_str("  ");
                body.text.push_str(labels.abi(result.abi));
                body.text.push_str(" · ");
                body.text.push_str(labels.kind(result.kind));
                if let Some(convention) = result.convention {
                    body.text.push_str(" · ");
                    body.text.push_str(convention.name());
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

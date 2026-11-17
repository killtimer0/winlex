//! Symbol demangle tool: explicit batch submission and semantic results.
use super::{Shell, combo_value};
use crate::app::context::Context;
use crate::native::text;
use crate::{
    icons::Icon,
    layout,
    native::*,
    resources::{self, *},
    tasks::tools::{Completion, Job, demangle::MAX_INPUT_UNITS},
};
use std::{
    cell::Cell,
    ops::{Deref, DerefMut},
};
use winlex_core::tools::demangle::Abi;

const ABIS: &[(u16, Abi)] = &[
    (IDS_AUTO, Abi::Auto),
    (IDS_DEMANGLE_MSVC, Abi::Msvc),
    (IDS_DEMANGLE_ITANIUM, Abi::Itanium),
];

pub(in crate::app) struct View {
    shell: Shell,
    input: HWND,
    abi: HWND,
    query: HWND,
    ticket: Cell<Option<u64>>,
    revision: Cell<u64>,
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
            query: std::ptr::null_mut(),
            ticket: Cell::new(None),
            revision: Cell::new(0),
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
        self.query = self.shell.create_button(
            hwnd,
            IDS_DEMANGLE_QUERY,
            DEMANGLE_QUERY,
            BS_DEFPUSHBUTTON as u32,
        )?;
        self.shell.create_copy_button(hwnd, DEMANGLE_COPY_ALL)?;
        self.shell
            .create_result(hwnd, DEMANGLE_RESULT, IDS_DEMANGLE_HINT)?;
        send(self.input, EM_SETLIMITTEXT, MAX_INPUT_UNITS, 0);
        self.shell.subclass_text(self.input)?;
        Ok(())
    }
    fn execute(&self) {
        if unsafe { IsWindowEnabled(self.query) } == 0 {
            return;
        }
        let job = Job::Demangle {
            revision: self.revision.get(),
            input: text(self.input),
            abi: combo_value(self.abi, ABIS, Abi::Auto),
        };
        if self.submit(&self.ticket, job) {
            unsafe { EnableWindow(self.query, 0) };
            self.set_status(&resources::text(IDS_DEMANGLE_QUERYING));
        }
    }
    fn changed(&self) {
        self.revision.set(self.revision.get() + 1);
        self.set_status(&resources::text(IDS_TOOL_STALE));
    }
    pub fn complete(&self, id: u64, completion: Completion) {
        let Completion::Demangle { revision, result } = completion else {
            return;
        };
        if self.ticket.get() != Some(id) {
            return;
        }
        self.ticket.set(None);
        unsafe { EnableWindow(self.query, 1) };
        match result {
            Ok(output) => {
                let status = if revision != self.revision.get() {
                    resources::text(IDS_TOOL_STALE)
                } else {
                    output.status
                };
                self.shell.show(output.content, output.copy, &status);
            }
            Err(error) => self.set_status(&format!("{error:#}")),
        }
    }
    pub fn layout(&self) {
        let mut positions = vec![];
        let Some((m, mut area, gap, label_gap)) = self.layout_start(&mut positions) else {
            return;
        };
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
        let combo = bounds(self.abi, self.hwnd);
        let row_height = (combo.bottom - combo.top).max(m.height);
        let mut line = layout::take_top(&mut area, row_height);
        let label = layout::take_left(&mut line, label_width);
        positions.push((self.labels[1], layout::center_vertical(label, m.height)));
        layout::take_left(&mut line, label_gap);
        let line = layout::take_left(&mut line, m.combo_width(self.abi));
        positions.push((
            self.abi,
            layout::center_vertical(line, combo.bottom - combo.top),
        ));
        layout::take_top(&mut area, gap);
        m.buttons(&mut area, [self.query], &mut positions);
        self.finish_layout(&m, &mut area, self.labels[2], &mut positions);
    }
    pub fn focus(&self) {
        focus(self.input)
    }
    pub fn prepare(&self) {
        self.prepare_common(&[self.input, self.abi, self.query], &[self.abi]);
        self.layout();
    }
    pub fn key(&self, msg: &MSG) -> bool {
        if self.ctrl_a(msg, &[self.input, self.result]) {
            return true;
        }
        if unsafe { GetKeyState(VK_CONTROL as i32) } < 0 && self.submits(msg, self.input) {
            self.execute();
            return true;
        }
        false
    }
    pub(super) fn command(&self, id: u16, code: u32) -> bool {
        match id {
            DEMANGLE_INPUT if code == EN_CHANGE => self.changed(),
            DEMANGLE_ABI if code == CBN_SELENDOK => self.changed(),
            DEMANGLE_QUERY if code == BN_CLICKED => self.execute(),
            DEMANGLE_COPY_ALL if code == BN_CLICKED => self.copy_plain(),
            _ => return false,
        }
        true
    }
    pub fn destroy(&self) {
        if let Some(id) = self.ticket.take() {
            self.ctx().cancel_job(id);
        }
    }
    pub fn worker_stopped(&self) {
        if self.ticket.take().is_some() {
            unsafe { EnableWindow(self.query, 1) };
            self.set_status(&resources::text(IDS_WORKER_STOPPED));
        }
    }
}

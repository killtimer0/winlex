use super::Binding;
use crate::{
    app::context::Context,
    dialog::{FONT_ONLY, MOVE_X, MOVE_Y, SIZE_X, SIZE_Y, modal},
    native::*,
    resources::{self, *},
};
use anyhow::Context as _;
use std::cell::{Cell, RefCell};
use winlex_core::DeclarationTarget;

fn field(index: usize) -> u16 {
    GENERIC_FIELD_BASE + index as u16
}
fn label(index: usize) -> u16 {
    GENERIC_LABEL_BASE + index as u16
}

unsafe extern "system" fn field_proc(
    hwnd: HWND,
    message: u32,
    w: usize,
    l: isize,
    id: usize,
    count: usize,
) -> isize {
    unsafe {
        if message == WM_GETDLGCODE && w == VK_TAB as usize {
            return default_subclass(hwnd, message, w, l) | DLGC_WANTTAB as isize;
        }
        if message == WM_KEYDOWN && w == VK_TAB as usize {
            let parent = GetParent(hwnd);
            let current = (GetDlgCtrlID(hwnd) - GENERIC_FIELD_BASE as i32) as usize;
            let previous = GetKeyState(VK_SHIFT as i32) < 0;
            let next = if previous {
                current.checked_sub(1)
            } else {
                (current + 1 < count).then_some(current + 1)
            };
            let target = if let Some(index) = next {
                let target = GetDlgItem(parent, field(index) as i32);
                // Reveal the next row before assigning keyboard focus.
                send(
                    parent,
                    WM_COMMAND,
                    field(index) as usize | ((EN_SETFOCUS as usize) << 16),
                    target as isize,
                );
                target
            } else {
                GetDlgItem(
                    parent,
                    if previous {
                        IDCANCEL
                    } else {
                        GENERIC_APPLY as i32
                    },
                )
            };
            focus(target);
            return 0;
        }
        if message == WM_NCDESTROY {
            remove_subclass(hwnd, Some(field_proc), id);
        }
        default_subclass(hwnd, message, w, l)
    }
}

struct Form {
    count: usize,
    first: Cell<usize>,
}
impl Form {
    fn layout(&self, hwnd: HWND, focused: Option<usize>) -> anyhow::Result<()> {
        let area = bounds(unsafe { GetDlgItem(hwnd, GENERIC_FORM as i32) }, hwnd);
        let row = bounds(unsafe { GetDlgItem(hwnd, GENERIC_ROW as i32) }, hwnd);
        let height = (row.bottom - row.top).max(1);
        let page = ((area.bottom - area.top) / height).max(1) as usize;
        let mut first = self.first.get().min(self.count.saturating_sub(page));
        if let Some(index) = focused {
            if index < first {
                first = index;
            }
            if index >= first + page {
                first = index + 1 - page;
            }
        }
        self.first.set(first);
        let label_rect = bounds(
            unsafe { GetDlgItem(hwnd, GENERIC_LABEL_TEMPLATE as i32) },
            hwnd,
        );
        let field_rect = bounds(
            unsafe { GetDlgItem(hwnd, GENERIC_FIELD_TEMPLATE as i32) },
            hwnd,
        );
        let mut positions = Vec::new();
        for index in 0..self.count {
            let visible = (first..first + page).contains(&index);
            let top = area.top + (index as i32 - first as i32) * height;
            for (id, template) in [(label(index), label_rect), (field(index), field_rect)] {
                let control = unsafe { GetDlgItem(hwnd, id as i32) };
                crate::native::show(control, visible);
                positions.push((
                    control,
                    crate::layout::rect(
                        area.left,
                        top + template.top - row.top,
                        area.right - area.left,
                        template.bottom - template.top,
                    ),
                ));
            }
        }
        place_all(&positions)?;
        unsafe {
            let scroll = GetDlgItem(hwnd, GENERIC_SCROLL as i32);
            let info = SCROLLINFO {
                cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
                nMin: 0,
                nMax: self.count as i32 - 1,
                nPage: page as u32,
                nPos: first as i32,
                nTrackPos: 0,
            };
            SetScrollInfo(scroll, SB_CTL, &info, 1);
            crate::native::show(scroll, self.count > page);
        }
        Ok(())
    }
}

pub(super) fn show(
    ctx: &Context,
    owner: HWND,
    binding: &Binding,
) -> anyhow::Result<Option<DeclarationTarget>> {
    let parameters = binding
        .catalog
        .type_parameter_names(binding.target.id, binding.architecture)?;
    if parameters.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(parameters.len() <= 64, "too many type arguments");
    let initial = binding
        .target
        .arguments
        .iter()
        .map(|ty| {
            binding
                .catalog
                .argument_expression(binding.target.id, ty, binding.architecture)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let form = Form {
        count: parameters.len(),
        first: Cell::new(0),
    };
    let result = RefCell::new(None);
    let mut rules = vec![
        (GENERIC_HINT as i32, SIZE_X),
        (GENERIC_FORM as i32, SIZE_X | SIZE_Y),
        (GENERIC_ROW as i32, SIZE_X),
        (GENERIC_LABEL_TEMPLATE as i32, SIZE_X),
        (GENERIC_FIELD_TEMPLATE as i32, SIZE_X),
        (GENERIC_SCROLL as i32, MOVE_X | SIZE_Y),
        (GENERIC_ERROR as i32, SIZE_X | MOVE_Y),
        (GENERIC_APPLY as i32, MOVE_X | MOVE_Y),
        (IDCANCEL, MOVE_X | MOVE_Y),
    ];
    for index in 0..form.count {
        rules.extend([
            (label(index) as i32, FONT_ONLY),
            (field(index) as i32, FONT_ONLY),
        ]);
    }
    modal::show_prepared(
        owner,
        ctx.dpi(),
        resources::dialog(IDD_GENERIC_BIND)?,
        &rules,
        &|hwnd, baseline| {
            unsafe {
                // Create all rows before the shared layout captures fonts and DLU bounds.
                let font = send(hwnd, WM_GETFONT, 0, 0);
                let row = bounds(GetDlgItem(hwnd, GENERIC_ROW as i32), hwnd);
                let row_height = row.bottom - row.top;
                for id in [GENERIC_LABEL_TEMPLATE, GENERIC_FIELD_TEMPLATE] {
                    crate::native::show(GetDlgItem(hwnd, id as i32), false);
                }
                let mut previous = GetDlgItem(hwnd, GENERIC_FIELD_TEMPLATE as i32);
                for index in 0..form.count {
                    for (id, prototype, class) in [
                        (label(index), GENERIC_LABEL_TEMPLATE, w!("STATIC")),
                        (field(index), GENERIC_FIELD_TEMPLATE, w!("EDIT")),
                    ] {
                        let prototype = GetDlgItem(hwnd, prototype as i32);
                        let control = child_ex(
                            hwnd,
                            class,
                            &[0],
                            GetWindowLongPtrW(prototype, GWL_STYLE) as u32,
                            GetWindowLongPtrW(prototype, GWL_EXSTYLE) as u32,
                            id,
                        )?;
                        send(control, WM_SETFONT, font as usize, 0);
                        set_bounds(control, bounds(prototype, hwnd))?;
                        anyhow::ensure!(
                            SetWindowPos(
                                control,
                                previous,
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE
                            ) != 0,
                            "Order type argument fields"
                        );
                        previous = control;
                    }
                }
                let mut visible = form.count.min(4) as i32;
                let mut width_delta = 0;
                let mut units = RECT {
                    left: 0,
                    top: 0,
                    right: 4,
                    bottom: 8,
                };
                anyhow::ensure!(
                    MapDialogRect(hwnd, &mut units) != 0,
                    "Measure type argument form"
                );
                let row_units = MulDiv(row_height, 8, units.bottom);
                if let Some(info) = monitor(owner) {
                    let metrics = ctx.metrics(owner);
                    if ctx.dpi().mode == crate::dpi::Mode::Pmv1 {
                        units.right =
                            MulDiv(units.right, metrics.dpi as i32, ctx.dpi().system() as i32);
                        units.bottom =
                            MulDiv(units.bottom, metrics.dpi as i32, ctx.dpi().system() as i32);
                    }
                    let width = (info.rcWork.right - info.rcWork.left - metrics.px(40)) * 4
                        / units.right.max(1);
                    let height = (info.rcWork.bottom - info.rcWork.top - metrics.px(80)) * 8
                        / units.bottom.max(1);
                    width_delta = width.clamp(240, baseline.cx) - baseline.cx;
                    visible = visible.min(((height - baseline.cy) / row_units + 1).max(1));
                }
                baseline.cx += width_delta;
                baseline.cy += row_units * (visible - 1);
            }
            Ok(())
        },
        &|hwnd, message, w, l| {
            unsafe {
                if message == WM_INITDIALOG {
                    for (index, parameter) in parameters.iter().enumerate() {
                        set_text(
                            GetDlgItem(hwnd, label(index) as i32),
                            &parameter.replace('&', "&&"),
                        );
                        let control = GetDlgItem(hwnd, field(index) as i32);
                        anyhow::ensure!(
                            set_subclass(control, Some(field_proc), 1, form.count) != 0,
                            "Observe type argument keyboard navigation"
                        );
                        send(control, EM_SETLIMITTEXT, 8192, 0);
                        set_text(control, initial.get(index).map_or("", String::as_str));
                    }
                }
                if matches!(message, WM_INITDIALOG | WM_SIZE | WM_DPICHANGED) {
                    let focused = (0..form.count)
                        .find(|&index| GetFocus() == GetDlgItem(hwnd, field(index) as i32));
                    form.layout(hwnd, focused)?;
                }
                if message == WM_VSCROLL && l == GetDlgItem(hwnd, GENERIC_SCROLL as i32) as isize {
                    let mut info: SCROLLINFO = std::mem::zeroed();
                    info.cbSize = std::mem::size_of::<SCROLLINFO>() as u32;
                    info.fMask = SIF_ALL;
                    GetScrollInfo(l as HWND, SB_CTL, &mut info);
                    let pos = form.first.get() as i32;
                    let next = match w as u16 as i32 {
                        SB_LINEUP => pos - 1,
                        SB_LINEDOWN => pos + 1,
                        SB_PAGEUP => pos - info.nPage as i32,
                        SB_PAGEDOWN => pos + info.nPage as i32,
                        SB_TOP => 0,
                        SB_BOTTOM => form.count as i32,
                        SB_THUMBTRACK | SB_THUMBPOSITION => info.nTrackPos,
                        _ => pos,
                    };
                    form.first.set(next.max(0) as usize);
                    form.layout(hwnd, None)?;
                }
                if message == WM_MOUSEWHEEL {
                    let delta = (w >> 16) as u16 as i16 as i32;
                    form.first.set(
                        (form.first.get() as i32 - delta / WHEEL_DELTA as i32).max(0) as usize,
                    );
                    form.layout(hwnd, None)?;
                }
                if message == WM_COMMAND
                    && (w >> 16) as u32 == EN_SETFOCUS
                    && let Some(index) = (0..form.count).find(|&index| field(index) == w as u16)
                {
                    form.layout(hwnd, Some(index))?;
                }
                if message == WM_COMMAND && w as u16 == GENERIC_APPLY {
                    let mut arguments = Vec::with_capacity(form.count);
                    for (index, parameter) in parameters.iter().enumerate() {
                        let control = GetDlgItem(hwnd, field(index) as i32);
                        let value = crate::native::text(control);
                        // Preserve source-scoped unresolved references when left unchanged.
                        let parsed = if initial.get(index) == Some(&value) {
                            Ok(binding.target.arguments[index].clone())
                        } else {
                            binding.catalog.parse_argument(
                                binding.target.id,
                                &value,
                                binding.architecture,
                            )
                        }
                        .and_then(|argument| {
                            binding
                                .catalog
                                .validate_winrt_argument(
                                    binding.target.id,
                                    &argument,
                                    binding.architecture,
                                )
                                .with_context(|| {
                                    resources::format(IDS_GENERIC_WINRT_ARGUMENT, &[&value])
                                })?;
                            Ok(argument)
                        });
                        match parsed {
                            Ok(argument) => arguments.push(argument),
                            Err(error) => {
                                set_text(
                                    GetDlgItem(hwnd, GENERIC_ERROR as i32),
                                    &format!("{parameter}: {error:#}"),
                                );
                                form.layout(hwnd, Some(index))?;
                                focus(control);
                                return Ok(());
                            }
                        }
                    }
                    match binding.catalog.bind_target(
                        &DeclarationTarget {
                            id: binding.target.id,
                            arguments,
                        },
                        binding.architecture,
                    ) {
                        Ok(target) => {
                            *result.borrow_mut() = Some(target);
                            EndDialog(hwnd, 1);
                        }
                        Err(error) => set_text(
                            GetDlgItem(hwnd, GENERIC_ERROR as i32),
                            &format!("{error:#}"),
                        ),
                    }
                }
            }
            Ok(())
        },
    )?;
    Ok(result.into_inner())
}

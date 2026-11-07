use super::Preferences;
use crate::app::context::Context;
use crate::{
    dialog::{property_sheet, template_size},
    native::*,
    resources::{self, *},
};
use std::{cell::Cell, mem::size_of, ptr};

mod general;
mod search;

// Standard property-sheet button ID from prsht.h (absent from windows-sys).
const ID_APPLY_NOW: i32 = 0x3021;

struct Page {
    template: u16,
    title: u16,
    initialize: fn(HWND, Preferences),
    read: fn(HWND, &mut Preferences) -> anyhow::Result<()>,
}

const PAGES: &[Page] = &[general::PAGE, search::PAGE];

struct Dialog<'a> {
    ctx: &'a Context,
    pages: Cell<[HWND; PAGES.len()]>,
}

struct PageState<'a> {
    dialog: &'a Dialog<'a>,
    index: usize,
}
pub(in super::super) fn open(ctx: &Context) {
    super::super::status::report(ctx, show(ctx));
}

fn show(ctx: &Context) -> anyhow::Result<()> {
    let state = Dialog {
        ctx,
        pages: Cell::new([ptr::null_mut(); PAGES.len()]),
    };
    let bindings: [_; PAGES.len()] = std::array::from_fn(|index| PageState {
        dialog: &state,
        index,
    });
    let mut pages: [PROPSHEETPAGEW; PAGES.len()] = unsafe { std::mem::zeroed() };
    let mut baseline = Vec::with_capacity(PAGES.len());
    for (index, definition) in PAGES.iter().enumerate() {
        let template = resources::dialog(definition.template)?;
        baseline.push(template_size(template)?);
        let page = &mut pages[index];
        page.dwSize = size_of::<PROPSHEETPAGEW>() as u32;
        page.dwFlags = PSP_DLGINDIRECT | PSP_USETITLE | PSP_PREMATURE;
        page.hInstance = instance();
        page.Anonymous1.pResource = template.as_ptr().cast_mut().cast();
        page.pszTitle = resources::wide_text(definition.title).as_ptr();
        page.pfnDlgProc = Some(page_proc);
        page.lParam = &bindings[index] as *const PageState<'_> as isize;
    }
    let mut header: PROPSHEETHEADERW_V2 = unsafe { std::mem::zeroed() };
    header.dwSize = size_of::<PROPSHEETHEADERW_V2>() as u32;
    header.dwFlags = PSH_PROPSHEETPAGE | PSH_NOCONTEXTHELP | PSH_USECALLBACK;
    header.hwndParent = ctx.frame();
    header.hInstance = instance();
    header.pszCaption = resources::wide_text(IDS_SETTINGS).as_ptr();
    header.nPages = pages.len() as u32;
    header.Anonymous3.ppsp = pages.as_mut_ptr();
    property_sheet::show(&mut header, ctx.dpi(), baseline, 0, sheet_callback)?;
    Ok(())
}

unsafe extern "system" fn sheet_callback(hwnd: HWND, message: u32, _l: isize, _data: usize) -> i32 {
    unsafe {
        if message == PSCB_INITIALIZED {
            set_text(GetDlgItem(hwnd, IDOK), &resources::text(IDS_SETTINGS_OK));
            set_text(
                GetDlgItem(hwnd, IDCANCEL),
                &resources::text(IDS_SETTINGS_CANCEL),
            );
            set_text(
                GetDlgItem(hwnd, ID_APPLY_NOW),
                &resources::text(IDS_SETTINGS_APPLY),
            );
        }
        0
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, message: u32, _w: usize, l: isize) -> isize {
    unsafe {
        if message == WM_INITDIALOG {
            let page = &*(l as *const PROPSHEETPAGEW);
            let binding = &*(page.lParam as *const PageState<'_>);
            let state = binding.dialog;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, page.lParam as _);
            (PAGES[binding.index].initialize)(hwnd, super::preferences(state.ctx));
            let mut pages = state.pages.get();
            pages[binding.index] = hwnd;
            state.pages.set(pages);
            return 1;
        }
        let Some(binding) =
            (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const PageState<'_>).as_ref()
        else {
            return 0;
        };
        let state = binding.dialog;
        match message {
            // Child-control notifications only trigger a read/compare, never
            // saving. No second list of editable control IDs needs updating.
            WM_COMMAND if l != 0 => {
                if state.pages.get().iter().all(|page| !page.is_null()) {
                    let changed = read_preferences(state).map_or(true, |preferences| {
                        preferences != super::preferences(state.ctx)
                    });
                    for page in state.pages.get() {
                        send(
                            GetParent(hwnd),
                            if changed { PSM_CHANGED } else { PSM_UNCHANGED },
                            page as usize,
                            0,
                        );
                    }
                }
            }
            WM_NOTIFY if (*(l as *const NMHDR)).code == PSN_APPLY => {
                // Every page receives PSN_APPLY. The first registered page
                // coordinates validation and saving for the whole sheet.
                if binding.index != 0 {
                    SetWindowLongPtrW(hwnd, DWLP_MSGRESULT as i32, PSNRET_NOERROR as _);
                    return 1;
                }
                let result = match read_preferences(state)
                    .and_then(|preferences| super::apply_preferences(state.ctx, preferences))
                {
                    Ok(()) => PSNRET_NOERROR,
                    Err(error) => {
                        if let Err(display_error) = crate::message::error(
                            GetParent(hwnd),
                            &resources::text(IDS_SETTINGS),
                            &format!("{error:#}"),
                        ) {
                            super::super::status::show_notice(
                                state.ctx,
                                format!("{error:#}\n{display_error:#}"),
                            );
                        }
                        PSNRET_INVALID_NOCHANGEPAGE
                    }
                };
                SetWindowLongPtrW(hwnd, DWLP_MSGRESULT as i32, result as _);
                return 1;
            }
            _ => {}
        }
        0
    }
}

fn read_preferences(state: &Dialog<'_>) -> anyhow::Result<Preferences> {
    let windows = state.pages.get();
    anyhow::ensure!(
        windows.iter().all(|hwnd| !hwnd.is_null()),
        "Settings pages are unavailable"
    );
    let mut preferences = super::preferences(state.ctx);
    for (page, hwnd) in PAGES.iter().zip(windows) {
        (page.read)(hwnd, &mut preferences)?;
    }
    Ok(preferences)
}

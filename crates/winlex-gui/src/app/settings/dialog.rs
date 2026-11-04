use super::super::frame;
use super::{Preferences, THEMES, catalog};
use crate::app::context::Context;
use crate::{
    dialog::{property_sheet, template_size},
    native::*,
    resources::{self, *},
};
use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr,
};
use winlex_core::EntityCategory;

const FILTERS: &[(u16, EntityCategory)] = &[
    (FILTER_TYPE, EntityCategory::Type),
    (FILTER_METHOD, EntityCategory::Method),
    (FILTER_CONSTANT, EntityCategory::Constant),
    (FILTER_FIELD, EntityCategory::Field),
    (FILTER_PROPERTY, EntityCategory::Property),
    (FILTER_EVENT, EntityCategory::Event),
    (FILTER_REFERENCE, EntityCategory::TypeReference),
];

struct Dialog<'a> {
    ctx: &'a Context,
    initial: Preferences,
    pages: Cell<[HWND; 2]>,
    failure: RefCell<Option<anyhow::Error>>,
}
impl Dialog<'_> {
    fn fail(&self, hwnd: HWND, error: anyhow::Error) {
        *self.failure.borrow_mut() = Some(error);
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
}
pub(in super::super) fn open(ctx: &Context) {
    super::super::status::report(ctx, show(ctx));
}

fn show(ctx: &Context) -> anyhow::Result<()> {
    let general = resources::dialog(IDD_SETTINGS_GENERAL)?;
    let search = resources::dialog(IDD_SETTINGS_SEARCH)?;
    let state = Dialog {
        ctx,
        initial: super::preferences(ctx),
        pages: Cell::new([ptr::null_mut(); 2]),
        failure: RefCell::new(None),
    };
    let mut pages: [PROPSHEETPAGEW; 2] = unsafe { std::mem::zeroed() };
    for (index, (template, title)) in [(general, IDS_GENERAL), (search, IDS_PANEL_SEARCH)]
        .into_iter()
        .enumerate()
    {
        let page = &mut pages[index];
        page.dwSize = size_of::<PROPSHEETPAGEW>() as u32;
        page.dwFlags = PSP_DLGINDIRECT | PSP_USETITLE | PSP_PREMATURE;
        page.hInstance = instance();
        page.Anonymous1.pResource = template.as_ptr().cast_mut().cast();
        page.pszTitle = resources::wide_text(title).as_ptr();
        page.pfnDlgProc = Some(page_proc);
        page.lParam = &state as *const Dialog<'_> as isize;
    }
    let mut header: PROPSHEETHEADERW_V2 = unsafe { std::mem::zeroed() };
    header.dwSize = size_of::<PROPSHEETHEADERW_V2>() as u32;
    header.dwFlags = PSH_PROPSHEETPAGE | PSH_NOAPPLYNOW | PSH_NOCONTEXTHELP | PSH_USECALLBACK;
    header.hwndParent = frame::hwnd(ctx);
    header.hInstance = instance();
    header.pszCaption = resources::wide_text(IDS_SETTINGS).as_ptr();
    header.nPages = pages.len() as u32;
    header.Anonymous3.ppsp = pages.as_mut_ptr();
    let baseline = vec![template_size(general)?, template_size(search)?];
    let data = &state as *const Dialog<'_> as usize;
    let result = property_sheet::show(&mut header, ctx.dpi(), baseline, data, Some(sheet_callback));
    result?;
    if let Some(error) = state.failure.into_inner() {
        return Err(error);
    }
    if state.initial.categories != super::categories(ctx) {
        catalog::search(ctx);
    }
    Ok(())
}

unsafe extern "system" fn sheet_callback(hwnd: HWND, message: u32, _l: isize, data: usize) -> i32 {
    unsafe {
        let Some(state) = (data as *const Dialog<'_>).as_ref() else {
            return 0;
        };
        if message == PSCB_INITIALIZED {
            set_text(GetDlgItem(hwnd, IDOK), &resources::text(IDS_SETTINGS_OK));
            set_text(
                GetDlgItem(hwnd, IDCANCEL),
                &resources::text(IDS_SETTINGS_CANCEL),
            );
            if set_subclass(hwnd, Some(sheet_proc), 2, data) == 0 {
                state.fail(hwnd, error("Subclass settings property sheet"));
            }
        }
        0
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, message: u32, _w: usize, l: isize) -> isize {
    unsafe {
        if message == WM_INITDIALOG {
            let page = &*(l as *const PROPSHEETPAGEW);
            let state = &*(page.lParam as *const Dialog<'_>);
            let language = GetDlgItem(hwnd, SETTINGS_LANGUAGE as i32);
            let mut pages = state.pages.get();
            if !language.is_null() {
                pages[0] = hwnd;
                for (index, option) in LANGUAGES.iter().enumerate() {
                    send(
                        language,
                        CB_ADDSTRING,
                        0,
                        resources::wide_text(option.label).as_ptr() as isize,
                    );
                    if option.value == state.initial.language {
                        send(language, CB_SETCURSEL, index, 0);
                    }
                }
                let theme = GetDlgItem(hwnd, SETTINGS_THEME as i32);
                let supported = crate::theme::supported();
                for (index, &(value, _, label)) in THEMES.iter().enumerate() {
                    if !supported && index != 0 {
                        continue;
                    }
                    send(
                        theme,
                        CB_ADDSTRING,
                        0,
                        resources::wide_text(label).as_ptr() as isize,
                    );
                    if value == state.initial.theme || !supported {
                        send(theme, CB_SETCURSEL, index, 0);
                    }
                }
                EnableWindow(theme, i32::from(supported));
            } else {
                pages[1] = hwnd;
                for &(id, category) in FILTERS {
                    send(
                        GetDlgItem(hwnd, id as i32),
                        BM_SETCHECK,
                        usize::from(state.initial.categories & category as u32 != 0),
                        0,
                    );
                }
            }
            state.pages.set(pages);
            return 1;
        }
        0
    }
}

unsafe extern "system" fn sheet_proc(
    hwnd: HWND,
    message: u32,
    w: usize,
    l: isize,
    _id: usize,
    data: usize,
) -> isize {
    unsafe {
        let state = &*(data as *const Dialog<'_>);
        match message {
            WM_COMMAND if w as u16 == IDOK as u16 => match commit(state) {
                Ok(()) => {}
                Err(error) => {
                    MessageBoxW(
                        hwnd,
                        wide(&format!("{error:#}")).as_ptr(),
                        resources::wide_text(IDS_SETTINGS).as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                    return 0;
                }
            },
            WM_NCDESTROY => {
                remove_subclass(hwnd, Some(sheet_proc), 2);
            }
            _ => {}
        }
        default_subclass(hwnd, message, w, l)
    }
}

fn commit(state: &Dialog<'_>) -> anyhow::Result<()> {
    let [general, search] = state.pages.get();
    anyhow::ensure!(
        !general.is_null() && !search.is_null(),
        "Settings pages are unavailable"
    );
    let index = send(
        unsafe { GetDlgItem(general, SETTINGS_LANGUAGE as i32) },
        CB_GETCURSEL,
        0,
        0,
    );
    let language = LANGUAGES
        .get(index as usize)
        .ok_or_else(|| anyhow::anyhow!("Invalid language selection"))?
        .value;
    let index = send(
        unsafe { GetDlgItem(general, SETTINGS_THEME as i32) },
        CB_GETCURSEL,
        0,
        0,
    );
    // Unsupported systems display System without destroying a preference saved
    // on a supported OS when the user commits unrelated settings.
    let theme = if crate::theme::supported() {
        THEMES
            .get(index as usize)
            .ok_or_else(|| anyhow::anyhow!("Invalid theme selection"))?
            .0
    } else {
        state.initial.theme
    };
    let mut categories = 0;
    for &(id, category) in FILTERS {
        if send(unsafe { GetDlgItem(search, id as i32) }, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
        {
            categories |= category as u32;
        }
    }
    let preferences = Preferences {
        language,
        categories,
        theme,
    };
    super::set_preferences(state.ctx, preferences)?;
    if state.initial.theme != theme {
        crate::theme::set_mode(theme as u32);
        send(frame::hwnd(state.ctx), WM_THEMECHANGED, 0, 0);
    }
    Ok(())
}

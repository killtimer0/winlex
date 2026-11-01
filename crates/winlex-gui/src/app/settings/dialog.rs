use super::{Preferences, catalog, dpi, read, write};
use crate::{
    native::*,
    resources::{self, *},
};
use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr,
};
use winlex_core::EntityCategory;

mod scale;

const PREPARE: u32 = WM_APP + 41;
const FILTERS: &[(u16, EntityCategory)] = &[
    (FILTER_TYPE, EntityCategory::Type),
    (FILTER_METHOD, EntityCategory::Method),
    (FILTER_CONSTANT, EntityCategory::Constant),
    (FILTER_FIELD, EntityCategory::Field),
    (FILTER_PROPERTY, EntityCategory::Property),
    (FILTER_EVENT, EntityCategory::Event),
    (FILTER_REFERENCE, EntityCategory::TypeReference),
];

struct Dialog {
    initial: Preferences,
    pages: Cell<[HWND; 2]>,
    scales: RefCell<Vec<scale::Scale>>,
    prepared: Cell<bool>,
    failure: RefCell<Option<anyhow::Error>>,
}
impl Dialog {
    fn fail(&self, hwnd: HWND, error: anyhow::Error) {
        *self.failure.borrow_mut() = Some(error);
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
}
thread_local! { static CURRENT: Cell<*const Dialog> = const { Cell::new(ptr::null()) }; }

pub(in super::super) fn open() {
    catalog::report(show());
}

fn show() -> anyhow::Result<()> {
    let general = resources::dialog(IDD_SETTINGS_GENERAL)?;
    let search = resources::dialog(IDD_SETTINGS_SEARCH)?;
    let state = Dialog {
        initial: read(|a| a.settings.preferences),
        pages: Cell::new([ptr::null_mut(); 2]),
        scales: RefCell::new(Vec::new()),
        prepared: Cell::new(false),
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
        page.lParam = &state as *const Dialog as isize;
    }
    let mut header: PROPSHEETHEADERW_V2 = unsafe { std::mem::zeroed() };
    header.dwSize = size_of::<PROPSHEETHEADERW_V2>() as u32;
    header.dwFlags = PSH_PROPSHEETPAGE | PSH_NOAPPLYNOW | PSH_NOCONTEXTHELP | PSH_USECALLBACK;
    header.hwndParent = read(|a| a.frame);
    header.hInstance = instance();
    header.pszCaption = resources::wide_text(IDS_SETTINGS).as_ptr();
    header.nPages = pages.len() as u32;
    header.Anonymous3.ppsp = pages.as_mut_ptr();
    header.pfnCallback = Some(sheet_callback);
    let hook = scale::CreationHook::new()?;
    let previous = CURRENT.with(|current| current.replace(&state));
    let result = unsafe { PropertySheetW(&mut header) };
    let failure = (result == -1).then(|| error("Create settings property sheet"));
    CURRENT.with(|current| current.set(previous));
    drop(hook);
    if let Some(error) = state.failure.into_inner() {
        return Err(error);
    }
    if let Some(error) = failure {
        return Err(error);
    }
    if state.initial.categories != read(|a| a.settings.preferences.categories) {
        catalog::search();
    }
    Ok(())
}

unsafe extern "system" fn sheet_callback(hwnd: HWND, message: u32, _l: isize) -> i32 {
    unsafe {
        CURRENT.with(|current| {
            let Some(state) = current.get().as_ref() else {
                return 0;
            };
            if message == PSCB_INITIALIZED {
                set_text(GetDlgItem(hwnd, IDOK), &resources::text(IDS_SETTINGS_OK));
                set_text(
                    GetDlgItem(hwnd, IDCANCEL),
                    &resources::text(IDS_SETTINGS_CANCEL),
                );
                if set_subclass(hwnd, Some(sheet_proc), 1, current.get() as usize) == 0 {
                    state.fail(hwnd, error("Subclass settings property sheet"));
                } else {
                    PostMessageW(hwnd, PREPARE, 0, 0);
                }
            }
            0
        })
    }
}

unsafe extern "system" fn page_proc(hwnd: HWND, message: u32, _w: usize, l: isize) -> isize {
    unsafe {
        if message == WM_INITDIALOG {
            let page = &*(l as *const PROPSHEETPAGEW);
            let state = &*(page.lParam as *const Dialog);
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
        if message == WM_NOTIFY && (*(l as *const NMHDR)).code == PSN_SETACTIVE {
            PostMessageW(GetParent(hwnd), PREPARE, 0, 0);
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
        let state = &*(data as *const Dialog);
        match message {
            PREPARE => {
                if let Err(error) = prepare(hwnd, state) {
                    state.fail(hwnd, error);
                }
                return 0;
            }
            // Native PMv2 property sheets can accumulate height loss across DPI changes:
            // page and tab adjustments are applied incrementally to the current window size.
            WM_DPICHANGED if dpi().mode == crate::dpi::Mode::Pmv1 && state.prepared.get() => {
                if let Err(error) = scale::update(&state.scales, (w & 0xffff) as u32) {
                    state.fail(hwnd, error);
                    return 0;
                }
                let _ = set_bounds(hwnd, *(l as *const RECT));
                align_pages(hwnd, state);
                RedrawWindow(
                    hwnd,
                    ptr::null(),
                    ptr::null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                );
                return 0;
            }
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
                remove_subclass(hwnd, Some(sheet_proc), 1);
            }
            _ => {}
        }
        default_subclass(hwnd, message, w, l)
    }
}

fn prepare(hwnd: HWND, state: &Dialog) -> anyhow::Result<()> {
    let initial = !state.prepared.replace(true);
    let manual = dpi().mode == crate::dpi::Mode::Pmv1;
    if manual {
        if initial {
            let mut scales = Vec::new();
            for page in state.pages.get() {
                anyhow::ensure!(!page.is_null(), "Settings page was not created");
                scales.push(scale::Scale::capture(page, &[])?);
            }
            scales.push(scale::Scale::capture(hwnd, &state.pages.get())?);
            *state.scales.borrow_mut() = scales;
        }
        scale::update(&state.scales, dpi().window(hwnd))?;
    }
    if initial {
        let owner = bounds(read(|a| a.frame), ptr::null_mut());
        let r = bounds(hwnd, ptr::null_mut());
        let size = if manual {
            state
                .scales
                .borrow()
                .last()
                .unwrap()
                .initial_size(dpi().window(hwnd))
        } else {
            SIZE {
                cx: r.right - r.left,
                cy: r.bottom - r.top,
            }
        };
        let (width, height) = (size.cx, size.cy);
        let mut x = owner.left + (owner.right - owner.left - width) / 2;
        let mut y = owner.top + (owner.bottom - owner.top - height) / 2;
        if let Some(monitor) = super::monitor(read(|a| a.frame)) {
            let work = monitor.rcWork;
            x = x.clamp(work.left, (work.right - width).max(work.left));
            y = y.clamp(work.top, (work.bottom - height).max(work.top));
        }
        set_bounds(hwnd, crate::layout::rect(x, y, width, height))?;
    }
    if manual {
        align_pages(hwnd, state);
    }
    Ok(())
}

fn align_pages(hwnd: HWND, state: &Dialog) {
    let tab = send(hwnd, PSM_GETTABCONTROL, 0, 0) as HWND;
    let mut area = client(tab);
    send(tab, TCM_ADJUSTRECT, 0, &mut area as *mut RECT as isize);
    unsafe {
        MapWindowPoints(tab, hwnd, (&mut area as *mut RECT).cast(), 2);
    }
    for page in state.pages.get() {
        crate::native::place(page, area);
    }
}

fn commit(state: &Dialog) -> anyhow::Result<()> {
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
    };
    write(|a| a.settings.set_preferences(&a.config, preferences))
}

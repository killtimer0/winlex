// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 ozone10. Rust integration: Winlex contributors.
// Port of win32-darkmodelib e8ec95b: DmlibHook.cpp (TaskDlgTheme) and
// DmlibSubclassWindow.cpp (DarkTaskDlgSubclass).
//! Scoped, process-wide comctl32 IAT hooks; native input/layout/painting remain native.
mod imports;

use crate::native::*;
use std::{
    cell::{Cell, RefCell},
    marker::PhantomData,
    ptr,
    rc::Rc,
    sync::Mutex,
};
use windows_sys::Win32::UI::Shell::GetWindowSubclass;
use windows_sys::core::HRESULT;

const TITLEBAR: usize = 0x574c5444;
const CONTENT: usize = 0x574c5445;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static PALETTE: RefCell<Option<Rc<Palette>>> = const { RefCell::new(None) };
    static PALETTE_LOADING: Cell<bool> = const { Cell::new(false) };
    static PALETTE_EPOCH: Cell<usize> = const { Cell::new(0) };
    static REFRESHING: Cell<bool> = const { Cell::new(false) };
}
static HOOKS: Mutex<imports::Hooks> = Mutex::new(imports::Hooks::new());

/// Hold across the actual TaskDialogIndirect call, including all its callbacks.
/// Pass the resolved comctl32 export's address, not a wrapper function.
/// The IAT patches are process-wide; synchronization only protects their lifetime.
/// Other threads forward unless they also hold a guard. This is not IAT isolation.
pub(crate) struct HookGuard {
    active: bool,
    _thread: PhantomData<Rc<()>>,
}

impl HookGuard {
    pub(crate) fn new(task_dialog_indirect: usize) -> Self {
        let active = super::supported()
            && HOOKS.lock().unwrap_or_else(|e| e.into_inner()).acquire(
                task_dialog_indirect,
                theme_color as *const () as usize,
                theme_background as *const () as usize,
            );
        if active {
            DEPTH.with(|depth| depth.set(depth.get() + 1));
        }
        Self {
            active,
            _thread: PhantomData,
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for HookGuard {
    fn drop(&mut self) {
        if self.active {
            DEPTH.with(|depth| depth.set(depth.get() - 1));
            HOOKS.lock().unwrap_or_else(|e| e.into_inner()).release();
            if DEPTH.with(Cell::get) == 0 {
                clear_palette();
            }
        }
    }
}

fn dark() -> bool {
    DEPTH.with(Cell::get) != 0 && super::is_dark() && !high_contrast()
}

struct Palette {
    theme: HTHEME,
    main: COLORREF,
    footer: COLORREF,
    instruction: COLORREF,
    text: COLORREF,
}

impl Palette {
    fn fallback() -> Self {
        Self {
            theme: 0,
            main: super::rgb(0x2c2c2c),
            footer: super::rgb(0x202020),
            instruction: super::rgb(0x99ebff),
            text: super::rgb(0xffffff),
        }
    }

    fn new() -> Self {
        let theme = if super::windows11() {
            unsafe { OpenThemeData(ptr::null_mut(), w!("DarkMode_Explorer::TaskDialog")) }
        } else {
            0
        };
        let mut palette = Self::fallback();
        palette.theme = theme;
        if theme != 0 {
            unsafe {
                query_color(theme, TDLG_PRIMARYPANEL, TMT_FILLCOLOR, &mut palette.main);
                query_color(theme, TDLG_FOOTNOTEPANE, TMT_FILLCOLOR, &mut palette.footer);
                query_color(
                    theme,
                    TDLG_MAININSTRUCTIONPANE,
                    TMT_TEXTCOLOR,
                    &mut palette.instruction,
                );
                query_color(theme, TDLG_PRIMARYPANEL, TMT_TEXTCOLOR, &mut palette.text);
            }
        }
        palette
    }
}
impl Drop for Palette {
    fn drop(&mut self) {
        if self.theme != 0 {
            unsafe {
                CloseThemeData(self.theme);
            }
        }
    }
}

unsafe fn query_color(theme: HTHEME, part: i32, property: u32, color: &mut COLORREF) {
    let mut candidate = *color;
    if unsafe { GetThemeColor(theme, part, 0, property as i32, &mut candidate) } >= 0 {
        *color = candidate;
    }
}
fn palette<T>(f: impl FnOnce(&Palette) -> T) -> T {
    let cached = PALETTE.with(|value| value.borrow().clone());
    let snapshot = cached.unwrap_or_else(|| {
        // OpenThemeData/GetThemeColor may reenter window procedures. Neither
        // palette creation nor use may retain a RefCell borrow across native APIs.
        if PALETTE_LOADING.with(|loading| loading.replace(true)) {
            return Rc::new(Palette::fallback());
        }
        struct Loading;
        impl Drop for Loading {
            fn drop(&mut self) {
                PALETTE_LOADING.with(|loading| loading.set(false));
            }
        }
        let _loading = Loading;
        let epoch = PALETTE_EPOCH.with(Cell::get);
        let snapshot = Rc::new(Palette::new());
        if PALETTE_EPOCH.with(Cell::get) == epoch {
            PALETTE.with(|value| *value.borrow_mut() = Some(snapshot.clone()));
        }
        snapshot
    });
    f(&snapshot)
}

fn clear_palette() {
    PALETTE_EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
    let previous = PALETTE.with(|value| value.borrow_mut().take());
    // Active native calls retain an Rc snapshot until they return. CloseThemeData
    // must also run outside the TLS borrow, since it can reenter native code.
    drop(previous);
}

unsafe extern "system" fn theme_color(
    theme: HTHEME,
    part: i32,
    state: i32,
    property: i32,
    color: *mut COLORREF,
) -> HRESULT {
    unsafe {
        let original = imports::original_color();
        if dark() && !color.is_null() && property == TMT_TEXTCOLOR as i32 {
            if part == TDLG_MAININSTRUCTIONPANE {
                *color = palette(|p| p.instruction);
                return S_OK;
            }
            if matches!(
                part,
                TDLG_CONTENTPANE
                    | TDLG_EXPANDOTEXT
                    | TDLG_VERIFICATIONTEXT
                    | TDLG_FOOTNOTEPANE
                    | TDLG_EXPANDEDFOOTERAREA
            ) {
                let result = palette(|p| {
                    if p.theme != 0 {
                        original(p.theme, part, state, property, color)
                    } else {
                        *color = p.text;
                        S_OK
                    }
                });
                if result >= 0 {
                    return S_OK;
                }
            }
        }
        original(theme, part, state, property, color)
    }
}

unsafe extern "system" fn theme_background(
    theme: HTHEME,
    dc: HDC,
    part: i32,
    state: i32,
    rect: *const RECT,
    options: *const DTBGOPTS,
) -> HRESULT {
    unsafe {
        if dark() && !options.is_null() && !rect.is_null() {
            let replacement = match part {
                TDLG_PRIMARYPANEL => Some((rect, palette(|p| p.main))),
                TDLG_SECONDARYPANEL | TDLG_FOOTNOTEPANE => {
                    Some((&(*options).rcClip as *const RECT, palette(|p| p.footer)))
                }
                _ => None,
            };
            if let Some((area, color)) = replacement {
                let old = SetDCBrushColor(dc, color);
                FillRect(dc, area, GetStockObject(DC_BRUSH));
                SetDCBrushColor(dc, old);
                return S_OK;
            }
        }
        imports::original_background()(theme, dc, part, state, rect, options)
    }
}

/// Shell integration is limited to native classes and titlebar attributes.
pub(crate) fn apply(hwnd: HWND) {
    install_titlebar(hwnd, true);
}

fn install_titlebar(hwnd: HWND, shell: bool) {
    if hwnd.is_null() || !super::supported() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    unsafe {
        set_subclass(hwnd, Some(titlebar_subclass), TITLEBAR, usize::from(shell));
    }
    refresh(hwnd, shell, false);
}

fn refresh(hwnd: HWND, shell: bool, policy: bool) {
    if REFRESHING.with(|refreshing| refreshing.replace(true)) {
        return;
    }
    struct Refreshing;
    impl Drop for Refreshing {
        fn drop(&mut self) {
            REFRESHING.with(|refreshing| refreshing.set(false));
        }
    }
    let _refreshing = Refreshing;
    if policy {
        super::api::refresh_policy();
        super::refresh_palette();
        clear_palette();
    }
    if shell {
        super::refresh_file_dialog(hwnd);
    } else {
        super::api::allow_window(hwnd, dark());
        let mut content = 0;
        unsafe {
            if DEPTH.with(Cell::get) != 0
                && GetWindowSubclass(hwnd, Some(content_subclass), CONTENT, &mut content) != 0
            {
                attach(hwnd);
                EnumChildWindows(hwnd, Some(attach_child), 0);
            }
        }
    }
    super::api::titlebar(hwnd, super::is_dark());
    unsafe {
        RedrawWindow(
            hwnd,
            ptr::null(),
            ptr::null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_FRAME,
        );
    }
}

/// Call on TDN_DIALOG_CONSTRUCTED, TDN_CREATED and TDN_NAVIGATED with a live guard.
/// Failed hook installation leaves the entire client native, not partially dark.
pub(crate) fn apply_native(hwnd: HWND) {
    if hwnd.is_null() || !super::supported() || unsafe { IsWindow(hwnd) } == 0 {
        return;
    }
    if DEPTH.with(Cell::get) != 0 {
        attach(hwnd);
    }
    install_titlebar(hwnd, false);
}

fn attach(hwnd: HWND) {
    unsafe {
        let mut existing = 0;
        if GetWindowSubclass(hwnd, Some(content_subclass), CONTENT, &mut existing) == 0 {
            let data = Box::into_raw(Box::new(Cell::new(true)));
            if set_subclass(hwnd, Some(content_subclass), CONTENT, data as usize) == 0 {
                drop(Box::from_raw(data));
            }
        } else {
            (*(existing as *const Cell<bool>)).set(true);
        }
    }
}

unsafe extern "system" fn attach_child(hwnd: HWND, _: isize) -> BOOL {
    match super::class_name(hwnd).as_str() {
        "directuihwnd" => {
            attach(hwnd);
            super::set_native_theme(hwnd, "DarkMode_Explorer", None);
        }
        "ctrlnotifysink" => attach(hwnd),
        // Native buttons retain hot/pressed/default/disabled/focus painting.
        "button" | "ccpushbutton" => {
            // TaskDialog uses CCPushButton on Win10, not necessarily WC_BUTTON.
            // Opt in before changing the theme; leave all state/input native.
            let enabled = dark();
            super::api::allow_window(hwnd, enabled);
            unsafe {
                SetWindowTheme(
                    hwnd,
                    if enabled {
                        w!("DarkMode_Explorer")
                    } else {
                        ptr::null()
                    },
                    ptr::null(),
                );
            }
        }
        _ => {}
    }
    1
}

unsafe extern "system" fn titlebar_subclass(
    hwnd: HWND,
    message: u32,
    w: usize,
    l: isize,
    _: usize,
    shell: usize,
) -> isize {
    unsafe {
        if message == WM_NCDESTROY {
            remove_subclass(hwnd, Some(titlebar_subclass), TITLEBAR);
        }
        let result = default_subclass(hwnd, message, w, l);
        if IsWindow(hwnd) != 0
            && !REFRESHING.with(Cell::get)
            && matches!(
                message,
                WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED
            )
        {
            refresh(hwnd, shell != 0, true);
        }
        result
    }
}

unsafe extern "system" fn content_subclass(
    hwnd: HWND,
    message: u32,
    w: usize,
    l: isize,
    _: usize,
    data: usize,
) -> isize {
    unsafe {
        if message == WM_NCDESTROY {
            remove_subclass(hwnd, Some(content_subclass), CONTENT);
            drop(Box::from_raw(data as *mut Cell<bool>));
            return default_subclass(hwnd, message, w, l);
        }
        if dark() {
            match message {
                WM_ERASEBKGND => {
                    let class = super::class_name(hwnd);
                    if class == "ctrlnotifysink" {
                        return default_subclass(hwnd, message, w, l);
                    }
                    if class == "directuihwnd" && (*(data as *const Cell<bool>)).replace(false) {
                        FillRect(
                            w as HDC,
                            &client(hwnd),
                            super::cached_brush(palette(|p| p.main)),
                        );
                    }
                    return 1;
                }
                WM_CTLCOLORDLG | WM_CTLCOLORSTATIC => {
                    let (text, bg) = palette(|p| (p.text, p.main));
                    SetTextColor(w as HDC, text);
                    SetBkColor(w as HDC, bg);
                    return super::cached_brush(bg) as isize;
                }
                WM_PRINTCLIENT => return 1,
                _ => {}
            }
        }
        default_subclass(hwnd, message, w, l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_notifications_reapply_native_opt_in_after_default_processing() {
        native_notifications_reapply_opt_in(true);
    }

    fn native_notifications_reapply_opt_in(shell: bool) {
        if !super::super::supported() {
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        type Allowed = unsafe extern "system" fn(HWND) -> bool;
        let Some(allowed) =
            (unsafe { GetProcAddress(GetModuleHandleW(w!("uxtheme.dll")), 137usize as *const u8) })
        else {
            eprintln!("Skipping native opt-in assertion: ordinal 137 unavailable");
            return;
        };
        let allowed: Allowed = unsafe { std::mem::transmute(allowed) };
        unsafe {
            let hwnd = test_window(
                w!("#32770"),
                ptr::null(),
                WS_POPUP,
                crate::layout::rect(0, 0, 100, 100),
            );
            let button = child(hwnd, w!("BUTTON"), &[0], BS_PUSHBUTTON as u32, 1).unwrap();
            let opposite = Cell::new(false);
            let observe = |hwnd, message, _, _| {
                if matches!(
                    message,
                    WM_SETTINGCHANGE | WM_SYSCOLORCHANGE | WM_THEMECHANGED
                ) && !REFRESHING.with(Cell::get)
                {
                    super::super::api::allow_window(hwnd, opposite.get());
                    super::super::api::allow_window(button, opposite.get());
                    return Some(73);
                }
                (message == WM_CTLCOLORSTATIC).then_some(73)
            };
            let callback = crate::test_support::observe_messages(hwnd, 1, &observe);
            if shell {
                apply(hwnd);
            } else {
                apply_native(hwnd);
            }
            for mode in [2, 1, 2] {
                super::super::set_mode(mode);
                let expected = super::super::is_dark();
                opposite.set(!expected);
                for message in [WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_THEMECHANGED] {
                    assert_eq!(send(hwnd, message, 0, 0), 73);
                    assert_eq!(
                        allowed(hwnd),
                        expected,
                        "root opt-in must follow native processing"
                    );
                    assert_eq!(
                        allowed(button),
                        expected,
                        "Shell button opt-in must be reapplied"
                    );
                }
                if shell {
                    assert_eq!(
                        send(hwnd, WM_CTLCOLORSTATIC, 0, button as isize),
                        73,
                        "Shell client painting must remain native/owner-controlled"
                    );
                }
            }
            remove_subclass(hwnd, callback, 1);
            DestroyWindow(hwnd);
            super::super::set_mode(0);
        }
    }

    // The IAT is process-wide even though callback policy is thread-local.
    #[test]
    #[ignore = "process-wide IAT hook test; run this test alone"]
    fn guarded_hooks_forward_in_light_mode_and_on_unrelated_threads() {
        if !super::super::supported() || high_contrast() {
            eprintln!("Skipping dark hook contract: unsupported or high contrast");
            return;
        }
        let _controls = super::super::tests::CommonControls::activate();
        let module = unsafe { LoadLibraryW(w!("comctl32.dll")) };
        assert!(!module.is_null());
        let address =
            unsafe { GetProcAddress(module, c"TaskDialogIndirect".as_ptr().cast()).unwrap() };
        super::super::set_mode(2);
        let guard = HookGuard::new(address as usize);
        unsafe { FreeLibrary(module) };
        assert!(guard.is_active());
        native_notifications_reapply_opt_in(false);
        super::super::set_mode(2);
        fn forwarded_color() {
            unsafe {
                let theme = OpenThemeData(ptr::null_mut(), w!("Button"));
                assert_ne!(theme, 0);
                let mut expected = 0x123456;
                let mut actual = expected;
                let status = imports::original_color()(
                    theme,
                    TDLG_MAININSTRUCTIONPANE,
                    0,
                    TMT_TEXTCOLOR as i32,
                    &mut expected,
                );
                assert_eq!(
                    theme_color(
                        theme,
                        TDLG_MAININSTRUCTIONPANE,
                        0,
                        TMT_TEXTCOLOR as i32,
                        &mut actual
                    ),
                    status
                );
                assert_eq!(
                    actual, expected,
                    "bypassed hook must preserve native output"
                );
                CloseThemeData(theme);
            }
        }
        unsafe {
            let mut actual = 0;
            assert_eq!(
                theme_color(
                    0,
                    TDLG_MAININSTRUCTIONPANE,
                    0,
                    TMT_TEXTCOLOR as i32,
                    &mut actual
                ),
                S_OK
            );
            assert_eq!(actual, palette(|p| p.instruction));
        }
        std::thread::spawn(|| {
            super::super::set_mode(2);
            assert!(!dark(), "another thread must not inherit a dialog scope");
            forwarded_color();
            super::super::set_mode(0);
        })
        .join()
        .unwrap();
        super::super::set_mode(1);
        forwarded_color();
        drop(guard);
        super::super::set_mode(0);
    }

    #[test]
    fn palette_invalidation_during_native_drawing_keeps_theme_alive() {
        if !super::super::supported() || high_contrast() {
            return;
        }
        clear_palette();
        let theme = unsafe { OpenThemeData(ptr::null_mut(), w!("Button")) };
        assert_ne!(theme, 0);
        let mut initial = Palette::fallback();
        initial.theme = theme;
        PALETTE.with(|value| *value.borrow_mut() = Some(Rc::new(initial)));
        palette(|snapshot| {
            clear_palette(); // models a synchronous WM_THEMECHANGED reentry
            let mut text = 0;
            assert!(
                unsafe {
                    GetThemeColor(
                        snapshot.theme,
                        BP_PUSHBUTTON,
                        PBS_NORMAL,
                        TMT_TEXTCOLOR as i32,
                        &mut text,
                    )
                } >= 0,
                "in-flight native drawing must retain its original theme handle"
            );
            // Reentrant paint must be able to populate the now-empty cache.
            palette(|_| {});
        });
        clear_palette();
    }
}

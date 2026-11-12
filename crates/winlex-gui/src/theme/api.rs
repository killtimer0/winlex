// SPDX-License-Identifier: MPL-2.0
// Ported from win32-darkmodelib DmlibWinApi.cpp / Darkmodelib.cpp
// Copyright (c) 2025-2026 ozone10. Rust integration: Winlex contributors.
//! Optional system theme capabilities, independent of the control painters.
use crate::{native::*, platform::WindowsVersion};
use std::{
    cell::{Cell, LazyCell},
    ptr,
};
use windows_sys::Win32::{Graphics::Dwm::DwmSetWindowAttribute, System::Registry::*};

type AllowWindow = unsafe extern "system" fn(HWND, bool) -> bool;
type AllowApp = unsafe extern "system" fn(bool) -> bool;
type PreferredApp = unsafe extern "system" fn(i32) -> i32;
type Flush = unsafe extern "system" fn();
type ContrastCache = unsafe extern "system" fn(i32) -> bool;
type Composition = unsafe extern "system" fn(HWND, *const CompositionData) -> BOOL;

#[repr(C)]
struct CompositionData {
    attribute: i32,
    data: *const BOOL,
    size: usize,
}

enum AppMode {
    Legacy(AllowApp),
    Preferred(PreferredApp),
}

struct Apis {
    version: WindowsVersion,
    allow_window: AllowWindow,
    app: AppMode,
    flush: Flush,
    refresh: Flush,
    contrast_cache: Option<ContrastCache>,
    composition: Option<Composition>,
    dark_theme: bool,
}

thread_local! {
    static APIS: LazyCell<Option<Apis>> = LazyCell::new(Apis::load);
    static REFRESHING_TITLEBAR: Cell<bool> = const { Cell::new(false) };
}

impl Apis {
    fn load() -> Option<Self> {
        let version = WindowsVersion::query().ok()?;
        if version < WindowsVersion::DARK_MODE {
            return None;
        }
        unsafe {
            let theme = GetModuleHandleW(w!("uxtheme.dll"));
            let ordinal = GetProcAddress(theme, 135usize as *const u8)?;
            let app = if version.build() < 18362 {
                AppMode::Legacy(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    AllowApp,
                >(ordinal))
            } else {
                AppMode::Preferred(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    PreferredApp,
                >(ordinal))
            };
            let mut revision = 0u32;
            let mut size = 4;
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
                w!("UBR"),
                RRF_RT_REG_DWORD,
                ptr::null_mut(),
                (&mut revision as *mut u32).cast(),
                &mut size,
            );
            Some(Self {
                version,
                allow_window: std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    AllowWindow,
                >(GetProcAddress(theme, 133usize as *const u8)?),
                app,
                flush: std::mem::transmute::<unsafe extern "system" fn() -> isize, Flush>(
                    GetProcAddress(theme, 136usize as *const u8)?,
                ),
                refresh: std::mem::transmute::<unsafe extern "system" fn() -> isize, Flush>(
                    GetProcAddress(theme, 104usize as *const u8)?,
                ),
                contrast_cache: GetProcAddress(theme, 106usize as *const u8).map(|p| {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, ContrastCache>(p)
                }),
                composition: GetProcAddress(
                    GetModuleHandleW(w!("user32.dll")),
                    c"SetWindowCompositionAttribute".as_ptr().cast(),
                )
                .map(|p| {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, Composition>(p)
                }),
                dark_theme: supports_dark_theme(version.build(), revision),
            })
        }
    }
}

fn supports_dark_theme(build: u32, revision: u32) -> bool {
    build > 26200 || matches!(build, 26100 | 26200) && revision >= 6899
}

pub(crate) fn supported() -> bool {
    APIS.with(|apis| apis.is_some())
}

pub(super) fn windows11() -> bool {
    APIS.with(|apis| {
        apis.as_ref()
            .is_some_and(|a| a.version >= WindowsVersion::WINDOWS_11)
    })
}

pub(super) fn native_dark_theme() -> bool {
    APIS.with(|apis| apis.as_ref().is_some_and(|a| a.dark_theme))
}

pub(super) fn refresh_policy() {
    APIS.with(|apis| unsafe {
        if let Some(a) = apis.as_ref() {
            (a.refresh)();
            if let Some(refresh) = a.contrast_cache {
                refresh(1);
            }
        }
    });
}

pub(super) fn set_app(dark: bool, contrast: bool) {
    APIS.with(|apis| unsafe {
        if let Some(a) = apis.as_ref() {
            match a.app {
                AppMode::Legacy(allow) => {
                    allow(dark && !contrast);
                }
                AppMode::Preferred(preferred) => {
                    preferred(if contrast {
                        0
                    } else if dark {
                        2
                    } else {
                        3
                    });
                }
            }
            (a.flush)();
        }
    });
}

pub(super) fn allow_window(hwnd: HWND, dark: bool) {
    APIS.with(|apis| unsafe {
        if let Some(a) = apis.as_ref() {
            (a.allow_window)(hwnd, dark);
        }
    });
}

pub(super) fn titlebar(hwnd: HWND, dark: bool) {
    APIS.with(|apis| unsafe {
        if let Some(a) = apis.as_ref() {
            let enabled = BOOL::from(dark);
            let applied = if a.version.build() >= 19041
                && DwmSetWindowAttribute(hwnd, 20, (&enabled as *const BOOL).cast(), 4) >= 0
            {
                true
            } else if a.version.build() >= 18362 {
                a.composition.is_some_and(|composition| {
                    composition(
                        hwnd,
                        &CompositionData {
                            attribute: 26,
                            data: &enabled,
                            size: 4,
                        },
                    ) != 0
                })
            } else {
                SetPropW(
                    hwnd,
                    w!("UseImmersiveDarkModeColors"),
                    enabled as usize as HANDLE,
                ) != 0
            };
            // Dmlib refreshes the cached Win10 non-client colors by toggling
            // activation, without changing the window's actual activation.
            // Hidden windows acquire their colors on first display instead.
            if applied
                && !windows11()
                && IsWindowVisible(hwnd) != 0
                && GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION == WS_CAPTION
            {
                refresh_titlebar(hwnd);
            }
        }
    });
}

unsafe fn refresh_titlebar(hwnd: HWND) {
    let Some(_guard) = super::Reentry::enter(&REFRESHING_TITLEBAR) else {
        return;
    };
    unsafe {
        let active = hwnd == GetActiveWindow() && hwnd == GetForegroundWindow();
        send(hwnd, WM_NCACTIVATE, usize::from(!active), 0);
        if IsWindow(hwnd) != 0 {
            send(hwnd, WM_NCACTIVATE, usize::from(active), 0);
        }
    }
}

pub(super) fn system_dark() -> bool {
    unsafe {
        let mut value = 1u32;
        let mut size = 4;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        ) == 0
            && size == 4
            && value == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonclient_refresh_preserves_activation_and_bounds_reentry() {
        unsafe {
            let hwnd = test_window(
                w!("STATIC"),
                ptr::null(),
                WS_POPUP,
                crate::layout::rect(0, 0, 100, 100),
            );
            let notifications = std::cell::RefCell::new(Vec::<usize>::new());
            let observe = |hwnd, message, w, _| {
                if message == WM_NCACTIVATE {
                    {
                        let mut notifications = notifications.borrow_mut();
                        notifications.push(w);
                        assert!(
                            notifications.len() <= 2,
                            "non-client refresh must not recurse"
                        );
                    }
                    refresh_titlebar(hwnd);
                }
                None
            };
            let active = GetActiveWindow();
            let foreground = GetForegroundWindow();
            let expected = hwnd == active && hwnd == foreground;
            let callback = crate::test_support::observe_messages(hwnd, 1, &observe);
            refresh_titlebar(hwnd);
            assert_eq!(
                *notifications.borrow(),
                [usize::from(!expected), usize::from(expected)]
            );
            assert_eq!(GetActiveWindow(), active);
            assert_eq!(GetForegroundWindow(), foreground);
            remove_subclass(hwnd, callback, 1);
            DestroyWindow(hwnd);
        }
    }

    #[test]
    fn dark_theme_requires_servicing_revision_on_both_supported_branches() {
        for build in [26100, 26200] {
            assert!(!supports_dark_theme(build, 6898));
            assert!(supports_dark_theme(build, 6899));
        }
        assert!(!supports_dark_theme(26099, u32::MAX));
        assert!(supports_dark_theme(26201, 0));
    }
}

// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 ozone10. Rust integration: Winlex contributors.
use crate::native::*;
use std::mem;

// YY-Thunks 1.2.3 covers DrawThemeTextEx/GetThemeTransitionDuration, but not
// buffered animation. Resolve these Vista exports without changing the XP
// loader contract. A missing export disables transitions, not custom painting.
#[derive(Clone, Copy)]
pub(super) struct AnimationApis {
    pub(super) init: unsafe extern "system" fn() -> i32,
    pub(super) uninit: unsafe extern "system" fn() -> i32,
    pub(super) stop: unsafe extern "system" fn(HWND) -> i32,
    pub(super) render: unsafe extern "system" fn(HWND, HDC) -> i32,
    pub(super) begin: unsafe extern "system" fn(
        HWND,
        HDC,
        *const RECT,
        BP_BUFFERFORMAT,
        *const BP_PAINTPARAMS,
        *const BP_ANIMATIONPARAMS,
        *mut HDC,
        *mut HDC,
    ) -> isize,
    pub(super) end: unsafe extern "system" fn(isize, i32) -> i32,
}

impl AnimationApis {
    fn load() -> Option<Self> {
        let module = unsafe { GetModuleHandleW(w!("uxtheme.dll")) };
        Self::load_with(|name| unsafe { GetProcAddress(module, name.as_ptr()) })
    }

    fn load_with(
        mut resolve: impl FnMut(&[u8]) -> Option<unsafe extern "system" fn() -> isize>,
    ) -> Option<Self> {
        unsafe {
            macro_rules! resolve {
                ($name:literal, $ty:ty) => {
                    mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(resolve(
                        concat!($name, "\0").as_bytes(),
                    )?)
                };
            }
            Some(Self {
                init: resolve!("BufferedPaintInit", unsafe extern "system" fn() -> i32),
                uninit: resolve!("BufferedPaintUnInit", unsafe extern "system" fn() -> i32),
                stop: resolve!(
                    "BufferedPaintStopAllAnimations",
                    unsafe extern "system" fn(HWND) -> i32
                ),
                render: resolve!(
                    "BufferedPaintRenderAnimation",
                    unsafe extern "system" fn(HWND, HDC) -> i32
                ),
                begin: resolve!(
                    "BeginBufferedAnimation",
                    unsafe extern "system" fn(
                        HWND,
                        HDC,
                        *const RECT,
                        BP_BUFFERFORMAT,
                        *const BP_PAINTPARAMS,
                        *const BP_ANIMATIONPARAMS,
                        *mut HDC,
                        *mut HDC,
                    ) -> isize
                ),
                end: resolve!(
                    "EndBufferedAnimation",
                    unsafe extern "system" fn(isize, i32) -> i32
                ),
            })
        }
    }
}

thread_local! {
    static ANIMATION: Option<AnimationApis> = AnimationApis::load();
}

pub(super) fn animation<T: Default>(action: impl FnOnce(&AnimationApis) -> T) -> T {
    ANIMATION.with(|api| api.as_ref().map(action).unwrap_or_default())
}

pub(super) fn stop_animation(hwnd: HWND) {
    animation(|api| unsafe { (api.stop)(hwnd) });
}

pub(super) fn client_animation_enabled() -> bool {
    let mut enabled = 0i32;
    unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&mut enabled as *mut i32).cast(),
            0,
        ) != 0
            && enabled != 0
    }
}

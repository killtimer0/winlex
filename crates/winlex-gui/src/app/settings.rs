//! Window placement and settings commands, using the single process configuration root.
use super::{catalog, dpi, read, write};
use crate::{
    config::Config,
    layout,
    native::*,
    resources::{self, *},
};
use std::ptr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Language {
    #[default]
    Auto,
    English,
    Chinese,
}
impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en-US",
            Self::Chinese => "zh-CN",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub maximized: bool,
}
pub(super) struct Settings {
    pub language: Language,
    pub window: Option<Window>,
}
impl Settings {
    pub fn load(config: &Config) -> Self {
        let key = config.key.as_ref();
        let language = match key.and_then(|k| k.text("Language")).as_deref() {
            Some("en-US") => Language::English,
            Some("zh-CN") => Language::Chinese,
            _ => Language::Auto,
        };
        let window = key.and_then(|k| k.bytes("Window")).and_then(|bytes| {
            let bytes: [u8; 24] = bytes.try_into().ok()?;
            let v: Vec<_> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| i32::from_le_bytes(*b))
                .collect();
            if !(USER_DEFAULT_SCREEN_DPI as i32..=USER_DEFAULT_SCREEN_DPI as i32 * 10)
                .contains(&v[4])
                || !(1..=100000).contains(&v[2])
                || !(1..=100000).contains(&v[3])
                || !(0..=1).contains(&v[5])
                || v[0].checked_add(v[2]).is_none()
                || v[1].checked_add(v[3]).is_none()
            {
                return None;
            }
            Some(Window {
                x: v[0],
                y: v[1],
                width: v[2],
                height: v[3],
                dpi: v[4] as u32,
                maximized: v[5] == 1,
            })
        });
        Self { language, window }
    }
    fn set_language(&mut self, config: &Config, language: Language) -> Result<(), String> {
        config.write_key()?.set_text("Language", language.name())?;
        self.language = language;
        Ok(())
    }
}

fn monitor(hwnd: HWND) -> Option<MONITORINFO> {
    unsafe {
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0)
            .then_some(info)
    }
}

pub(super) fn restore_window(hwnd: HWND) {
    let value = dpi().window(hwnd);
    let saved = read(|a| a.settings.window);
    let mut bounds = crate::native::bounds(hwnd, ptr::null_mut());
    let (mut width, mut height) = match saved {
        Some(w) => {
            bounds.left = w.x;
            bounds.top = w.y;
            (
                unsafe { MulDiv(w.width, value as i32, w.dpi as i32) },
                unsafe { MulDiv(w.height, value as i32, w.dpi as i32) },
            )
        }
        None => (layout::scale(1000, value), layout::scale(680, value)),
    };
    width = width.max(layout::scale(480, value));
    height = height.max(layout::scale(300, value));
    if let Some(info) = monitor(hwnd) {
        let work = info.rcWork;
        width = width.min(work.right - work.left);
        height = height.min(work.bottom - work.top);
        bounds.left = bounds.left.clamp(work.left, work.right - width);
        bounds.top = bounds.top.clamp(work.top, work.bottom - height);
    }
    unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            bounds.left,
            bounds.top,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub(super) fn save_window(hwnd: HWND) -> Result<(), String> {
    unsafe {
        let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
        placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
        if GetWindowPlacement(hwnd, &mut placement) == 0 {
            return Err(error("Read window placement"));
        }
        let mut r = placement.rcNormalPosition;
        // WINDOWPLACEMENT uses workspace coordinates for ordinary top-level windows.
        if let Some(info) = monitor(hwnd) {
            let x = info.rcWork.left - info.rcMonitor.left;
            let y = info.rcWork.top - info.rcMonitor.top;
            r.left += x;
            r.right += x;
            r.top += y;
            r.bottom += y;
        }
        let window = Window {
            x: r.left,
            y: r.top,
            width: r.right - r.left,
            height: r.bottom - r.top,
            dpi: dpi().window(hwnd),
            maximized: IsZoomed(hwnd) != 0
                || (IsIconic(hwnd) != 0 && placement.flags & WPF_RESTORETOMAXIMIZED != 0),
        };
        read(|a| {
            let key = a.config.write_key()?;
            let bytes: Vec<_> = [
                window.x,
                window.y,
                window.width,
                window.height,
                window.dpi as i32,
                i32::from(window.maximized),
            ]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect();
            key.set_bytes("Window", &bytes)?;
            a.sidebar.save(&key)
        })
    }
}

pub(super) fn update_language_menu() {
    let (hwnd, language) = read(|a| (a.frame, a.settings.language));
    let selected = match language {
        Language::Auto => LANGUAGE_AUTO,
        Language::English => LANGUAGE_ENGLISH,
        Language::Chinese => LANGUAGE_CHINESE,
    };
    unsafe {
        CheckMenuRadioItem(
            GetMenu(hwnd),
            LANGUAGE_AUTO as u32,
            LANGUAGE_CHINESE as u32,
            selected as u32,
            MF_BYCOMMAND,
        );
    }
}
pub(super) fn language(id: u16) {
    let language = match id {
        LANGUAGE_ENGLISH => Language::English,
        LANGUAGE_CHINESE => Language::Chinese,
        _ => Language::Auto,
    };
    match write(|a| a.settings.set_language(&a.config, language)) {
        Ok(()) => {
            update_language_menu();
            catalog::set_status(resources::text(IDS_LANGUAGE_RESTART));
        }
        Err(error) => catalog::set_status(error),
    }
}

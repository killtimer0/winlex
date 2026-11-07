//! Window placement and settings commands, using the single process configuration root.
use super::catalog;
use crate::app::context::Context;
use crate::{config::Config, layout, native::*, resources::Language};
use std::ptr;
use winlex_core::{Architecture, EntityCategory};

mod dialog;
pub(super) use dialog::open;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub(super) enum Theme {
    #[default]
    System = 0,
    Light = 1,
    Dark = 2,
}
const THEMES: &[(Theme, &str, u16)] = &[
    (Theme::System, "system", crate::resources::IDS_THEME_SYSTEM),
    (Theme::Light, "light", crate::resources::IDS_THEME_LIGHT),
    (Theme::Dark, "dark", crate::resources::IDS_THEME_DARK),
];
impl Theme {
    fn name(self) -> &'static str {
        THEMES[self as usize].1
    }
    fn from_name(name: &str) -> Option<Self> {
        THEMES
            .iter()
            .find(|(_, key, _)| *key == name)
            .map(|&(value, _, _)| value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Preferences {
    pub language: Language,
    pub categories: u32,
    pub theme: Theme,
}
impl Preferences {
    fn load(config: &Config) -> Self {
        let key = config.key.as_ref();
        Self {
            language: key
                .and_then(|k| k.text("Language"))
                .and_then(|value| Language::from_name(&value))
                .unwrap_or(Language::Auto),
            categories: key
                .and_then(|k| k.number("SearchCategories"))
                .filter(|value| value & !EntityCategory::ALL == 0)
                .unwrap_or(EntityCategory::DEFAULT),
            theme: key
                .and_then(|k| k.text("Theme"))
                .and_then(|value| Theme::from_name(&value))
                .unwrap_or_default(),
        }
    }
    fn save(self, config: &Config) -> anyhow::Result<()> {
        let key = config.write_key()?;
        key.set_text("Language", self.language.name())?;
        key.set_number("SearchCategories", self.categories)?;
        key.set_text("Theme", self.theme.name())
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn theme_options_round_trip() {
        for &(value, _, _) in THEMES {
            assert_eq!(Theme::from_name(value.name()), Some(value));
        }
        assert_eq!(Theme::from_name("unknown"), None);
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
    pub preferences: Preferences,
    pub window: Option<Window>,
    pub preferred_architecture: Option<Architecture>,
}
impl Settings {
    pub fn load(config: &Config) -> Self {
        let key = config.key.as_ref();
        let preferences = Preferences::load(config);
        let preferred_architecture = key
            .and_then(|k| k.text("PreferredArchitecture"))
            .and_then(|value| value.parse::<Architecture>().ok());
        let window = key.and_then(|k| k.bytes("Window")).and_then(|bytes| {
            let bytes: [u8; 24] = bytes.try_into().ok()?;
            let v = bytes
                .as_chunks::<4>()
                .0
                .first_chunk::<6>()?
                .map(i32::from_le_bytes);
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
        Self {
            preferences,
            window,
            preferred_architecture,
        }
    }
    pub fn set_preferred_architecture(
        &mut self,
        config: &Config,
        architecture: Architecture,
    ) -> anyhow::Result<()> {
        if self.preferred_architecture != Some(architecture) {
            config
                .write_key()?
                .set_text("PreferredArchitecture", architecture.name())?;
            self.preferred_architecture = Some(architecture);
        }
        Ok(())
    }
    pub fn set_preferences(
        &mut self,
        config: &Config,
        preferences: Preferences,
    ) -> anyhow::Result<()> {
        if self.preferences != preferences {
            preferences.save(config)?;
            self.preferences = preferences;
        }
        Ok(())
    }
}

pub(super) fn preferences(ctx: &Context) -> Preferences {
    ctx.with_settings(|settings| settings.preferences)
}
pub(super) fn categories(ctx: &Context) -> u32 {
    preferences(ctx).categories
}
pub(super) fn preferred_architecture(ctx: &Context) -> Option<Architecture> {
    ctx.with_settings(|settings| settings.preferred_architecture)
}
pub(super) fn set_preferred_architecture(
    ctx: &Context,
    architecture: Architecture,
) -> anyhow::Result<()> {
    ctx.with_settings_and_config(|settings, config| {
        settings.set_preferred_architecture(config, architecture)
    })
}
pub(super) fn apply_preferences(ctx: &Context, preferences: Preferences) -> anyhow::Result<()> {
    let previous = self::preferences(ctx);
    ctx.with_settings_and_config(|settings, config| settings.set_preferences(config, preferences))?;
    if previous.theme != preferences.theme {
        crate::theme::set_mode(preferences.theme as u32);
        send(ctx.frame(), WM_THEMECHANGED, 0, 0);
    }
    if previous.categories != preferences.categories {
        catalog::search(ctx);
    }
    Ok(())
}
pub(super) fn restore_window(ctx: &Context, hwnd: HWND) {
    let value = ctx.dpi().window(hwnd);
    let saved = ctx.with_settings(|settings| settings.window);
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
        None => (
            layout::scale(layout::DEFAULT_WIDTH, value),
            layout::scale(layout::DEFAULT_HEIGHT, value),
        ),
    };
    width = width.max(layout::scale(layout::MIN_WIDTH, value));
    height = height.max(layout::scale(layout::MIN_HEIGHT, value));
    if let Some(info) = monitor(hwnd) {
        let work = info.rcWork;
        width = width.min(work.right - work.left);
        height = height.min(work.bottom - work.top);
        bounds.left = bounds.left.clamp(work.left, work.right - width);
        bounds.top = bounds.top.clamp(work.top, work.bottom - height);
    }
    let _ = set_bounds(hwnd, layout::rect(bounds.left, bounds.top, width, height));
}

pub(super) fn save_window(ctx: &Context, hwnd: HWND) -> anyhow::Result<()> {
    unsafe {
        let placement = window_placement(hwnd, "Read window placement")?;
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
        let dpi = ctx.dpi().window(hwnd);
        let maximized = IsZoomed(hwnd) != 0
            || (IsIconic(hwnd) != 0 && placement.flags & WPF_RESTORETOMAXIMIZED != 0);
        let key = ctx.with_config(|config| config.write_key())?;
        let bytes: Vec<_> = [
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            dpi as i32,
            i32::from(maximized),
        ]
        .into_iter()
        .flat_map(i32::to_le_bytes)
        .collect();
        key.set_bytes("Window", &bytes)?;
        ctx.with_sidebar(|sidebar| sidebar.save(&key))
    }
}

//! Window placement and settings commands, using the single process configuration root.
use super::{catalog, dpi, read, write};
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
    fn parse(value: &str) -> Option<Self> {
        let (language, categories) = value.split_once(':')?;
        let (categories, theme) = match categories.split_once(':') {
            Some((categories, theme)) => (categories, Theme::from_name(theme)?),
            None => (categories, Theme::System),
        };
        let categories: u32 = categories.parse().ok()?;
        (categories & !EntityCategory::ALL == 0).then_some(Self {
            language: Language::from_name(language)?,
            categories,
            theme,
        })
    }
    fn serialize(self) -> String {
        let mut value = format!("{}:{}", self.language.name(), self.categories);
        if self.theme != Theme::System {
            value.push(':');
            value.push_str(self.theme.name());
        }
        value
    }
    fn save(self, config: &Config) -> anyhow::Result<()> {
        config
            .write_key()?
            .set_text("Preferences", &self.serialize())
    }
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn preferences_validate_language_and_category_bits() {
        for language in crate::resources::LANGUAGES {
            for categories in [0, EntityCategory::DEFAULT, EntityCategory::ALL] {
                let value = format!("{}:{categories}", language.value.name());
                assert_eq!(
                    Preferences::parse(&value),
                    Some(Preferences {
                        language: language.value,
                        categories,
                        theme: Theme::System,
                    })
                );
            }
        }
        for value in [
            "auto",
            "auto:-1",
            "auto:128",
            "other:7",
            "en-US:1:2",
            "auto:7:",
            "auto:7:other",
            "auto:7:dark:extra",
        ] {
            assert_eq!(Preferences::parse(value), None);
        }
    }

    #[test]
    fn theme_options_and_preferences_round_trip() {
        for (index, &(value, _, label)) in THEMES.iter().enumerate() {
            assert_eq!(value as u32, index as u32);
            assert_eq!(Theme::from_name(value.name()), Some(value));
            assert_eq!(THEMES.iter().filter(|other| other.2 == label).count(), 1);
            for language in crate::resources::LANGUAGES {
                for categories in [0, EntityCategory::DEFAULT, EntityCategory::ALL] {
                    let preferences = Preferences {
                        language: language.value,
                        categories,
                        theme: value,
                    };
                    assert_eq!(
                        Preferences::parse(&preferences.serialize()),
                        Some(preferences)
                    );
                }
            }
        }
        assert_eq!(Theme::from_name("unknown"), None);
    }

    #[test]
    fn system_theme_preserves_legacy_preferences_format() {
        let preferences = Preferences {
            language: Language::English,
            categories: EntityCategory::DEFAULT,
            theme: Theme::System,
        };
        assert_eq!(preferences.serialize(), "en-US:7");
        assert_eq!(Preferences::parse("en-US:7:system"), Some(preferences));
        for (theme, expected) in [
            (Theme::Light, "en-US:7:light"),
            (Theme::Dark, "en-US:7:dark"),
        ] {
            assert_eq!(
                Preferences {
                    theme,
                    ..preferences
                }
                .serialize(),
                expected
            );
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
    pub preferences: Preferences,
    pub window: Option<Window>,
    pub preferred_architecture: Option<Architecture>,
}
impl Settings {
    pub fn load(config: &Config) -> Self {
        let key = config.key.as_ref();
        let preferences = key
            .and_then(|k| k.text("Preferences"))
            .and_then(|value| Preferences::parse(&value))
            .unwrap_or(Preferences {
                language: Language::Auto,
                categories: EntityCategory::DEFAULT,
                theme: Theme::System,
            });
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
    let _ = set_bounds(hwnd, layout::rect(bounds.left, bounds.top, width, height));
}

pub(super) fn save_window(hwnd: HWND) -> anyhow::Result<()> {
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
        let dpi = dpi().window(hwnd);
        let maximized = IsZoomed(hwnd) != 0
            || (IsIconic(hwnd) != 0 && placement.flags & WPF_RESTORETOMAXIMIZED != 0);
        read(|a| {
            let key = a.config.write_key()?;
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
            a.sidebar.save(&key)
        })
    }
}

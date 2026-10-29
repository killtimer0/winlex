//! Native RC resources with explicit language lookup, including on XP.
use crate::native::*;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/resource_ids.rs"));

const ENGLISH: u16 = 0x0409;
const CHINESE: u16 = 0x0804;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Auto,
    English,
    Chinese,
}
struct LanguageOption {
    value: Language,
    name: &'static str,
    command: u16,
    langid: Option<u16>,
}
const LANGUAGES: &[LanguageOption] = &[
    LanguageOption {
        value: Language::Auto,
        name: "auto",
        command: LANGUAGE_AUTO,
        langid: None,
    },
    LanguageOption {
        value: Language::English,
        name: "en-US",
        command: LANGUAGE_ENGLISH,
        langid: Some(ENGLISH),
    },
    LanguageOption {
        value: Language::Chinese,
        name: "zh-CN",
        command: LANGUAGE_CHINESE,
        langid: Some(CHINESE),
    },
];
impl Language {
    fn option(self) -> &'static LanguageOption {
        LANGUAGES
            .iter()
            .find(|option| option.value == self)
            .expect("registered language")
    }
    pub fn name(self) -> &'static str {
        self.option().name
    }
    pub fn command(self) -> u16 {
        self.option().command
    }
    pub fn from_name(name: &str) -> Option<Self> {
        LANGUAGES
            .iter()
            .find(|option| option.name == name)
            .map(|option| option.value)
    }
    pub fn from_command(command: u16) -> Option<Self> {
        LANGUAGES
            .iter()
            .find(|option| option.command == command)
            .map(|option| option.value)
    }
    fn langid(self) -> u16 {
        self.option().langid.unwrap_or_else(|| {
            match unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } {
                0x0804 | 0x1004 => CHINESE,
                _ => ENGLISH,
            }
        })
    }
}
// RT_STRING is misplaced under KernelStreaming in windows-sys 0.59.
const RT_STRING: *const u16 = 6usize as *const u16;
static LANGUAGE: OnceLock<u16> = OnceLock::new();
static STRINGS: OnceLock<std::collections::HashMap<u16, Vec<u16>>> = OnceLock::new();

pub fn initialize(language: Language) -> anyhow::Result<()> {
    let selected = language.langid();
    let _ = LANGUAGE.set(selected);
    // Validate before entering window callbacks, where Rust unwinding is forbidden.
    let strings = STRING_IDS
        .iter()
        .map(|&id| load_text(id).map(|text| (id, text)))
        .collect::<Result<_, _>>()?;
    let _ = STRINGS.set(strings);
    data(RT_MENU, IDR_MAIN_MENU)?;
    Ok(())
}

fn data(kind: *const u16, id: u16) -> anyhow::Result<&'static [u8]> {
    unsafe {
        let module = instance();
        let language = *LANGUAGE.get().expect("initialized resource language");
        let mut resource = FindResourceExW(module, kind, id as usize as *const u16, language);
        if resource.is_null() && language != ENGLISH {
            resource = FindResourceExW(module, kind, id as usize as *const u16, ENGLISH);
        }
        if resource.is_null() {
            return Err(error("Find GUI resource").context(format!("Missing GUI resource {id}")));
        }
        let size = SizeofResource(module, resource) as usize;
        if size == 0 {
            return Err(error("Read GUI resource size").context(format!("GUI resource {id}")));
        }
        let loaded = LoadResource(module, resource);
        if loaded.is_null() {
            return Err(error("Load GUI resource").context(format!("GUI resource {id}")));
        }
        let pointer = LockResource(loaded);
        if pointer.is_null() {
            anyhow::bail!("Cannot load GUI resource {id}");
        }
        // Resources belong to the executable module, which stays loaded for the process lifetime.
        Ok(std::slice::from_raw_parts(pointer.cast(), size))
    }
}
fn load_text(id: u16) -> anyhow::Result<Vec<u16>> {
    let bytes = data(RT_STRING, id / 16 + 1)?;
    let mut words = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]));
    for index in 0..=id % 16 {
        let count = words
            .next()
            .ok_or_else(|| anyhow::anyhow!("Invalid string resource {id}"))?
            as usize;
        let mut value: Vec<_> = words.by_ref().take(count).collect();
        if value.len() != count {
            anyhow::bail!("Truncated string resource {id}");
        }
        if index == id % 16 {
            if value.is_empty() {
                anyhow::bail!("Missing string resource {id}");
            }
            if char::decode_utf16(value.iter().copied()).any(|c| c.is_err()) {
                anyhow::bail!("Invalid UTF-16 resource {id}");
            }
            value.push(0);
            return Ok(value);
        }
    }
    unreachable!()
}
pub fn text(id: u16) -> String {
    let value = wide_text(id);
    String::from_utf16(&value[..value.len() - 1]).expect("validated resource")
}
pub fn wide_text(id: u16) -> &'static [u16] {
    &STRINGS.get().expect("initialized resources")[&id]
}
pub fn menu() -> anyhow::Result<HMENU> {
    let bytes = data(RT_MENU, IDR_MAIN_MENU)?;
    let menu = unsafe { LoadMenuIndirectW(bytes.as_ptr().cast()) };
    if menu.is_null() {
        Err(error("Load GUI menu"))
    } else {
        Ok(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_names_and_commands_round_trip() {
        for option in LANGUAGES {
            assert_eq!(Language::from_name(option.value.name()), Some(option.value));
            assert_eq!(
                Language::from_command(option.value.command()),
                Some(option.value)
            );
        }
        assert_eq!(Language::from_name("unknown"), None);
        assert_eq!(Language::from_command(0), None);
        assert_eq!(Language::English.langid(), 0x0409);
        assert_eq!(Language::Chinese.langid(), 0x0804);
    }
}

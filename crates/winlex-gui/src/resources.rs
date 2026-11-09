//! Native RC resources with explicit language lookup, including on XP.
use crate::native::*;
use std::sync::OnceLock;
use windows_sys::Win32::System::Diagnostics::Debug::{
    FORMAT_MESSAGE_ALLOCATE_BUFFER, FORMAT_MESSAGE_ARGUMENT_ARRAY, FORMAT_MESSAGE_FROM_STRING,
    FormatMessageW,
};

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
pub struct LanguageOption {
    pub value: Language,
    name: &'static str,
    pub label: u16,
    langid: Option<u16>,
}
pub const LANGUAGES: &[LanguageOption] = &[
    LanguageOption {
        value: Language::Auto,
        name: "auto",
        label: IDS_LANGUAGE_AUTO,
        langid: None,
    },
    LanguageOption {
        value: Language::English,
        name: "en-US",
        label: IDS_LANGUAGE_ENGLISH,
        langid: Some(ENGLISH),
    },
    LanguageOption {
        value: Language::Chinese,
        name: "zh-CN",
        label: IDS_LANGUAGE_CHINESE,
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
    pub fn from_name(name: &str) -> Option<Self> {
        LANGUAGES
            .iter()
            .find(|option| option.name == name)
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

pub fn binary(id: u16) -> anyhow::Result<&'static [u8]> {
    data(10usize as *const u16, id)
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
            resource = FindResourceExW(module, kind, id as usize as *const u16, 0);
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
    let mut words = bytes.as_chunks::<2>().0;
    for index in 0..=id % 16 {
        let (count, rest) = words
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("Invalid string resource {id}"))?;
        let (value, rest) = rest
            .split_at_checked(u16::from_le_bytes(*count) as usize)
            .ok_or_else(|| anyhow::anyhow!("Truncated string resource {id}"))?;
        words = rest;
        if index == id % 16 {
            if value.is_empty() {
                anyhow::bail!("Missing string resource {id}");
            }
            let mut value: Vec<_> = value.iter().map(|&word| u16::from_le_bytes(word)).collect();
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
pub fn format(id: u16, arguments: &[&str]) -> String {
    format_message(wide_text(id), arguments).unwrap_or_else(|error| format!("{error:#}"))
}
fn format_message(template: &[u16], arguments: &[&str]) -> anyhow::Result<String> {
    owned_handle! { struct MessageBuffer(HLOCAL); LocalFree, null = std::ptr::null_mut() }
    let arguments: Vec<_> = arguments.iter().map(|value| wide(value)).collect();
    let pointers: Vec<_> = arguments
        .iter()
        .map(|value| value.as_ptr().cast::<i8>())
        .collect();
    let mut buffer = MessageBuffer(std::ptr::null_mut());
    let count = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_ALLOCATE_BUFFER
                | FORMAT_MESSAGE_FROM_STRING
                | FORMAT_MESSAGE_ARGUMENT_ARRAY,
            template.as_ptr().cast(),
            0,
            0,
            (&mut buffer.0 as *mut HLOCAL).cast(),
            0,
            pointers.as_ptr(),
        )
    };
    if count == 0 {
        return Err(error("Format GUI resource"));
    }
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(buffer.0.cast(), count as usize)
    }))
}
pub fn interface_langid() -> u16 {
    *LANGUAGE.get().expect("initialized resource language")
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
pub fn dialog(id: u16) -> anyhow::Result<&'static [u8]> {
    data(5usize as *const u16, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_inserts_preserve_order_unicode_and_literal_argument_text() {
        assert_eq!(
            format_message(
                &wide("%2!s! / %1!s! / %2!s!%n100%%"),
                &["中文 🦀", "%1!s!\n{value}"]
            )
            .unwrap(),
            "%1!s!\n{value} / 中文 🦀 / %1!s!\n{value}\r\n100%"
        );
    }

    #[test]
    fn language_names_round_trip() {
        for option in LANGUAGES {
            assert_eq!(Language::from_name(option.value.name()), Some(option.value));
        }
        assert_eq!(Language::from_name("unknown"), None);
        assert_eq!(Language::English.langid(), 0x0409);
        assert_eq!(Language::Chinese.langid(), 0x0804);
    }
}

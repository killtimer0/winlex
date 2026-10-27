//! Native RC resources with explicit language lookup, including on XP.
use crate::native::*;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/resource_ids.rs"));

const ENGLISH: u16 = 0x0409;
const CHINESE: u16 = 0x0804;
// RT_STRING is misplaced under KernelStreaming in windows-sys 0.59.
const RT_STRING: *const u16 = 6usize as *const u16;
static LANGUAGE: OnceLock<u16> = OnceLock::new();
static STRINGS: OnceLock<std::collections::HashMap<u16, Vec<u16>>> = OnceLock::new();

pub fn initialize(language: &str) -> Result<(), String> {
    let selected = match language {
        "en-US" => ENGLISH,
        "zh-CN" => CHINESE,
        "auto" => match unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() } {
            0x0804 | 0x1004 => CHINESE,
            _ => ENGLISH,
        },
        _ => return Err(format!("Unknown UI language: {language}")),
    };
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

fn data(kind: *const u16, id: u16) -> Result<&'static [u8], String> {
    unsafe {
        let module = instance();
        let language = *LANGUAGE.get().unwrap_or(&ENGLISH);
        let mut resource = FindResourceExW(module, kind, id as usize as *const u16, language);
        if resource.is_null() && language != ENGLISH {
            resource = FindResourceExW(module, kind, id as usize as *const u16, ENGLISH);
        }
        if resource.is_null() {
            return Err(format!("Missing GUI resource {id}"));
        }
        let size = SizeofResource(module, resource) as usize;
        let loaded = LoadResource(module, resource);
        let pointer = LockResource(loaded);
        if pointer.is_null() || size == 0 {
            return Err(format!("Cannot load GUI resource {id}"));
        }
        // Resources belong to the executable module, which stays loaded for the process lifetime.
        Ok(std::slice::from_raw_parts(pointer.cast(), size))
    }
}
fn load_text(id: u16) -> Result<Vec<u16>, String> {
    let bytes = data(RT_STRING, id / 16 + 1)?;
    let mut words = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]));
    for index in 0..=id % 16 {
        let count = words
            .next()
            .ok_or_else(|| format!("Invalid string resource {id}"))? as usize;
        let mut value: Vec<_> = words.by_ref().take(count).collect();
        if value.len() != count {
            return Err(format!("Truncated string resource {id}"));
        }
        if index == id % 16 {
            if value.is_empty() {
                return Err(format!("Missing string resource {id}"));
            }
            if char::decode_utf16(value.iter().copied()).any(|c| c.is_err()) {
                return Err(format!("Invalid UTF-16 resource {id}"));
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
pub fn menu() -> Result<HMENU, String> {
    let bytes = data(RT_MENU, IDR_MAIN_MENU)?;
    let menu = unsafe { LoadMenuIndirectW(bytes.as_ptr().cast()) };
    if menu.is_null() {
        Err(error("Load GUI menu"))
    } else {
        Ok(menu)
    }
}

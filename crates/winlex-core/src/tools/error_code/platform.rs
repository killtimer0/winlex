use super::*;
use std::{os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, GetLastError, HMODULE, SetLastError},
    Globalization::{
        CP_ACP, EnumUILanguagesW, GetSystemDefaultLangID, GetSystemDefaultUILanguage,
        GetThreadLocale, GetUserDefaultLangID, GetUserDefaultUILanguage, MUI_LANGUAGE_ID,
        MultiByteToWideChar,
    },
    Storage::FileSystem::GetShortPathNameW,
    System::{
        Diagnostics::Debug::{
            FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS, FormatMessageW,
        },
        LibraryLoader::{
            EnumResourceLanguagesW, EnumResourceNamesW, FindResourceExW, LOAD_LIBRARY_AS_DATAFILE,
            LoadLibraryExW, LoadResource, LockResource, SizeofResource,
        },
    },
};

fn failure(operation: &str) -> Failure {
    let code = unsafe { GetLastError() };
    Failure {
        operation: operation.into(),
        os_code: Some(code),
        detail: std::io::Error::from_raw_os_error(code as i32).to_string(),
    }
}

struct Module(HMODULE);
impl Module {
    fn open(path: &Path) -> Result<Self, Failure> {
        // Absolute paths also prevent bare names from entering DLL search order.
        let path = std::path::absolute(path).map_err(|e| Failure {
            operation: "module_path".into(),
            os_code: e.raw_os_error().map(|c| c as u32),
            detail: e.to_string(),
        })?;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if !wide.starts_with(&[92, 92, 63, 92]) {
            if wide.starts_with(&[92, 92]) {
                wide.splice(..2, [92, 92, 63, 92, 85, 78, 67, 92]);
            } else {
                wide.splice(..0, [92, 92, 63, 92]);
            }
        }
        wide.push(0);
        let mut handle =
            unsafe { LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE) };
        if handle.is_null() && wide.len() > 260 {
            let original = failure("LoadLibraryExW");
            // XP's resource loader can reject extended-length paths. An existing
            // filesystem alias addresses the same file without copying or loading code.
            let mut short = vec![0u16; 32768];
            let length =
                unsafe { GetShortPathNameW(wide.as_ptr(), short.as_mut_ptr(), short.len() as u32) }
                    as usize;
            if length == 0 || length >= short.len() || length + 1 >= wide.len() {
                return Err(original);
            }
            handle = unsafe {
                LoadLibraryExW(short.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE)
            };
        }
        if handle.is_null() {
            Err(failure("LoadLibraryExW"))
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}

pub fn lookup(code: u32, language: u16, source: &Source) -> Result<(String, Option<u16>), Failure> {
    let module = match source {
        Source::System => None,
        Source::NtDll => Some(Module::open(&system_path("ntdll.dll")?)?),
        Source::Crt => return crt_message(code, language),
        Source::Module(path) => Some(Module::open(path)?),
    };
    if let Some(module) = module {
        return module_message(&module, code, language);
    }
    let flags = FORMAT_MESSAGE_IGNORE_INSERTS | FORMAT_MESSAGE_FROM_SYSTEM;
    // FormatMessage limits messages to 64 KiB. Avoid an allocated-buffer ownership API.
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        FormatMessageW(
            flags,
            ptr::null(),
            code,
            language as u32,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            ptr::null(),
        )
    };
    if length == 0 {
        return Err(failure("FormatMessageW"));
    }
    let mut text = String::from_utf16_lossy(&buffer[..length as usize]);
    text.truncate(text.trim_end().len());
    Ok((text, (language != 0).then_some(language)))
}

fn system_path(name: &str) -> Result<PathBuf, Failure> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut directory = vec![0u16; 32768];
    let length =
        unsafe { GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32) } as usize;
    if length == 0 || length >= directory.len() {
        return Err(failure("GetSystemDirectoryW"));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&directory[..length])).join(name))
}

fn crt_message(code: u32, language: u16) -> Result<(String, Option<u16>), Failure> {
    if !matches!(language, 0 | 0x409) {
        return Err(resource_error(1815));
    }
    let code = i32::try_from(code).map_err(|_| resource_error(317))?;
    unsafe extern "C" {
        fn _wcserror(code: i32) -> *mut u16;
    }
    let copy = |value| -> Result<String, Failure> {
        let text = unsafe { _wcserror(value) };
        if text.is_null() {
            return Err(Failure {
                operation: "_wcserror".into(),
                os_code: None,
                detail: "CRT could not allocate the error message buffer".into(),
            });
        }
        let mut length = 0;
        unsafe {
            while *text.add(length) != 0 {
                length += 1;
            }
            Ok(String::from_utf16_lossy(std::slice::from_raw_parts(
                text, length,
            )))
        }
    };
    // Copy the thread-local buffer before another call overwrites it. Comparing
    // the runtime's unknown sentinel also rejects holes in its errno table.
    let unknown = copy(-1)?;
    let text = copy(code)?;
    if text == unknown {
        return Err(resource_error(317));
    }
    Ok((text, Some(0x409)))
}

fn resource_error(code: u32) -> Failure {
    Failure {
        operation: "message_resource".into(),
        os_code: Some(code),
        detail: std::io::Error::from_raw_os_error(code as i32).to_string(),
    }
}

enum ResourceName {
    Id(usize),
    Text(Vec<u16>),
}

impl ResourceName {
    fn as_ptr(&self) -> *const u16 {
        match self {
            Self::Id(id) => *id as *const u16,
            Self::Text(text) => text.as_ptr(),
        }
    }
}

struct Table {
    name: ResourceName,
    languages: Vec<u16>,
}

fn eligible_languages(requested: u16, preferred: &[u16], available: &[u16]) -> Vec<u16> {
    if requested != 0 {
        return Vec::from_iter(available.contains(&requested).then_some(requested));
    }
    let mut order = Vec::new();
    for &language in preferred.iter().chain(available) {
        if available.contains(&language) && !order.contains(&language) {
            order.push(language);
        }
    }
    order
}

fn module_message(
    module: &Module,
    code: u32,
    language: u16,
) -> Result<(String, Option<u16>), Failure> {
    let tables = resource_tables(module)?;
    if tables.is_empty() {
        return Err(resource_error(1813));
    }
    let available: Vec<u16> = tables
        .iter()
        .flat_map(|table| table.languages.iter().copied())
        .collect();
    let preferred = [
        0,
        unsafe { GetThreadLocale() } as u16,
        unsafe { GetUserDefaultLangID() },
        unsafe { GetSystemDefaultLangID() },
        0x409,
    ];
    let order = eligible_languages(language, &preferred, &available);
    for &selected in &order {
        for table in &tables {
            // FindResourceExW itself can fall back, including for language zero.
            // Enumerated availability makes each request exact before calling it.
            if !table.languages.contains(&selected) {
                continue;
            }
            let resource = unsafe {
                FindResourceExW(
                    module.0,
                    11usize as *const u16,
                    table.name.as_ptr(),
                    selected,
                )
            };
            if resource.is_null() {
                return Err(failure("FindResourceExW"));
            }
            let size = unsafe { SizeofResource(module.0, resource) } as usize;
            if size == 0 || size > isize::MAX as usize {
                return Err(resource_error(13));
            }
            let loaded = unsafe { LoadResource(module.0, resource) };
            if loaded.is_null() {
                return Err(failure("LoadResource"));
            }
            let data = unsafe { LockResource(loaded) };
            if data.is_null() {
                return Err(failure("LockResource"));
            }
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size) };
            if let Some((text, unicode)) = message_entry(bytes, code)? {
                let mut text = decode_entry(text, unicode)?;
                text.truncate(text.trim_end().len());
                return Ok((text, Some(selected)));
            }
        }
    }
    Err(resource_error(if order.is_empty() { 1815 } else { 317 }))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Failure> {
    let value = bytes
        .get(offset..offset.checked_add(4).ok_or_else(|| resource_error(13))?)
        .ok_or_else(|| resource_error(13))?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

// MESSAGE_RESOURCE_DATA contains byte offsets and variable-sized entries, not pointers.
fn message_entry(bytes: &[u8], code: u32) -> Result<Option<(&[u8], bool)>, Failure> {
    let count = read_u32(bytes, 0)? as usize;
    let header_end = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(4))
        .filter(|&end| end <= bytes.len())
        .ok_or_else(|| resource_error(13))?;
    for block in 0..count {
        let base = 4 + block * 12;
        let low = read_u32(bytes, base)?;
        let high = read_u32(bytes, base + 4)?;
        let mut offset = read_u32(bytes, base + 8)? as usize;
        if low > high || offset < header_end || offset >= bytes.len() {
            return Err(resource_error(13));
        }
        if !(low..=high).contains(&code) {
            continue;
        }
        for id in low..=code {
            let header = read_u32(bytes, offset)?;
            let length = (header & 0xffff) as usize;
            let flags = (header >> 16) as u16;
            let end = offset
                .checked_add(length)
                .filter(|&end| end <= bytes.len())
                .ok_or_else(|| resource_error(13))?;
            if length < 4 || flags > 1 || (flags == 1 && !(length - 4).is_multiple_of(2)) {
                return Err(resource_error(13));
            }
            if id == code {
                return Ok(Some((&bytes[offset + 4..end], flags == 1)));
            }
            offset = end;
        }
    }
    Ok(None)
}

fn decode_entry(bytes: &[u8], unicode: bool) -> Result<String, Failure> {
    if unicode {
        let words = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|&word| word != 0);
        return Ok(char::decode_utf16(words)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect());
    }
    let bytes = &bytes[..bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len())];
    if bytes.is_empty() {
        return Ok(String::new());
    }
    let length = unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            ptr::null_mut(),
            0,
        )
    };
    if length == 0 {
        return Err(failure("MultiByteToWideChar"));
    }
    let mut words = vec![0u16; length as usize];
    if unsafe {
        MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            words.as_mut_ptr(),
            length,
        )
    } == 0
    {
        return Err(failure("MultiByteToWideChar"));
    }
    Ok(String::from_utf16_lossy(&words))
}

unsafe extern "system" fn language_callback(
    _: HMODULE,
    _: *const u16,
    _: *const u16,
    language: u16,
    context: isize,
) -> i32 {
    unsafe { &mut *(context as *mut Vec<u16>) }.push(language);
    1
}

struct Enumeration {
    tables: Vec<Table>,
    failure: Option<Failure>,
}
unsafe extern "system" fn name_callback(
    module: HMODULE,
    kind: *const u16,
    name: *const u16,
    context: isize,
) -> i32 {
    let state = unsafe { &mut *(context as *mut Enumeration) };
    let mut languages = Vec::new();
    let ok = unsafe {
        EnumResourceLanguagesW(
            module,
            kind,
            name,
            Some(language_callback),
            &mut languages as *mut Vec<u16> as isize,
        )
    };
    if ok == 0 {
        state.failure = Some(failure("EnumResourceLanguagesW"));
        return 0;
    }
    let name = if name as usize <= 0xffff {
        ResourceName::Id(name as usize)
    } else {
        let mut text = Vec::new();
        let mut cursor = name;
        loop {
            let word = unsafe { *cursor };
            text.push(word);
            if word == 0 {
                break;
            }
            cursor = unsafe { cursor.add(1) };
        }
        ResourceName::Text(text)
    };
    state.tables.push(Table { name, languages });
    1
}

fn resource_tables(module: &Module) -> Result<Vec<Table>, Failure> {
    let mut state = Enumeration {
        tables: vec![],
        failure: None,
    };
    unsafe {
        SetLastError(0);
    }
    let ok = unsafe {
        EnumResourceNamesW(
            module.0,
            11usize as *const u16,
            Some(name_callback),
            &mut state as *mut Enumeration as isize,
        )
    };
    if let Some(error) = state.failure {
        return Err(error);
    }
    if ok == 0 {
        let error = failure("EnumResourceNamesW");
        if !matches!(error.os_code, Some(1812..=1814)) {
            return Err(error);
        }
    }
    Ok(state.tables)
}

pub fn module_languages(path: &Path) -> Result<Vec<u16>, Failure> {
    let module = Module::open(path)?;
    let mut languages: Vec<u16> = resource_tables(&module)?
        .into_iter()
        .flat_map(|table| table.languages)
        .collect();
    languages.sort_unstable();
    languages.dedup();
    Ok(languages)
}

unsafe extern "system" fn ui_language_callback(text: *const u16, context: isize) -> i32 {
    let mut length = 0;
    while length < 8 && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    if let Ok(id) = u16::from_str_radix(&text, 16) {
        unsafe { &mut *(context as *mut Vec<u16>) }.push(id);
    }
    1
}

pub fn system_languages() -> Vec<u16> {
    let mut languages = vec![0, 0x409, unsafe { GetUserDefaultUILanguage() }, unsafe {
        GetSystemDefaultUILanguage()
    }];
    // Failure on older systems simply leaves the reliable defaults available.
    unsafe {
        EnumUILanguagesW(
            Some(ui_language_callback),
            MUI_LANGUAGE_ID,
            &mut languages as *mut Vec<u16> as isize,
        );
    }
    languages.sort_unstable();
    languages.dedup();
    languages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(low: u32, entries: &[(&str, bool)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [1, low, low + (entries.len() as u32 - 1), 16] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for &(text, unicode) in entries {
            let mut payload = if unicode {
                text.encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>()
            } else {
                text.bytes().chain(Some(0)).collect::<Vec<_>>()
            };
            while (payload.len() + 4) % 4 != 0 {
                payload.push(0);
            }
            bytes.extend_from_slice(&((payload.len() + 4) as u16).to_le_bytes());
            bytes.extend_from_slice(&u16::from(unicode).to_le_bytes());
            bytes.extend_from_slice(&payload);
        }
        bytes
    }

    #[test]
    fn raw_templates_keep_legacy_and_numbered_inserts_and_paragraphs() {
        let template = "\"0x%08lx\" referenced memory at \"0x%08lx\".\r\n\r\n%1!s! %n %0 tail";
        for unicode in [false, true] {
            let bytes = table(0xc0000005, &[("previous", unicode), (template, unicode)]);
            let (payload, encoding) = message_entry(&bytes, 0xc0000006).unwrap().unwrap();
            assert_eq!(encoding, unicode);
            assert_eq!(decode_entry(payload, encoding).unwrap(), template);
            assert!(message_entry(&bytes, 5).unwrap().is_none());
        }
        let bytes = table(u32::MAX, &[("中文 %1", true)]);
        let (payload, unicode) = message_entry(&bytes, u32::MAX).unwrap().unwrap();
        assert_eq!(decode_entry(payload, unicode).unwrap(), "中文 %1");
    }

    #[test]
    fn malformed_message_resources_fail_without_out_of_bounds_reads() {
        let valid = table(5, &[("first", true), ("second", true)]);
        for end in 0..valid.len() {
            assert!(message_entry(&valid[..end], 6).is_err(), "prefix {end}");
        }
        for (offset, value) in [
            (0, u32::MAX),
            (4, 7),
            (12, 0),
            (12, u32::MAX),
            (16, 0),
            (16, 3),
            (16, 0x0001_0005),
            (16, 0x0002_0008),
            (16, 0xffff),
        ] {
            let mut bytes = valid.clone();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                message_entry(&bytes, 6).is_err(),
                "offset {offset}, value {value}"
            );
        }
    }

    #[test]
    fn language_selection_is_exact_or_uses_available_documented_order() {
        let preferred = [0, 0x411, 0x804, 0x407, 0x409];
        let available = [0x40c, 0x409, 0x804, 0x804, 0];
        assert_eq!(
            eligible_languages(0, &preferred, &available),
            [0, 0x804, 0x409, 0x40c]
        );
        assert!(eligible_languages(0x411, &preferred, &available).is_empty());
        assert_eq!(eligible_languages(0x409, &preferred, &available), [0x409]);
        assert!(eligible_languages(0, &preferred, &[]).is_empty());
    }
}

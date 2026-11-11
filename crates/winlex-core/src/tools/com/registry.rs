//! Live registry source backed by `RegOpenKeyExW` and `RegQueryValueExW`.
use super::{ReadError, Root, Source, View};
use std::{io, ptr};
use windows_sys::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS,
    ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
};
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};

/// Reads the registry of the current system.
#[derive(Clone, Copy, Debug, Default)]
pub struct Live;

impl Source for Live {
    fn exists(&self, root: Root, view: View, path: &str) -> Result<bool, ReadError> {
        Ok(open(root, view, path)?.is_some())
    }
    fn value(
        &self,
        root: Root,
        view: View,
        path: &str,
        name: &str,
    ) -> Result<Option<String>, ReadError> {
        let Some(key) = open(root, view, path)? else {
            return Ok(None);
        };
        let name = wide(name);
        let mut kind = 0u32;
        let mut size = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                ptr::null_mut(),
                &mut kind,
                ptr::null_mut(),
                &mut size,
            )
        };
        match status {
            ERROR_SUCCESS => {}
            ERROR_FILE_NOT_FOUND => return Ok(None),
            ERROR_INVALID_PARAMETER => return Err(ReadError::UnsupportedView),
            status => return Err(status_error("RegQueryValueExW", status)),
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Ok(None);
        }
        for _ in 0..3 {
            let mut buffer = vec![0u8; size as usize + 2];
            let mut read = size;
            let status = unsafe {
                RegQueryValueExW(
                    key.0,
                    name.as_ptr(),
                    ptr::null_mut(),
                    &mut kind,
                    buffer.as_mut_ptr(),
                    &mut read,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    buffer.truncate(read as usize);
                    let text = decode(&buffer);
                    return Ok(Some(if kind == REG_EXPAND_SZ {
                        expand(&text)
                    } else {
                        text
                    }));
                }
                ERROR_MORE_DATA => size = read,
                ERROR_INVALID_PARAMETER => return Err(ReadError::UnsupportedView),
                status => return Err(status_error("RegQueryValueExW", status)),
            }
        }
        Err(ReadError::Other(
            "RegQueryValueExW: the value changed repeatedly".into(),
        ))
    }
    fn subkeys(&self, root: Root, view: View, path: &str) -> Result<Vec<String>, ReadError> {
        let Some(key) = open(root, view, path)? else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        for index in 0.. {
            let mut length = 256u32;
            loop {
                let mut buffer = vec![0u16; length as usize];
                let status = unsafe {
                    RegEnumKeyExW(
                        key.0,
                        index,
                        buffer.as_mut_ptr(),
                        &mut length,
                        ptr::null(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                        ptr::null_mut(),
                    )
                };
                match status {
                    ERROR_SUCCESS => {
                        names.push(String::from_utf16_lossy(&buffer[..length as usize]));
                        break;
                    }
                    ERROR_MORE_DATA if length <= 16384 => length = length.saturating_mul(2),
                    ERROR_NO_MORE_ITEMS => return Ok(names),
                    ERROR_INVALID_PARAMETER => return Err(ReadError::UnsupportedView),
                    status => return Err(status_error("RegEnumKeyExW", status)),
                }
            }
        }
        Ok(names)
    }
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open(root: Root, view: View, path: &str) -> Result<Option<Key>, ReadError> {
    let base = match root {
        Root::Machine => HKEY_LOCAL_MACHINE,
        Root::User => HKEY_CURRENT_USER,
    };
    let desired = KEY_READ
        | match view {
            View::X86 => KEY_WOW64_32KEY,
            View::X64 => KEY_WOW64_64KEY,
            View::Native => 0,
        };
    let path = wide(path);
    let mut key: HKEY = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(base, path.as_ptr(), 0, desired, &mut key) };
    match status {
        ERROR_SUCCESS => Ok(Some(Key(key))),
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
        ERROR_INVALID_PARAMETER => Err(ReadError::UnsupportedView),
        status => Err(status_error("RegOpenKeyExW", status)),
    }
}

fn status_error(context: &str, status: u32) -> ReadError {
    ReadError::Other(format!(
        "{context}: {}",
        io::Error::from_raw_os_error(status as i32)
    ))
}

fn decode(buffer: &[u8]) -> String {
    let (pairs, _) = buffer.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

fn expand(text: &str) -> String {
    let source = wide(text);
    let needed = unsafe { ExpandEnvironmentStringsW(source.as_ptr(), ptr::null_mut(), 0) };
    if needed == 0 {
        return text.to_owned();
    }
    let mut buffer = vec![0u16; needed as usize];
    let written =
        unsafe { ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), needed) };
    if written == 0 || written > needed {
        return text.to_owned();
    }
    String::from_utf16_lossy(&buffer[..written as usize - 1])
}

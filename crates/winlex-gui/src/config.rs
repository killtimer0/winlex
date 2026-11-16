//! Typed registry access beneath one application-selected HKCU root.
use crate::native::*;
use anyhow::Context;
use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::System::Registry::*;

const MAX_VALUE: u32 = 1024 * 1024;
pub struct Config {
    root: Vec<u16>,
    version: u32,
    warning: Option<Warning>,
    paths_warning: Option<Warning>,
    pub paths: Option<Vec<PathBuf>>,
    pub key: Option<Key>,
}
owned_handle! { pub struct Key(HKEY); RegCloseKey }
enum Warning {
    UnsupportedVersion,
    Read(u32),
    ValueRead(&'static str, u32),
}
fn registry_error(operation: &str, status: u32) -> anyhow::Error {
    anyhow::Error::new(std::io::Error::from_raw_os_error(status as i32))
        .context(operation.to_owned())
}
impl Config {
    pub fn load(version: u32) -> anyhow::Result<Self> {
        let root =
            std::env::var_os("WINLEX_CONFIG_ROOT").unwrap_or_else(|| "Software\\Winlex".into());
        Self::from_root(&root, version)
    }
    fn from_root(root: &OsStr, version: u32) -> anyhow::Result<Self> {
        let mut root: Vec<_> = root.encode_wide().collect();
        if root.contains(&0) || root.split(|&c| c == b'\\' as u16).any(|s| s.is_empty()) {
            anyhow::bail!("WINLEX_CONFIG_ROOT must be a nonempty HKCU-relative registry path");
        }
        root.push(0);
        let mut config = Self {
            root,
            version,
            warning: None,
            paths_warning: None,
            paths: None,
            key: None,
        };
        let mut handle = ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                config.root.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut handle,
            )
        };
        match status {
            ERROR_FILE_NOT_FOUND => config.paths = Some(Vec::new()),
            ERROR_SUCCESS => {
                let key = Key(handle);
                let stored_version = key.value("Version", REG_DWORD).and_then(|bytes| {
                    bytes
                        .map(|bytes| {
                            bytes
                                .try_into()
                                .map(u32::from_le_bytes)
                                .map_err(|_| ERROR_INVALID_DATA)
                        })
                        .transpose()
                });
                match stored_version {
                    Ok(Some(stored)) if stored != version => {
                        config.warning = Some(Warning::UnsupportedVersion)
                    }
                    Ok(_) => {}
                    Err(status) => config.warning = Some(Warning::ValueRead("Version", status)),
                }
                if config.warning.is_none() {
                    match key.value("AutoLoad", REG_MULTI_SZ).and_then(|bytes| {
                        bytes.map_or_else(
                            || Ok(Vec::new()),
                            |bytes| decode_paths(&bytes).ok_or(ERROR_INVALID_DATA),
                        )
                    }) {
                        Ok(paths) => config.paths = Some(paths),
                        Err(status) => {
                            config.paths_warning = Some(Warning::ValueRead("AutoLoad", status))
                        }
                    }
                    config.key = Some(key);
                }
            }
            _ => config.warning = Some(Warning::Read(status)),
        }
        Ok(config)
    }
    pub fn warning(&self) -> Option<anyhow::Error> {
        self.warning
            .as_ref()
            .or(self.paths_warning.as_ref())
            .map(Self::warning_error)
    }
    fn warning_error(warning: &Warning) -> anyhow::Error {
        match warning {
            Warning::UnsupportedVersion => anyhow::anyhow!(
                "Unsupported configuration version; defaults used without overwriting settings"
            ),
            Warning::Read(status) => registry_error("Read configuration", *status),
            Warning::ValueRead(name, status) => {
                registry_error(&format!("Read configuration value {name}"), *status)
            }
        }
    }
    pub fn write_paths(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        if let Some(warning) = &self.paths_warning {
            return Err(Self::warning_error(warning));
        }
        self.write_key()?.set_paths("AutoLoad", paths)
    }
    pub fn write_key(&self) -> anyhow::Result<Key> {
        if let Some(warning) = &self.warning {
            return Err(Self::warning_error(warning));
        }
        let mut handle = ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                self.root.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(registry_error("Open configuration for writing", status));
        }
        let key = Key(handle);
        key.set_number("Version", self.version)?;
        Ok(key)
    }
}
impl Key {
    fn value(&self, name: &str, kind: u32) -> Result<Option<Vec<u8>>, u32> {
        unsafe {
            let name = wide(name);
            let query = |data, size: &mut u32, actual: &mut u32| {
                RegQueryValueExW(self.0, name.as_ptr(), ptr::null(), actual, data, size)
            };
            for _ in 0..8 {
                let mut size = 0;
                let mut actual = 0;
                let status = query(ptr::null_mut(), &mut size, &mut actual);
                match status {
                    ERROR_FILE_NOT_FOUND => return Ok(None),
                    ERROR_SUCCESS => {}
                    _ => return Err(status),
                }
                if actual != kind || size > MAX_VALUE {
                    return Err(ERROR_INVALID_DATA);
                }
                let mut bytes = vec![0; (size as usize).max(1)];
                let status = query(bytes.as_mut_ptr(), &mut size, &mut actual);
                match status {
                    ERROR_MORE_DATA => continue,
                    ERROR_FILE_NOT_FOUND => return Ok(None),
                    ERROR_SUCCESS => {}
                    _ => return Err(status),
                }
                if actual != kind || size as usize > bytes.len() {
                    return Err(ERROR_INVALID_DATA);
                }
                bytes.truncate(size as usize);
                return Ok(Some(bytes));
            }
            Err(ERROR_MORE_DATA)
        }
    }
    fn set(&self, name: &str, kind: u32, bytes: &[u8]) -> anyhow::Result<()> {
        let size = u32::try_from(bytes.len()).context("Configuration value too large")?;
        let status =
            unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, kind, bytes.as_ptr(), size) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(registry_error("Write configuration", status))
        }
    }
    pub fn number(&self, name: &str) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.value(name, REG_DWORD)
                .ok()
                .flatten()?
                .try_into()
                .ok()?,
        ))
    }
    pub fn set_number(&self, name: &str, value: u32) -> anyhow::Result<()> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }
    pub fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.value(name, REG_BINARY).ok().flatten()
    }
    pub fn set_bytes(&self, name: &str, value: &[u8]) -> anyhow::Result<()> {
        self.set(name, REG_BINARY, value)
    }
    pub fn text(&self, name: &str) -> Option<String> {
        let words = words(&self.value(name, REG_SZ).ok().flatten()?)?;
        if words.last() != Some(&0) {
            return None;
        }
        String::from_utf16(&words[..words.len() - 1]).ok()
    }
    pub fn set_text(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let words = value.encode_utf16().chain(Some(0));
        let bytes: Vec<_> = words.flat_map(u16::to_le_bytes).collect();
        self.set(name, REG_SZ, &bytes)
    }
    pub fn set_paths(&self, name: &str, paths: &[PathBuf]) -> anyhow::Result<()> {
        let mut bytes = Vec::new();
        for path in paths.iter().map(|path| path.as_os_str()) {
            if path.is_empty() || path.encode_wide().any(|unit| unit == 0) {
                anyhow::bail!("Invalid configuration path");
            }
            bytes.extend(path.encode_wide().chain(Some(0)).flat_map(u16::to_le_bytes));
        }
        bytes.extend_from_slice(&[0, 0]);
        if bytes.len() > MAX_VALUE as usize {
            anyhow::bail!("Configuration paths too large");
        }
        self.set(name, REG_MULTI_SZ, &bytes)
    }
}
fn words(bytes: &[u8]) -> Option<Vec<u16>> {
    bytes.len().is_multiple_of(2).then(|| {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect()
    })
}
fn decode_paths(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    let words = words(bytes)?;
    if words == [0] || words == [0, 0] {
        return Some(Vec::new());
    }
    if !words.ends_with(&[0, 0]) {
        return None;
    }
    let mut paths = Vec::new();
    for part in words[..words.len() - 2].split(|&c| c == 0) {
        if part.is_empty() {
            return None;
        }
        let path = PathBuf::from(OsString::from_wide(part));
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_sz_preserves_windows_paths() {
        let long = format!(r"\\?\C:\{}\元数据.winmd", "segment\\".repeat(40));
        let paths = [r"C:\元数据\😀.winmd", r"\\server\share\types.winmd", &long];
        // Independent REG_MULTI_SZ fixture: UTF-16 paths separated by NUL,
        // with one additional terminating NUL after the final string.
        let input = format!("{}\0\0", paths.join("\0"));
        let bytes: Vec<_> = input.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(
            decode_paths(&bytes),
            Some(paths.map(PathBuf::from).to_vec())
        );
    }

    #[test]
    fn multi_sz_preserves_unpaired_surrogates_in_paths() {
        // Windows paths are WCHAR sequences, not necessarily Unicode strings.
        let units = [0x43, 0x3a, 0x5c, 0xd800, 0x2e, 0x77];
        let bytes: Vec<_> = units
            .into_iter()
            .chain([0, 0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let paths = decode_paths(&bytes).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0].as_os_str().encode_wide().collect::<Vec<_>>(),
            units
        );
    }

    #[test]
    fn multi_sz_rejects_truncated_data() {
        assert_eq!(decode_paths(&[0, 0, 0, 0]), Some(Vec::new()));
        for bytes in [
            &[][..],
            &[0x41],
            &[0x41, 0],
            &[0x41, 0, 0, 0],
            &[0x41, 0, 0, 0, 0],
        ] {
            assert!(decode_paths(bytes).is_none(), "{bytes:?}");
        }
    }
}

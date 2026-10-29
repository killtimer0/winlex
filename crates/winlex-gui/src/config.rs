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
    pub key: Option<Key>,
}
pub struct Key(HKEY);
enum Warning {
    UnsupportedVersion,
    Read(u32),
}
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
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
        if root.is_empty()
            || root.contains(&0)
            || root[0] == b'\\' as u16
            || root.split(|&c| c == b'\\' as u16).any(|s| s.is_empty())
        {
            anyhow::bail!("WINLEX_CONFIG_ROOT must be a nonempty HKCU-relative registry path");
        }
        root.push(0);
        let mut config = Self {
            root,
            version,
            warning: None,
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
            ERROR_FILE_NOT_FOUND => {}
            ERROR_SUCCESS => {
                let key = Key(handle);
                if key.number("Version").is_some_and(|v| v != version) {
                    config.warning = Some(Warning::UnsupportedVersion);
                } else {
                    config.key = Some(key);
                }
            }
            _ => config.warning = Some(Warning::Read(status)),
        }
        Ok(config)
    }
    pub fn warning(&self) -> Option<anyhow::Error> {
        self.warning.as_ref().map(|warning| match warning {
            Warning::UnsupportedVersion => anyhow::anyhow!(
                "Unsupported configuration version; defaults used without overwriting settings"
            ),
            Warning::Read(status) => registry_error("Read configuration", *status),
        })
    }
    pub fn write_key(&self) -> anyhow::Result<Key> {
        if let Some(warning) = self.warning() {
            return Err(warning);
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
    fn value(&self, name: &str, kind: u32) -> Option<Vec<u8>> {
        unsafe {
            let (mut actual, mut size) = (0, 0);
            if RegQueryValueExW(
                self.0,
                wide(name).as_ptr(),
                ptr::null(),
                &mut actual,
                ptr::null_mut(),
                &mut size,
            ) != ERROR_SUCCESS
                || actual != kind
                || size > MAX_VALUE
            {
                return None;
            }
            let mut bytes = vec![0; size as usize];
            if RegQueryValueExW(
                self.0,
                wide(name).as_ptr(),
                ptr::null(),
                &mut actual,
                bytes.as_mut_ptr(),
                &mut size,
            ) != ERROR_SUCCESS
                || actual != kind
            {
                return None;
            }
            bytes.truncate(size as usize);
            Some(bytes)
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
            self.value(name, REG_DWORD)?.try_into().ok()?,
        ))
    }
    pub fn set_number(&self, name: &str, value: u32) -> anyhow::Result<()> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }
    pub fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.value(name, REG_BINARY)
    }
    pub fn set_bytes(&self, name: &str, value: &[u8]) -> anyhow::Result<()> {
        self.set(name, REG_BINARY, value)
    }
    pub fn text(&self, name: &str) -> Option<String> {
        let words = words(&self.value(name, REG_SZ)?)?;
        if words.last() != Some(&0) {
            return None;
        }
        String::from_utf16(&words[..words.len() - 1]).ok()
    }
    pub fn set_text(&self, name: &str, value: &str) -> anyhow::Result<()> {
        let bytes: Vec<_> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
        self.set(name, REG_SZ, &bytes)
    }
    pub fn paths(&self, name: &str) -> Vec<PathBuf> {
        self.value(name, REG_MULTI_SZ)
            .and_then(|bytes| decode_paths(&bytes))
            .unwrap_or_default()
    }
    pub fn set_paths(&self, name: &str, paths: &[PathBuf]) -> anyhow::Result<()> {
        let mut words = Vec::new();
        for path in paths {
            let path: Vec<_> = path.as_os_str().encode_wide().collect();
            if path.is_empty() || path.contains(&0) {
                anyhow::bail!("Invalid configuration path");
            }
            words.extend(path);
            words.push(0);
        }
        words.push(0);
        let bytes: Vec<_> = words.into_iter().flat_map(u16::to_le_bytes).collect();
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
    if words == [0] {
        return Some(Vec::new());
    }
    if !words.ends_with(&[0, 0]) {
        return None;
    }
    let mut paths = Vec::new();
    for part in words[..words.len() - 1].split(|&c| c == 0) {
        if part.is_empty() {
            break;
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
    fn read_warning_preserves_status_when_writes_are_blocked() {
        let config = Config {
            root: vec![],
            version: 1,
            warning: Some(Warning::Read(ERROR_ACCESS_DENIED)),
            key: None,
        };
        let warning = config.warning().unwrap();
        unsafe {
            SetLastError(ERROR_FILE_NOT_FOUND);
        }
        let failure = config.write_key().err().unwrap();
        for error in [warning, failure] {
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(ERROR_ACCESS_DENIED as i32)
            );
            assert!(format!("{error:#}").starts_with("Read configuration: "));
        }
    }

    struct Profile(OsString);
    impl Profile {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            Self(
                format!(
                    "Software\\Winlex\\Tests\\rust-{}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                )
                .into(),
            )
        }
        fn load(&self) -> Config {
            Config::from_root(&self.0, 1).unwrap()
        }
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            let path: Vec<_> = self.0.encode_wide().chain(Some(0)).collect();
            unsafe {
                RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr());
            }
        }
    }
    #[test]
    fn native_paths_and_typed_values_are_isolated() {
        let one = Profile::new();
        let two = Profile::new();
        let config = one.load();
        let key = config.write_key().unwrap();
        let path = PathBuf::from(OsString::from_wide(&[
            67, 58, 92, 0xd800, 46, 119, 105, 110, 109, 100,
        ]));
        key.set_paths("Paths", std::slice::from_ref(&path)).unwrap();
        key.set_number("Number", 144).unwrap();
        key.set_text("Text", "中文").unwrap();
        let loaded = one.load();
        let key = loaded.key.as_ref().unwrap();
        assert_eq!(key.paths("Paths"), vec![path]);
        assert_eq!(key.number("Number"), Some(144));
        assert_eq!(key.text("Text").as_deref(), Some("中文"));
        assert!(two.load().key.is_none());
    }
    #[test]
    fn malformed_values_and_future_version_are_preserved() {
        let profile = Profile::new();
        let config = profile.load();
        let key = config.write_key().unwrap();
        key.set_bytes("Number", &[0; 4]).unwrap();
        key.set("Paths", REG_MULTI_SZ, &[1, 0]).unwrap();
        let loaded = profile.load();
        assert!(loaded.key.as_ref().unwrap().number("Number").is_none());
        assert!(loaded.key.as_ref().unwrap().paths("Paths").is_empty());
        key.set_number("Version", 99).unwrap();
        let future = profile.load();
        assert!(future.warning.is_some() && future.key.is_none());
        assert!(future.write_key().is_err());
        assert_eq!(loaded.key.as_ref().unwrap().number("Version"), Some(99));
    }
}

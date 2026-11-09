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
    fn configuration_root_rejects_empty_components_and_nul() {
        for root in [
            "",
            "\\",
            "\\Software",
            "Software\\",
            "Software\\\\Winlex",
            "Soft\0ware",
        ] {
            assert!(Config::from_root(OsStr::new(root), 1).is_err());
        }
    }

    #[test]
    fn read_warning_preserves_status_when_writes_are_blocked() {
        let config = Config {
            root: vec![],
            version: 1,
            warning: Some(Warning::Read(ERROR_ACCESS_DENIED)),
            paths_warning: None,
            paths: None,
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
        fn write_key(&self) -> Key {
            self.load().write_key().unwrap()
        }
        fn open_key(&self, desired_access: REG_SAM_FLAGS) -> Key {
            let path: Vec<_> = self.0.encode_wide().chain(Some(0)).collect();
            let mut handle = ptr::null_mut();
            assert_eq!(
                unsafe {
                    RegOpenKeyExW(
                        HKEY_CURRENT_USER,
                        path.as_ptr(),
                        0,
                        desired_access,
                        &mut handle,
                    )
                },
                ERROR_SUCCESS
            );
            Key(handle)
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
        let key = one.write_key();
        let path = PathBuf::from(OsString::from_wide(&[
            67, 58, 92, 0xd800, 46, 119, 105, 110, 109, 100,
        ]));
        key.set_paths("AutoLoad", std::slice::from_ref(&path))
            .unwrap();
        key.set_number("Number", 144).unwrap();
        key.set_text("Text", "中文").unwrap();
        let loaded = one.load();
        let key = loaded.key.as_ref().unwrap();
        assert_eq!(loaded.paths, Some(vec![path]));
        assert_eq!(key.number("Number"), Some(144));
        assert_eq!(key.text("Text").as_deref(), Some("中文"));
        assert!(two.load().key.is_none());
    }
    #[test]
    fn future_version_is_preserved() {
        let profile = Profile::new();
        let key = profile.write_key();
        key.set_bytes("Number", &[0; 4]).unwrap();
        let loaded = profile.load();
        assert!(loaded.key.as_ref().unwrap().number("Number").is_none());
        key.set_number("Version", 99).unwrap();
        let future = profile.load();
        assert!(future.warning.is_some() && future.key.is_none());
        assert!(future.write_key().is_err());
        assert_eq!(loaded.key.as_ref().unwrap().number("Version"), Some(99));
    }
    #[test]
    fn invalid_version_blocks_all_writes_without_overwriting_it() {
        let profile = Profile::new();
        let key = profile.write_key();
        key.set_text("Version", "unknown").unwrap();
        let loaded = profile.load();
        assert!(loaded.warning().is_some());
        assert!(loaded.write_key().is_err());
        assert!(loaded.write_paths(&[]).is_err());
        assert_eq!(
            profile.open_key(KEY_QUERY_VALUE).text("Version").as_deref(),
            Some("unknown")
        );
    }
    #[test]
    fn missing_version_and_autoload_use_writable_defaults() {
        let profile = Profile::new();
        let key = profile.write_key();
        assert_eq!(
            unsafe { RegDeleteValueW(key.0, wide("Version").as_ptr()) },
            ERROR_SUCCESS
        );
        let loaded = profile.load();
        assert!(loaded.warning().is_none());
        assert_eq!(loaded.paths, Some(vec![]));
        loaded.write_paths(&[]).unwrap();
        assert_eq!(profile.open_key(KEY_QUERY_VALUE).number("Version"), Some(1));
    }
    #[test]
    fn invalid_autoload_is_preserved_without_blocking_preferences() {
        for (kind, bytes) in [
            (REG_MULTI_SZ, vec![1, 0]),
            (REG_DWORD, 42u32.to_le_bytes().to_vec()),
        ] {
            let profile = Profile::new();
            profile.write_key().set("AutoLoad", kind, &bytes).unwrap();
            let original = profile
                .open_key(KEY_QUERY_VALUE)
                .value("AutoLoad", kind)
                .unwrap()
                .unwrap();
            let loaded = profile.load();
            assert!(loaded.paths.is_none());
            assert!(loaded.warning().is_some());
            assert!(loaded.write_paths(&[]).is_err());
            loaded.write_key().unwrap().set_number("Other", 3).unwrap();
            let saved = profile.open_key(KEY_QUERY_VALUE);
            assert_eq!(saved.value("AutoLoad", kind).unwrap().unwrap(), original);
            assert_eq!(saved.number("Other"), Some(3));
        }
    }
    #[test]
    fn query_retains_registry_status() {
        let profile = Profile::new();
        let key = profile.write_key();
        key.set_number("Value", 1).unwrap();
        let restricted = profile.open_key(KEY_SET_VALUE);
        assert_eq!(
            restricted.value("Value", REG_DWORD),
            Err(ERROR_ACCESS_DENIED)
        );
    }
}

//! User settings. A single HKCU-relative root is selected before any settings access.
//! Native paths are stored as UTF-16 REG_MULTI_SZ, never serialized through UTF-8.
use crate::native::*;
use std::{
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    ptr,
};
use windows_sys::Win32::System::Registry::*;

const VERSION: u32 = 1;
const MAX_VALUE: u32 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Auto,
    English,
    Chinese,
}
impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en-US",
            Self::Chinese => "zh-CN",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
    pub maximized: bool,
}

pub struct Config {
    root: Vec<u16>,
    writable: bool,
    pub warning: Option<String>,
    pub language: Language,
    pub sidebar: i32,
    pub panel: u32,
    pub collapsed: bool,
    pub window: Option<Window>,
    pub files: Vec<PathBuf>,
    pub directories: Vec<PathBuf>,
    pub auto_load: Vec<PathBuf>,
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn registry_error(operation: &str, status: u32) -> String {
    format!(
        "{operation}: {}",
        std::io::Error::from_raw_os_error(status as i32)
    )
}
impl Key {
    fn value(&self, name: &str, kind: u32) -> Option<Vec<u8>> {
        unsafe {
            let mut actual = 0;
            let mut size = 0;
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
    fn number(&self, name: &str) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.value(name, REG_DWORD)?.try_into().ok()?,
        ))
    }
    fn set(&self, name: &str, kind: u32, bytes: &[u8]) -> Result<(), String> {
        let size = u32::try_from(bytes.len()).map_err(|_| "Configuration value too large")?;
        let status =
            unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, kind, bytes.as_ptr(), size) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(registry_error("Write configuration", status))
        }
    }
    fn set_number(&self, name: &str, value: u32) -> Result<(), String> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }
    fn paths(&self, name: &str) -> Vec<PathBuf> {
        self.value(name, REG_MULTI_SZ)
            .and_then(|b| decode_paths(&b))
            .unwrap_or_default()
    }
    fn set_paths(&self, name: &str, paths: &[PathBuf]) -> Result<(), String> {
        self.set(name, REG_MULTI_SZ, &encode_paths(paths)?)
    }
}

fn decode_paths(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let words: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
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
fn encode_paths(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    let mut words = Vec::new();
    for path in paths {
        let path: Vec<_> = path.as_os_str().encode_wide().collect();
        if path.is_empty() || path.contains(&0) {
            return Err("Invalid configuration path".into());
        }
        words.extend(path);
        words.push(0);
    }
    words.push(0);
    let bytes: Vec<_> = words.into_iter().flat_map(u16::to_le_bytes).collect();
    if bytes.len() > MAX_VALUE as usize {
        return Err("Configuration paths too large".into());
    }
    Ok(bytes)
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let root =
            std::env::var_os("WINLEX_CONFIG_ROOT").unwrap_or_else(|| "Software\\Winlex".into());
        Self::from_root(&root)
    }
    fn from_root(root: &OsStr) -> Result<Self, String> {
        let mut root: Vec<_> = root.encode_wide().collect();
        if root.is_empty()
            || root.contains(&0)
            || root[0] == b'\\' as u16
            || root.split(|&c| c == b'\\' as u16).any(|s| s.is_empty())
        {
            return Err("WINLEX_CONFIG_ROOT must be a nonempty HKCU-relative registry path".into());
        }
        root.push(0);
        let mut config = Self {
            root,
            writable: true,
            warning: None,
            language: Language::Auto,
            sidebar: 240,
            panel: 0,
            collapsed: false,
            window: None,
            files: vec![],
            directories: vec![],
            auto_load: vec![],
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
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(config);
        }
        if status != ERROR_SUCCESS {
            config.writable = false;
            config.warning = Some(registry_error("Read configuration", status));
            return Ok(config);
        }
        let key = Key(handle);
        if key.number("Version").is_some_and(|v| v != VERSION) {
            config.writable = false;
            config.warning = Some(
                "Unsupported configuration version; defaults used without overwriting settings"
                    .into(),
            );
            return Ok(config);
        }
        if let Some(bytes) = key.value("Language", REG_SZ)
            && bytes.len().is_multiple_of(2)
        {
            let words: Vec<_> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if words.last() == Some(&0) {
                config.language = match String::from_utf16_lossy(&words[..words.len() - 1]).as_str()
                {
                    "en-US" => Language::English,
                    "zh-CN" => Language::Chinese,
                    _ => Language::Auto,
                };
            }
        }
        config.sidebar = key
            .number("SidebarWidth")
            .filter(|v| (120..=2000).contains(v))
            .unwrap_or(240) as i32;
        config.panel = key.number("ActivePanel").filter(|v| *v <= 1).unwrap_or(0);
        config.collapsed = key.number("PanelCollapsed") == Some(1);
        config.files = key.paths("Files");
        config.directories = key.paths("Directories");
        config.auto_load = key.paths("AutoLoad");
        if let Some(bytes) = key.value("Window", REG_BINARY)
            && bytes.len() == 24
        {
            let values: Vec<_> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| i32::from_le_bytes(*b))
                .collect();
            if (96..=960).contains(&values[4])
                && (1..=100000).contains(&values[2])
                && (1..=100000).contains(&values[3])
                && (0..=1).contains(&values[5])
                && values[0].checked_add(values[2]).is_some()
                && values[1].checked_add(values[3]).is_some()
            {
                config.window = Some(Window {
                    x: values[0],
                    y: values[1],
                    width: values[2],
                    height: values[3],
                    dpi: values[4] as u32,
                    maximized: values[5] == 1,
                });
            }
        }
        Ok(config)
    }
    fn write_key(&self) -> Result<Key, String> {
        if !self.writable {
            return Err(self
                .warning
                .clone()
                .unwrap_or_else(|| "Configuration is read-only".into()));
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
        key.set_number("Version", VERSION)?;
        Ok(key)
    }
    pub fn startup_paths(&self, explicit: Vec<PathBuf>) -> Vec<PathBuf> {
        if explicit.is_empty() {
            self.auto_load.clone()
        } else {
            explicit
        }
    }
    pub fn set_language(&mut self, language: Language) -> Result<(), String> {
        let bytes: Vec<_> = wide(language.name())
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        self.write_key()?.set("Language", REG_SZ, &bytes)?;
        self.language = language;
        Ok(())
    }
    pub fn set_source_intent(&mut self, remove: &[PathBuf], add: &[PathBuf]) -> Result<(), String> {
        let mut auto_load = self.auto_load.clone();
        auto_load.retain(|p| !remove.contains(p));
        let mut files = self.files.clone();
        for path in add {
            if !auto_load.contains(path) {
                auto_load.push(path.clone());
            }
            if !files.contains(path) {
                files.push(path.clone());
            }
        }
        let key = self.write_key()?;
        key.set_paths("Files", &files)?;
        key.set_paths("AutoLoad", &auto_load)?;
        self.files = files;
        self.auto_load = auto_load;
        Ok(())
    }
    pub fn save_layout(&self, window: Window, sidebar: i32, panel: u32) -> Result<(), String> {
        // Unsupported versions/read failures were reported at startup. Never overwrite them.
        if !self.writable {
            return Ok(());
        }
        let key = self.write_key()?;
        let bytes: Vec<_> = [
            window.x,
            window.y,
            window.width,
            window.height,
            window.dpi as i32,
            i32::from(window.maximized),
        ]
        .into_iter()
        .flat_map(i32::to_le_bytes)
        .collect();
        key.set("Window", REG_BINARY, &bytes)?;
        key.set_number("SidebarWidth", sidebar as u32)?;
        key.set_number("ActivePanel", panel)?;
        key.set_number("PanelCollapsed", u32::from(self.collapsed))
    }
    pub fn remember_locations(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let mut files = self.files.clone();
        let mut directories = self.directories.clone();
        for path in paths {
            let path = std::path::absolute(path).map_err(|e| e.to_string())?;
            let list = if path.is_dir() {
                &mut directories
            } else {
                &mut files
            };
            if !list.contains(&path) {
                list.push(path);
            }
        }
        let key = self.write_key()?;
        key.set_paths("Files", &files)?;
        key.set_paths("Directories", &directories)?;
        self.files = files;
        self.directories = directories;
        Ok(())
    }
    pub fn forget_location(&mut self, path: &std::path::Path) -> Result<(), String> {
        let files = self
            .files
            .iter()
            .filter(|p| p.as_path() != path)
            .cloned()
            .collect::<Vec<_>>();
        let directories = self
            .directories
            .iter()
            .filter(|p| p.as_path() != path)
            .cloned()
            .collect::<Vec<_>>();
        let key = self.write_key()?;
        key.set_paths("Files", &files)?;
        key.set_paths("Directories", &directories)?;
        self.files = files;
        self.directories = directories;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Profile(OsString);
    impl Profile {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            Self(
                format!(
                    "Software\\Winlex\\Tests\\rust-{}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                )
                .into(),
            )
        }
        fn load(&self) -> Config {
            Config::from_root(&self.0).unwrap()
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
    fn independent_roots_and_explicit_inputs_do_not_overwrite_saved_intent() {
        let one = Profile::new();
        let two = Profile::new();
        let mut config = one.load();
        let a = PathBuf::from(r"C:\saved.winmd");
        let b = PathBuf::from(r"C:\temporary.winmd");
        let c = PathBuf::from(r"C:\added.winmd");
        config
            .set_source_intent(&[], std::slice::from_ref(&a))
            .unwrap();
        config.set_language(Language::Chinese).unwrap();
        assert_eq!(config.startup_paths(vec![b.clone()]), vec![b]);
        config
            .set_source_intent(&[], std::slice::from_ref(&c))
            .unwrap();
        assert_eq!(one.load().auto_load, vec![a.clone(), c]);
        config
            .set_source_intent(std::slice::from_ref(&a), &[])
            .unwrap();
        assert!(!one.load().auto_load.contains(&a));
        assert!(one.load().files.contains(&a));
        assert_eq!(two.load().language, Language::Auto);
        assert!(two.load().auto_load.is_empty());
    }

    #[test]
    fn native_paths_and_window_settings_survive_reopen() {
        let profile = Profile::new();
        let mut config = profile.load();
        let path = PathBuf::from(OsString::from_wide(&[
            67, 58, 92, 0xd800, 46, 119, 105, 110, 109, 100,
        ]));
        config
            .set_source_intent(&[], std::slice::from_ref(&path))
            .unwrap();
        let window = Window {
            x: -1400,
            y: 40,
            width: 900,
            height: 650,
            dpi: 144,
            maximized: true,
        };
        config.save_layout(window, 320, 1).unwrap();
        let loaded = profile.load();
        assert_eq!(loaded.auto_load, vec![path]);
        assert_eq!(loaded.window, Some(window));
        assert_eq!((loaded.sidebar, loaded.panel), (320, 1));
    }

    #[test]
    fn malformed_values_fall_back_and_future_version_is_not_overwritten() {
        let profile = Profile::new();
        let config = profile.load();
        let key = config.write_key().unwrap();
        key.set("Language", REG_BINARY, b"zh-CN").unwrap();
        key.set_number("SidebarWidth", u32::MAX).unwrap();
        key.set("Window", REG_BINARY, &[0; 24]).unwrap();
        key.set("AutoLoad", REG_MULTI_SZ, &[1, 0]).unwrap();
        let loaded = profile.load();
        assert_eq!(loaded.language, Language::Auto);
        assert_eq!(loaded.sidebar, 240);
        assert!(loaded.window.is_none() && loaded.auto_load.is_empty());
        key.set_number("Version", 99).unwrap();
        let mut future = profile.load();
        assert!(future.warning.is_some());
        assert!(future.set_language(Language::English).is_err());
        drop(key);
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    config.root.as_ptr(),
                    0,
                    KEY_QUERY_VALUE,
                    &mut handle,
                )
            },
            ERROR_SUCCESS
        );
        assert_eq!(Key(handle).number("Version"), Some(99));
    }
}

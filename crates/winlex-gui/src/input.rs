//! Native file selection; paths stay UTF-16/OsString end to end.
use crate::{
    native::*,
    platform::WindowsVersion,
    resources::{self, *},
};
use anyhow::Context;
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};
use windows_sys::Win32::UI::Controls::Dialogs::*;
pub struct Apartment(std::marker::PhantomData<std::rc::Rc<()>>);
impl Apartment {
    pub fn initialize() -> anyhow::Result<Self> {
        unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            )
            .ok()
        }
        .context("Initialize COM")?;
        Ok(Self(std::marker::PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}
pub fn choose(
    owner: HWND,
    version: WindowsVersion,
    title: u16,
    label: u16,
    pattern: &str,
    multiple: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    if version.at_least(WindowsVersion::VISTA) {
        modern_choose(owner, title, label, pattern, multiple).context("Select files")
    } else {
        legacy_choose(owner, title, label, pattern, multiple)
    }
}

fn legacy_choose(
    owner: HWND,
    title: u16,
    label: u16,
    pattern: &str,
    multiple: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut filter = Vec::new();
    for s in [
        resources::text(label),
        pattern.into(),
        resources::text(IDS_ALL_FILES),
        "*.*".into(),
    ] {
        filter.extend(s.encode_utf16().chain(Some(0)));
    }
    filter.push(0);
    let title = resources::wide_text(title);
    let mut buffer = vec![0u16; 65536];
    loop {
        let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrTitle = title.as_ptr();
        ofn.lpstrFile = buffer.as_mut_ptr();
        ofn.nMaxFile = u32::try_from(buffer.len()).context("File selection is too large")?;
        ofn.Flags = OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_DONTADDTORECENT
            | if multiple { OFN_ALLOWMULTISELECT } else { 0 };
        if unsafe { GetOpenFileNameW(&mut ofn) } != 0 {
            return Ok(selected_paths(&buffer, multiple));
        }
        let error = unsafe { CommDlgExtendedError() };
        if error == 0 {
            return Ok(Vec::new());
        }
        if !multiple || error != FNERR_BUFFERTOOSMALL {
            anyhow::bail!("GetOpenFileNameW: 0x{error:08x}");
        }
        let size = (buffer[0] as usize).max(
            buffer
                .len()
                .checked_mul(2)
                .context("File selection is too large")?,
        );
        buffer = vec![0; size];
    }
}
fn selected_paths(buffer: &[u16], multiple: bool) -> Vec<PathBuf> {
    if !multiple {
        let length = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
        return vec![PathBuf::from(OsString::from_wide(&buffer[..length]))];
    }
    let parts: Vec<_> = buffer
        .split(|c| *c == 0)
        .take_while(|s| !s.is_empty())
        .map(OsString::from_wide)
        .collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [path] => vec![PathBuf::from(path)],
        [directory, names @ ..] => names
            .iter()
            .map(|name| PathBuf::from(directory).join(name))
            .collect(),
    }
}

fn modern_choose(
    owner: HWND,
    title: u16,
    label: u16,
    pattern: &str,
    multiple: bool,
) -> windows::core::Result<Vec<PathBuf>> {
    use windows::{
        Win32::{
            Foundation::{ERROR_CANCELLED, HWND as ComHwnd},
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
            UI::Shell::{
                Common::COMDLG_FILTERSPEC, FOS_ALLOWMULTISELECT, FOS_DONTADDTORECENT,
                FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST,
                FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
            },
        },
        core::{PCWSTR, w},
    };
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let options = (dialog.GetOptions()? & !FOS_ALLOWMULTISELECT)
            | FOS_FORCEFILESYSTEM
            | FOS_PATHMUSTEXIST
            | FOS_NOCHANGEDIR
            | FOS_DONTADDTORECENT
            | FOS_FILEMUSTEXIST
            | if multiple {
                FOS_ALLOWMULTISELECT
            } else {
                Default::default()
            };
        dialog.SetOptions(options)?;
        let title = resources::wide_text(title);
        dialog.SetTitle(PCWSTR(title.as_ptr()))?;
        let label = resources::wide_text(label);
        let pattern = crate::native::wide(pattern);
        let all = resources::wide_text(IDS_ALL_FILES);
        dialog.SetFileTypes(&[
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(label.as_ptr()),
                pszSpec: PCWSTR(pattern.as_ptr()),
            },
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(all.as_ptr()),
                pszSpec: w!("*.*"),
            },
        ])?;
        if let Err(error) = dialog.Show(ComHwnd(owner)) {
            if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(Vec::new());
            }
            return Err(error);
        }
        let items = dialog.GetResults()?;
        let mut paths = Vec::new();
        for index in 0..items.GetCount()? {
            let path = items.GetItemAt(index)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let value = OsString::from_wide(path.as_wide());
            CoTaskMemFree(Some(path.0.cast()));
            paths.push(value.into());
        }
        Ok(paths)
    }
}

// Explorer drops use the same append operation as the file picker.
pub unsafe fn dropped_files(drop: windows_sys::Win32::UI::Shell::HDROP) -> Vec<PathBuf> {
    use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW};
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, ptr::null_mut(), 0);
        let mut paths = Vec::new();
        for index in 0..count {
            let length = DragQueryFileW(drop, index, ptr::null_mut(), 0);
            let mut buffer = vec![0; length as usize + 1];
            let copied = DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32);
            if copied > 0 {
                paths.push(PathBuf::from(OsString::from_wide(
                    &buffer[..copied as usize],
                )));
            }
        }
        DragFinish(drop);
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    #[test]
    fn legacy_selections_preserve_native_single_and_multiple_paths() {
        for prefix in [
            r"C:\",
            r"\\server\share\",
            r"\\?\C:\",
            r"\\?\UNC\server\share\",
        ] {
            let directory = format!("{prefix}{}", "directory\\".repeat(40));
            let mut filename: Vec<_> = "name".encode_utf16().collect();
            filename.extend([0xd800]);
            filename.extend(".winmd".encode_utf16());
            let path = PathBuf::from(&directory).join(OsString::from_wide(&filename));
            let single: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            for multiple in [false, true] {
                assert_eq!(
                    selected_paths(&single, multiple),
                    std::slice::from_ref(&path)
                );
            }
            let mut selected = wide(&directory);
            selected.extend(&filename);
            selected.extend([0, 0]);
            assert_eq!(selected_paths(&selected, true), [path]);
        }
        assert!(selected_paths(&[0, 0], true).is_empty());
        assert_eq!(selected_paths(&[0, 0], false), [PathBuf::new()]);
    }
}

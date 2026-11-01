//! Native file selection; paths stay UTF-16/OsString end to end.
use crate::{
    native::*,
    platform::WindowsVersion,
    resources::{self, *},
};
use anyhow::Context;
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};
use windows_sys::Win32::UI::Controls::Dialogs::*;
pub struct Apartment;
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
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}
pub fn choose_files(owner: HWND, version: WindowsVersion) -> anyhow::Result<Vec<PathBuf>> {
    if version.at_least(WindowsVersion::VISTA) {
        return modern_choose(owner).context("Select files");
    }
    let mut filter = Vec::new();
    for s in [
        resources::text(IDS_WINMD_FILES),
        "*.winmd".into(),
        resources::text(IDS_ALL_FILES),
        "*.*".into(),
    ] {
        filter.extend(s.encode_utf16().chain(Some(0)));
    }
    filter.push(0);
    let title = resources::wide_text(IDS_LOAD_METADATA);
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
            | OFN_ALLOWMULTISELECT;
        if unsafe { GetOpenFileNameW(&mut ofn) } != 0 {
            let parts: Vec<_> = buffer
                .split(|c| *c == 0)
                .take_while(|s| !s.is_empty())
                .map(OsString::from_wide)
                .collect();
            return Ok(match parts.as_slice() {
                [] => Vec::new(),
                [path] => vec![PathBuf::from(path)],
                [directory, names @ ..] => names
                    .iter()
                    .map(|name| PathBuf::from(directory).join(name))
                    .collect(),
            });
        }
        let error = unsafe { CommDlgExtendedError() };
        if error == 0 {
            return Ok(Vec::new());
        }
        if error != FNERR_BUFFERTOOSMALL {
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

fn modern_choose(owner: HWND) -> windows::core::Result<Vec<PathBuf>> {
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
        let options = dialog.GetOptions()?
            | FOS_FORCEFILESYSTEM
            | FOS_PATHMUSTEXIST
            | FOS_NOCHANGEDIR
            | FOS_DONTADDTORECENT
            | FOS_FILEMUSTEXIST
            | FOS_ALLOWMULTISELECT;
        dialog.SetOptions(options)?;
        let title = resources::wide_text(IDS_LOAD_METADATA);
        dialog.SetTitle(PCWSTR(title.as_ptr()))?;
        let winmd = resources::wide_text(IDS_WINMD_FILES);
        let all = resources::wide_text(IDS_ALL_FILES);
        dialog.SetFileTypes(&[
            COMDLG_FILTERSPEC {
                pszName: PCWSTR(winmd.as_ptr()),
                pszSpec: w!("*.winmd"),
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

//! Native file selection; paths stay UTF-16/OsString end to end.
use crate::{
    native::*,
    resources::{self, *},
};
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};
use windows_sys::Win32::UI::Controls::Dialogs::*;
pub fn choose_directory(owner: HWND) -> Result<Option<PathBuf>, String> {
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::*};
    unsafe {
        let mut display = [0u16; 260];
        let title = wide(&resources::text(IDS_DIRECTORY));
        let info = BROWSEINFOW {
            hwndOwner: owner,
            pszDisplayName: display.as_mut_ptr(),
            lpszTitle: title.as_ptr(),
            ulFlags: BIF_RETURNONLYFSDIRS,
            ..std::mem::zeroed()
        };
        let item = SHBrowseForFolderW(&info);
        if item.is_null() {
            return Ok(None);
        }
        let mut path = [0u16; 260];
        let ok = SHGetPathFromIDListW(item, path.as_mut_ptr());
        CoTaskMemFree(item.cast());
        if ok == 0 {
            return Err("Cannot resolve selected directory".into());
        }
        Ok(Some(
            OsString::from_wide(&path[..path.iter().position(|&c| c == 0).unwrap_or(path.len())])
                .into(),
        ))
    }
}

pub fn choose_files(owner: HWND, multiple: bool) -> Result<Vec<PathBuf>, String> {
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
    let title = wide(&resources::text(IDS_LOAD_METADATA));
    let mut buffer = vec![0u16; 65536];
    loop {
        let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrTitle = title.as_ptr();
        ofn.lpstrFile = buffer.as_mut_ptr();
        ofn.nMaxFile = u32::try_from(buffer.len()).map_err(|_| "File selection is too large")?;
        ofn.Flags = OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_NOCHANGEDIR
            | OFN_DONTADDTORECENT
            | if multiple { OFN_ALLOWMULTISELECT } else { 0 };
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
            return Err(format!("GetOpenFileNameW: 0x{error:08x}"));
        }
        let size = (buffer[0] as usize).max(
            buffer
                .len()
                .checked_mul(2)
                .ok_or("File selection is too large")?,
        );
        buffer = vec![0; size];
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

//! Apply the native root theme once the public file-dialog lifecycle exposes its window.
#![allow(non_snake_case)]

use std::cell::Cell;
use windows::{
    Win32::{
        Foundation::HWND,
        System::Ole::IOleWindow,
        UI::Shell::{
            FDE_OVERWRITE_RESPONSE, FDE_SHAREVIOLATION_RESPONSE, FDEOR_DEFAULT, FDESVR_DEFAULT,
            IFileDialog, IFileDialogEvents, IFileDialogEvents_Impl, IShellItem,
        },
    },
    core::{Interface, Result, implement},
};

pub(super) struct Subscription {
    dialog: IFileDialog,
    cookie: u32,
}

pub(super) fn advise(dialog: &IFileDialog) -> Result<Subscription> {
    let events: IFileDialogEvents = ThemeEvents {
        prepared: Cell::new(false),
        themed: Cell::new(None),
    }
    .into();
    let cookie = unsafe { dialog.Advise(&events)? };
    Ok(Subscription {
        dialog: dialog.clone(),
        cookie,
    })
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Also runs when Show is cancelled or any subsequent COM operation fails.
        let _ = unsafe { self.dialog.Unadvise(self.cookie) };
    }
}

#[implement(IFileDialogEvents)]
struct ThemeEvents {
    prepared: Cell<bool>,
    themed: Cell<Option<HWND>>,
}

impl ThemeEvents {
    fn apply(&self, dialog: Option<&IFileDialog>) {
        let Some(dialog) = dialog else { return };
        if !self.prepared.replace(true) {
            crate::theme::prepare_file_dialog();
        }
        // OnFolderChanging can run before the dialog has created its window.
        // A missing HWND must not veto navigation; OnFolderChange retries.
        let Ok(window) = dialog.cast::<IOleWindow>() else {
            return;
        };
        let Ok(hwnd) = (unsafe { window.GetWindow() }) else {
            return;
        };
        if hwnd.0.is_null() {
            return;
        }
        if self.themed.get() != Some(hwnd) {
            crate::theme::apply_file_dialog(hwnd.0);
            self.themed.set(Some(hwnd));
        }
        // Shell can create the standard bottom-row controls after the first
        // HWND-bearing callback. Navigation completion retries just those classes.
        crate::theme::apply_file_dialog_controls(hwnd.0);
    }
}

impl IFileDialogEvents_Impl for ThemeEvents_Impl {
    fn OnFileOk(&self, _dialog: Option<&IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnFolderChanging(
        &self,
        dialog: Option<&IFileDialog>,
        _folder: Option<&IShellItem>,
    ) -> Result<()> {
        self.apply(dialog);
        Ok(())
    }

    fn OnFolderChange(&self, dialog: Option<&IFileDialog>) -> Result<()> {
        self.apply(dialog);
        Ok(())
    }

    fn OnSelectionChange(&self, _dialog: Option<&IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnShareViolation(
        &self,
        _dialog: Option<&IFileDialog>,
        _item: Option<&IShellItem>,
    ) -> Result<FDE_SHAREVIOLATION_RESPONSE> {
        Ok(FDESVR_DEFAULT)
    }

    fn OnTypeChange(&self, _dialog: Option<&IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnOverwrite(
        &self,
        _dialog: Option<&IFileDialog>,
        _item: Option<&IShellItem>,
    ) -> Result<FDE_OVERWRITE_RESPONSE> {
        Ok(FDEOR_DEFAULT)
    }
}

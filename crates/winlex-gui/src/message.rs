//! Application messages use the native dialog appropriate for the Windows version.
use crate::{native::*, platform::WindowsVersion};
use std::ptr;

/// Last-resort startup reporting, independent of resources, themes and OS probing.
/// This is not a fallback for application dialog failures.
pub(crate) fn startup_error(message: &str) {
    let message = wide(message);
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            message.as_ptr(),
            w!("Winlex"),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    MessageBox,
    TaskDialog,
}

fn backend(version: WindowsVersion) -> Backend {
    if version >= WindowsVersion::VISTA {
        Backend::TaskDialog
    } else {
        Backend::MessageBox
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Error,
    Information,
}

#[derive(Clone, Copy)]
enum Buttons {
    Ok,
    YesNo,
    YesNoDefaultNo,
}

/// Show an error with a native OK button; Escape and the close button are allowed.
pub fn error(owner: HWND, title: &str, text: &str) -> anyhow::Result<()> {
    error_result(show(owner, title, text, Icon::Error, Buttons::Ok)?)
}

fn error_result(button: i32) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(button, IDOK | IDCANCEL),
        "Unexpected error dialog result: {button}"
    );
    Ok(())
}

/// Ask a Yes/No question, defaulting to Yes without allowing cancellation.
pub fn confirm(owner: HWND, title: &str, text: &str) -> anyhow::Result<bool> {
    confirmation_result(show(owner, title, text, Icon::Information, Buttons::YesNo)?)
}

/// Ask a Yes/No question defaulting to No, for actions that run local code.
pub fn confirm_no(owner: HWND, title: &str, text: &str) -> anyhow::Result<bool> {
    confirmation_result(show(
        owner,
        title,
        text,
        Icon::Information,
        Buttons::YesNoDefaultNo,
    )?)
}

fn confirmation_result(button: i32) -> anyhow::Result<bool> {
    match button {
        IDYES => Ok(true),
        IDNO => Ok(false),
        _ => anyhow::bail!("Unexpected confirmation dialog result: {button}"),
    }
}

fn show(owner: HWND, title: &str, text: &str, icon: Icon, buttons: Buttons) -> anyhow::Result<i32> {
    let title = wide(title);
    let text = wide(text);
    match backend(WindowsVersion::query()?) {
        Backend::MessageBox => {
            let flags = match icon {
                Icon::Error => MB_ICONERROR,
                Icon::Information => MB_ICONINFORMATION,
            } | match buttons {
                Buttons::Ok => MB_OK,
                Buttons::YesNo => MB_YESNO | MB_DEFBUTTON1,
                Buttons::YesNoDefaultNo => MB_YESNO | MB_DEFBUTTON2,
            };
            let button = unsafe { MessageBoxW(owner, text.as_ptr(), title.as_ptr(), flags) };
            if button == 0 {
                return Err(crate::native::error("Show message box"));
            }
            Ok(button)
        }
        Backend::TaskDialog => {
            let config = task_config(owner, &title, &text, icon, buttons);
            run_task_dialog(&config)
        }
    }
}

fn task_config(
    owner: HWND,
    title: &[u16],
    text: &[u16],
    icon: Icon,
    buttons: Buttons,
) -> TASKDIALOGCONFIG {
    let mut config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: TDF_POSITION_RELATIVE_TO_WINDOW,
        pszWindowTitle: title.as_ptr(),
        pszContent: text.as_ptr(),
        pfCallback: Some(task_callback),
        ..unsafe { std::mem::zeroed() }
    };
    // Keep the caller's DPI awareness and the common control's native layout.
    config.Anonymous1.pszMainIcon = match icon {
        Icon::Error => TD_ERROR_ICON,
        Icon::Information => TD_INFORMATION_ICON,
    };
    match buttons {
        Buttons::Ok => {
            config.dwFlags |= TDF_ALLOW_DIALOG_CANCELLATION;
            config.dwCommonButtons = TDCBF_OK_BUTTON;
            config.nDefaultButton = IDOK;
        }
        Buttons::YesNo => {
            config.dwCommonButtons = TDCBF_YES_BUTTON | TDCBF_NO_BUTTON;
            config.nDefaultButton = IDYES;
        }
        Buttons::YesNoDefaultNo => {
            config.dwCommonButtons = TDCBF_YES_BUTTON | TDCBF_NO_BUTTON;
            config.nDefaultButton = IDNO;
        }
    }
    config
}

unsafe extern "system" fn task_callback(
    hwnd: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    _: WPARAM,
    _: LPARAM,
    _: isize,
) -> windows_sys::core::HRESULT {
    if matches!(
        notification,
        TDN_DIALOG_CONSTRUCTED | TDN_CREATED | TDN_NAVIGATED
    ) {
        crate::theme::apply_task_dialog(hwnd);
    }
    0
}

type TaskDialogIndirect = unsafe extern "system" fn(
    *const TASKDIALOGCONFIG,
    *mut i32,
    *mut i32,
    *mut BOOL,
) -> windows_sys::core::HRESULT;

owned_handle! { struct CommonControls(HMODULE); FreeLibrary }

fn run_task_dialog(config: &TASKDIALOGCONFIG) -> anyhow::Result<i32> {
    // LoadLibrary honors the executable's common-controls v6 activation context.
    // Do not statically import this Vista API, and never fall back on modern OSes.
    let module = unsafe { LoadLibraryW(w!("comctl32.dll")) };
    if module.is_null() {
        return Err(crate::native::error(
            "Load common-controls v6 for TaskDialogIndirect",
        ));
    }
    let module = CommonControls(module);
    let address = unsafe { GetProcAddress(module.0, c"TaskDialogIndirect".as_ptr().cast()) };
    let address = address.ok_or_else(|| {
        anyhow::anyhow!(
            "TaskDialogIndirect is unavailable in common-controls v6 on Windows Vista or later"
        )
    })?;
    let task_dialog = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, TaskDialogIndirect>(address)
    };
    let mut button = 0;
    let guard = crate::theme::TaskDialogThemeGuard::new(task_dialog as usize);
    anyhow::ensure!(
        !crate::theme::is_dark() || guard.is_active(),
        "Install task-dialog dark theme hooks"
    );
    let status = unsafe { task_dialog(config, &mut button, ptr::null_mut(), ptr::null_mut()) };
    task_result(status, button)
}

fn task_result(status: windows_sys::core::HRESULT, button: i32) -> anyhow::Result<i32> {
    if status < 0 {
        return Err(
            anyhow::Error::new(windows_core::Error::from_hresult(windows_core::HRESULT(
                status,
            )))
            .context("Show task dialog"),
        );
    }
    anyhow::ensure!(button != 0, "Task dialog returned no button");
    Ok(button)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_dialog_routing_depends_on_version_not_theme_capabilities() {
        for version in [
            WindowsVersion::VISTA,
            WindowsVersion::DARK_MODE,
            WindowsVersion::WINDOWS_11,
        ] {
            assert_eq!(backend(version), Backend::TaskDialog);
        }
    }

    #[test]
    fn confirmation_distinguishes_no_from_failure_or_cancellation() {
        assert!(confirmation_result(IDYES).unwrap());
        assert!(!confirmation_result(IDNO).unwrap());
        for button in [0, IDOK, IDCANCEL, -1] {
            assert!(confirmation_result(button).is_err());
        }
    }

    #[test]
    fn error_dialog_accepts_acknowledgement_and_close_but_not_failures() {
        assert!(error_result(IDOK).is_ok());
        assert!(error_result(IDCANCEL).is_ok());
        for button in [0, IDYES, IDNO, -1] {
            assert!(error_result(button).is_err());
        }
    }

    #[test]
    fn task_failures_cannot_be_mistaken_for_a_selected_button() {
        let failure = task_result(0x80070005u32 as i32, IDYES).unwrap_err();
        assert_eq!(
            failure
                .downcast_ref::<windows_core::Error>()
                .unwrap()
                .code(),
            windows_core::HRESULT(0x80070005u32 as i32)
        );
        assert!(task_result(0, 0).is_err());
        assert_eq!(task_result(0, IDNO).unwrap(), IDNO);
    }

    #[test]
    fn native_buttons_preserve_default_and_cancellation_contracts() {
        let title = wide("title");
        let text = wide("text");
        let error = task_config(ptr::null_mut(), &title, &text, Icon::Error, Buttons::Ok);
        let confirm = task_config(
            ptr::null_mut(),
            &title,
            &text,
            Icon::Information,
            Buttons::YesNo,
        );
        // TASKDIALOGCONFIG is packed: copy fields instead of borrowing them.
        let buttons = error.dwCommonButtons;
        let default = error.nDefaultButton;
        assert_eq!(buttons, TDCBF_OK_BUTTON);
        assert_eq!(default, IDOK);
        assert_ne!(error.dwFlags & TDF_ALLOW_DIALOG_CANCELLATION, 0);
        let buttons = confirm.dwCommonButtons;
        let default = confirm.nDefaultButton;
        assert_eq!(buttons, TDCBF_YES_BUTTON | TDCBF_NO_BUTTON);
        assert_eq!(default, IDYES);
        assert_eq!(confirm.dwFlags & TDF_ALLOW_DIALOG_CANCELLATION, 0);
        assert_eq!(confirm.dwFlags & TDF_USE_HICON_MAIN, 0);
        assert_eq!(
            unsafe { confirm.Anonymous1.pszMainIcon },
            TD_INFORMATION_ICON
        );
    }

    #[test]
    fn native_task_dialog_returns_native_button_ids_without_user_interaction() {
        if backend(WindowsVersion::query().unwrap()) != Backend::TaskDialog {
            return;
        }
        struct Selection {
            button: i32,
            constructed: bool,
        }
        unsafe extern "system" fn select(
            hwnd: HWND,
            notification: TASKDIALOG_NOTIFICATIONS,
            wparam: WPARAM,
            lparam: LPARAM,
            data: isize,
        ) -> windows_sys::core::HRESULT {
            unsafe {
                task_callback(hwnd, notification, wparam, lparam, 0);
                let selection = &mut *(data as *mut Selection);
                if notification == TDN_DIALOG_CONSTRUCTED {
                    selection.constructed = true;
                    crate::test_support::move_offscreen(hwnd);
                } else if notification == TDN_CREATED {
                    let button = selection.button;
                    ShowWindow(hwnd, SW_HIDE);
                    if button == IDCANCEL {
                        SendMessageW(hwnd, WM_CLOSE, 0, 0);
                    } else {
                        SendMessageW(hwnd, TDM_CLICK_BUTTON as u32, button as usize, 0);
                    }
                } else if notification == TDN_TIMER && wparam >= 1000 {
                    // Closing completes after the callback returns. A delayed
                    // watchdog catches broken cancellation without replacing
                    // its result while native teardown is still pending.
                    SendMessageW(hwnd, TDM_CLICK_BUTTON as u32, IDOK as usize, 0);
                }
            }
            0
        }
        let _activation = crate::theme::tests::CommonControls::activate();
        let title = wide("Winlex message test");
        let text = wide("Native dialog result");
        for mode in [1, 2] {
            for (icon, buttons, button) in [
                (Icon::Information, Buttons::YesNo, IDYES),
                (Icon::Information, Buttons::YesNo, IDNO),
                (Icon::Error, Buttons::Ok, IDOK),
                (Icon::Error, Buttons::Ok, IDCANCEL),
            ] {
                crate::theme::set_mode(mode);
                let mut selection = Selection {
                    button,
                    constructed: false,
                };
                let mut config = task_config(ptr::null_mut(), &title, &text, icon, buttons);
                config.pfCallback = Some(select);
                config.dwFlags |= TDF_CALLBACK_TIMER;
                config.lpCallbackData = &mut selection as *mut Selection as isize;
                assert_eq!(run_task_dialog(&config).unwrap(), button);
                assert!(selection.constructed);
            }
        }
        crate::theme::set_mode(0);
    }
}

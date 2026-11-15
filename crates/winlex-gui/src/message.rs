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

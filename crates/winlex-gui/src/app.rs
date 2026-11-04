//! UI-thread state, resource lifetime and the native message loop.
//!
//! Frame owns shared controls and root DPI orchestration; workspace owns MDI
//! lifecycle/arrangement; document owns its controls and content. Native MDI is
//! the sole authority for activation. State borrows must end before Win32 calls
//! that can synchronously reenter a window procedure.
use crate::{
    dpi::{Dpi, Font},
    layout,
    native::*,
};
use std::{cell::RefCell, ptr};

mod catalog;
mod content;
mod document;
mod frame;
mod help;
mod history;
mod keys;
mod metadata;
mod panel;
mod settings;
mod sidebar;
mod sources;
mod tools;
mod workspace;

use crate::resources::{self, *};
use document::document_proc;
use frame::frame_proc;
use workspace::active;

struct App {
    config: crate::config::Config,
    settings: settings::Settings,
    dpi: Dpi,
    system_version: crate::platform::WindowsVersion,
    frame: HWND,
    icons: Option<crate::icons::WindowIcons>,
    status: HWND,
    status_state: frame::Status,
    session: catalog::Session,
    sources: sources::Sources,
    worker: crate::worker::Worker<crate::tasks::Job, crate::tasks::Completion>,
    sidebar: sidebar::Sidebar,
    workspace: workspace::Workspace,
    fonts: Vec<Font>,
    failure: Option<anyhow::Error>,
}
thread_local! {static APP:RefCell<Option<App>>=const {RefCell::new(None)};}
// These closures only inspect/update owned data; never send messages inside them.
fn read<T>(f: impl FnOnce(&App) -> T) -> T {
    APP.with(|a| f(a.borrow().as_ref().unwrap()))
}
fn write<T>(f: impl FnOnce(&mut App) -> T) -> T {
    APP.with(|a| f(a.borrow_mut().as_mut().unwrap()))
}
fn checked<T>(result: anyhow::Result<T>) -> Option<T> {
    result
        .map_err(|error| write(|a| a.failure = Some(error)))
        .ok()
}
fn dpi() -> Dpi {
    read(|a| a.dpi)
}
fn font_for(value: u32) -> (HFONT, i32) {
    if let Some(font) = read(|a| {
        a.fonts
            .iter()
            .find(|f| f.dpi == value)
            .map(|f| (f.handle.0, f.height))
    }) {
        return font;
    }
    let Some(font) = checked(Font::new(value)) else {
        // Window callbacks report errors at the application boundary.
        return (ptr::null_mut(), 0);
    };
    let result = (font.handle.0, font.height);
    write(|a| a.fonts.push(font));
    result
}
fn bind(hwnd: HWND, font: HFONT) {
    if send(hwnd, WM_GETFONT, 0, 0) != font as isize {
        send(hwnd, WM_SETFONT, font as usize, 1);
    }
}
fn metrics(hwnd: HWND) -> layout::Metrics {
    let dpi = dpi().window(hwnd);
    let (font, height) = font_for(dpi);
    layout::Metrics { dpi, font, height }
}

pub fn run(
    dpi: Dpi,
    config: crate::config::Config,
    paths: Vec<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let system_version = crate::platform::WindowsVersion::query()?;
    let _apartment = crate::input::Apartment::initialize()?;
    let _graphics = crate::icons::Runtime::new()?;
    let settings = settings::Settings::load(&config);
    crate::theme::set_mode(settings.preferences.theme as u32);
    resources::initialize(settings.preferences.language)?;
    let session = catalog::Session::new(paths.is_empty());
    let worker = crate::worker::Worker::new(crate::tasks::run)?;
    let paths = if paths.is_empty() {
        config
            .key
            .as_ref()
            .map_or_else(Vec::new, |key| key.paths("AutoLoad"))
    } else {
        paths
    };
    let sidebar = sidebar::Sidebar::new(&config);
    APP.with(|a| {
        *a.borrow_mut() = Some(App {
            config,
            settings,
            dpi,
            system_version,
            frame: ptr::null_mut(),
            icons: None,
            status: ptr::null_mut(),
            status_state: frame::Status::default(),
            session,
            sources: sources::Sources::default(),
            worker,
            sidebar,
            workspace: workspace::Workspace::default(),
            fonts: Vec::new(),
            failure: None,
        })
    });
    let result = unsafe { run_windows(paths) };
    APP.with(|a| a.borrow_mut().take());
    result
}
unsafe fn run_windows(paths: Vec<std::path::PathBuf>) -> anyhow::Result<()> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES | ICC_LINK_CLASS,
        };
        if InitCommonControlsEx(&controls) == 0 {
            anyhow::bail!("Initialize common controls failed");
        }
        let classes: [(*const u16, WNDPROC); 3] = [
            (w!("Winlex.Frame"), Some(frame_proc)),
            (w!("Winlex.Document"), Some(document_proc)),
            (w!("Winlex.Splitter"), Some(sidebar::splitter_proc)),
        ];
        for (name, proc) in classes {
            let class = WNDCLASSW {
                lpfnWndProc: proc,
                hInstance: instance(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hIcon: LoadIconW(ptr::null_mut(), IDI_APPLICATION),
                hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
                lpszClassName: name,
                ..std::mem::zeroed()
            };
            if RegisterClassW(&class) == 0 {
                return Err(error("Register window class"));
            }
        }
        let menu = resources::menu()?;
        let value = dpi().system();
        let saved = read(|a| a.settings.window);
        let frame = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("Winlex.Frame"),
            resources::wide_text(IDS_APP_TITLE).as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            saved.map_or(CW_USEDEFAULT, |w| w.x),
            saved.map_or(CW_USEDEFAULT, |w| w.y),
            layout::scale(layout::DEFAULT_WIDTH, value),
            layout::scale(layout::DEFAULT_HEIGHT, value),
            ptr::null_mut(),
            menu,
            instance(),
            ptr::null(),
        );
        if frame.is_null() {
            let failure =
                write(|a| a.failure.take()).unwrap_or_else(|| error("Create main window"));
            DestroyMenu(menu);
            return Err(failure);
        }
        if let Some(error) = write(|a| a.failure.take()) {
            DestroyWindow(frame);
            return Err(error);
        }
        settings::restore_window(frame);
        let accelerators = LoadAcceleratorsW(instance(), IDR_ACCELERATORS as usize as *const u16);
        if accelerators.is_null() {
            let failure = error("Load accelerators");
            DestroyWindow(frame);
            return Err(failure);
        }
        if !paths.is_empty() {
            catalog::reload(paths, vec![]);
        }
        if let Some(warning) = read(|a| a.config.warning()) {
            frame::set_status(format!("{warning:#}"));
        }
        ShowWindow(
            frame,
            if saved.is_some_and(|w| w.maximized) {
                SW_SHOWMAXIMIZED
            } else {
                SW_SHOW
            },
        );
        UpdateWindow(frame);
        sidebar::focus();
        let mut msg: MSG = std::mem::zeroed();
        loop {
            history::refresh();
            if let Some(error) = write(|a| a.failure.take()) {
                DestroyWindow(frame);
                return Err(error);
            }
            let event = read(|a| a.worker.event());
            let wake =
                MsgWaitForMultipleObjectsEx(1, &event, u32::MAX, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            if wake == WAIT_FAILED {
                let failure = error("Wait for UI messages");
                DestroyWindow(frame);
                return Err(failure);
            }
            if wake == WAIT_OBJECT_0 {
                poll_completions();
            }
            if PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                continue;
            }
            if msg.message == WM_QUIT {
                break;
            }
            let (mdi, document_tabs) = read(|a| (a.workspace.mdi, a.workspace.tabs));
            if matches!(
                msg.message,
                WM_KEYDOWN | WM_SYSKEYDOWN | WM_LBUTTONDOWN | WM_RBUTTONDOWN
            ) {
                frame::status_input(msg.hwnd);
            }
            if let Some(document) = workspace::content(active())
                && document.find_key(&msg)
            {
                continue;
            }
            if TranslateMDISysAccel(mdi, &msg) != 0
                || TranslateAcceleratorW(frame, accelerators, &msg) != 0
                || keys::translate(&msg)
                || sidebar::translate(frame, &msg)
                || msg.hwnd == document_tabs && IsDialogMessageW(frame, &msg) != 0
            {
                continue;
            }
            let doc = active();
            if !doc.is_null()
                && (msg.hwnd == doc || IsChild(doc, msg.hwnd) != 0)
                && IsDialogMessageW(doc, &msg) != 0
            {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(())
    }
}
fn poll_completions() {
    use crate::tasks::Completion;
    for _ in 0..8 {
        match read(|a| a.worker.rx.try_recv()) {
            Ok(Completion::Loaded { id, result }) => catalog::loaded(id, result),
            Ok(Completion::Matches { id, items }) => catalog::matches(id, items),
            Ok(Completion::SearchDone { id, result }) => catalog::search_done(id, result),
            Ok(Completion::Description {
                id,
                document,
                result,
            }) => {
                if let Some(view) = workspace::by_id(document) {
                    view.complete(id, result);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                catalog::flush_results();
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                catalog::flush_results();
                catalog::worker_stopped();
                for view in workspace::views() {
                    view.worker_stopped();
                }
                panel::cancel_debounce();
                return;
            }
        }
    }
    catalog::flush_results();
    read(|a| a.worker.resignal());
}

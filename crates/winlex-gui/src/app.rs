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

mod architecture;
mod catalog;
mod document;
mod frame;
mod keys;
mod metadata;
mod panel;
mod settings;
mod sidebar;
mod sources;
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
    status: HWND,
    session: catalog::Session,
    worker: crate::worker::Worker<crate::tasks::Job, crate::tasks::Completion>,
    sidebar: sidebar::Sidebar,
    workspace: workspace::Workspace,
    fonts: Vec<Font>,
    failure: Option<String>,
}
thread_local! {static APP:RefCell<Option<App>>=const {RefCell::new(None)};}
// These closures only inspect/update owned data; never send messages inside them.
fn read<T>(f: impl FnOnce(&App) -> T) -> T {
    APP.with(|a| f(a.borrow().as_ref().unwrap()))
}
fn write<T>(f: impl FnOnce(&mut App) -> T) -> T {
    APP.with(|a| f(a.borrow_mut().as_mut().unwrap()))
}
fn dpi() -> Dpi {
    read(|a| a.dpi)
}
fn font_for(value: u32) -> (HFONT, i32) {
    if let Some(font) = read(|a| {
        a.fonts
            .iter()
            .find(|f| f.dpi == value)
            .map(|f| (f.handle, f.height))
    }) {
        return font;
    }
    let font = match Font::new(value) {
        Ok(font) => font,
        Err(error) => {
            write(|a| a.failure = Some(error));
            // Window callbacks cannot return a Rust error through the native ABI.
            // The application boundary reports it and destroys the window tree.
            return (ptr::null_mut(), 0);
        }
    };
    let result = (font.handle, font.height);
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
) -> Result<(), String> {
    let system_version = crate::platform::WindowsVersion::query()?;
    let _apartment = crate::input::Apartment::initialize()?;
    let settings = settings::Settings::load(&config);
    resources::initialize(settings.language.name())?;
    let mut session = catalog::Session::new();
    let worker = crate::worker::Worker::new(crate::tasks::run)?;
    session.sources.settings = sources::Settings::load(&config);
    let paths = if paths.is_empty() {
        session.sources.settings.auto_load.clone()
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
            status: ptr::null_mut(),
            session,
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
unsafe fn run_windows(paths: Vec<std::path::PathBuf>) -> Result<(), String> {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_TAB_CLASSES | ICC_BAR_CLASSES,
        };
        if InitCommonControlsEx(&controls) == 0 {
            return Err(error("Initialize common controls"));
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
            layout::scale(1000, value),
            layout::scale(680, value),
            ptr::null_mut(),
            menu,
            instance(),
            ptr::null(),
        );
        if frame.is_null() {
            let failure = error("Create main window");
            DestroyMenu(menu);
            return Err(failure);
        }
        if let Some(error) = write(|a| a.failure.take()) {
            DestroyWindow(frame);
            return Err(error);
        }
        settings::restore_window(frame);
        settings::update_language_menu();
        let accelerators = LoadAcceleratorsW(instance(), IDR_ACCELERATORS as usize as *const u16);
        if accelerators.is_null() {
            let failure = error("Load accelerators");
            DestroyWindow(frame);
            return Err(failure);
        }
        write(|a| a.session.sources.extra = paths.clone());
        sources::refresh();
        if !paths.is_empty() {
            catalog::load(paths);
        }
        if let Some(warning) = read(|a| a.config.warning.clone()) {
            catalog::set_status(warning);
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
            if TranslateMDISysAccel(mdi, &msg) != 0 {
                continue;
            }
            if TranslateAcceleratorW(frame, accelerators, &msg) != 0 {
                continue;
            }
            if keys::translate(&msg) {
                continue;
            }
            if sidebar::translate(frame, &msg)
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
    for _ in 0..8 {
        match read(|a| a.worker.rx.try_recv()) {
            Ok(c) => {
                use crate::tasks::Target;
                match c.target {
                    Target::Load | Target::Search => catalog::apply(c),
                    Target::Discover => sources::apply_discovery(c),
                    Target::Document(id) | Target::Reopen(id) => workspace::complete(id, c),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                catalog::worker_stopped();
                for (hwnd, view) in workspace::views() {
                    view.worker_stopped(hwnd);
                }
                panel::cancel_debounce();
                return;
            }
        }
    }
    read(|a| a.worker.resignal());
}
